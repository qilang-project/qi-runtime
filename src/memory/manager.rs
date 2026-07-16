//! Memory Manager Implementation
//!
//! This module provides the core memory management functionality for the Qi runtime,
//! including allocation tracking, tracing GC integration, and exact deallocation.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::DashMap;

use super::{AllocatorType, GarbageCollector, GcConfig, MemoryError, MemoryResult, MemoryUsage};

/// Main memory manager for the Qi runtime.
///
/// Hot path（allocate / deallocate / add_root / add_reference）走 DashMap +
/// 原子计数器，无全局 mutex；外层 `RUNTIME` mutex 也不再需要持有。
#[derive(Debug)]
pub struct MemoryManager {
    /// 当前使用字节数（原子）
    in_use_bytes: Arc<AtomicUsize>,
    /// 累计分配字节数（原子，info only）
    total_allocated: Arc<AtomicUsize>,
    /// GC 周期数（原子，info only）
    gc_cycles: Arc<AtomicUsize>,
    /// 仅在偶尔需要 full snapshot 时拿（getMetrics）
    usage_meta: Arc<Mutex<MemoryUsage>>,
    allocation_strategy: AllocatorType,
    #[allow(dead_code)]
    gc_config: GcConfig,
    max_memory_bytes: usize,
    allocations: Arc<DashMap<*const u8, AllocationInfo>>,
    gc: GarbageCollector,
    gc_threshold: f64,
}

/// Information about a memory allocation
#[derive(Debug, Clone)]
struct AllocationInfo {
    size: usize,
    align: usize,
    allocator_type: AllocatorType,
}

impl MemoryManager {
    /// Create a new memory manager
    pub fn new(max_memory_mb: usize, gc_threshold: f64) -> MemoryResult<Self> {
        if max_memory_mb == 0 {
            return Err(MemoryError::AllocationFailed {
                requested: 0,
                available: 0,
            });
        }

        let max_memory_bytes = max_memory_mb * 1024 * 1024;
        let gc_config = GcConfig::default();

        Ok(Self {
            in_use_bytes: Arc::new(AtomicUsize::new(0)),
            total_allocated: Arc::new(AtomicUsize::new(0)),
            gc_cycles: Arc::new(AtomicUsize::new(0)),
            usage_meta: Arc::new(Mutex::new(MemoryUsage::new())),
            allocation_strategy: AllocatorType::Hybrid,
            gc_config: gc_config.clone(),
            max_memory_bytes,
            allocations: Arc::new(DashMap::new()),
            gc: GarbageCollector::new(gc_config),
            gc_threshold: gc_threshold.clamp(0.1, 0.95),
        })
    }

    /// Initialize the memory manager
    pub fn initialize(&self) -> MemoryResult<()> {
        self.in_use_bytes.store(0, Ordering::Relaxed);
        self.total_allocated.store(0, Ordering::Relaxed);
        self.gc_cycles.store(0, Ordering::Relaxed);
        *self.usage_meta.lock().unwrap() = MemoryUsage::new();
        self.allocations.clear();
        self.gc.initialize()?;
        Ok(())
    }

    /// Allocate memory with specified size and strategy
    pub fn allocate(&self, size: usize, strategy: Option<AllocatorType>) -> MemoryResult<*mut u8> {
        if size == 0 {
            return Err(MemoryError::AllocationFailed {
                requested: size,
                available: self.get_available_memory(),
            });
        }

        let current = self.in_use_bytes.load(Ordering::Relaxed);
        if current + size > self.max_memory_bytes {
            return Err(MemoryError::OutOfMemory { size });
        }

        let allocator_type = strategy.unwrap_or(self.allocation_strategy);
        let align = 8;
        let ptr = unsafe { self.allocate_raw(size, align)? };

        let info = AllocationInfo {
            size,
            align,
            allocator_type,
        };

        self.allocations.insert(ptr as *const u8, info);
        self.gc.add_root(ptr as *const u8)?;
        self.gc.clear_references(ptr as *const u8)?;

        self.in_use_bytes.fetch_add(size, Ordering::Relaxed);
        self.total_allocated.fetch_add(size, Ordering::Relaxed);

        // 常规追踪 GC 已移到后台收集线程（见 executor 的后台 GC，read 锁下收集，
        // 不偷请求线程），避开分配热路径上的全堆 stop-the-world 扫描导致的尾延迟尖峰。
        // 这里只留一个濒临上限(>95%)的兜底：后台来不及时同步收一次，防 OOM。
        // 常态 web 负载占用远达不到 95%，故热路径不会命中这条。
        if self.get_usage_ratio() > 0.95 {
            let _ = self.trigger_gc();
        }

        Ok(ptr)
    }

    /// Deallocate memory
    pub fn deallocate(&self, ptr: *mut u8) -> MemoryResult<()> {
        if ptr.is_null() {
            return Ok(());
        }

        let info = self
            .allocations
            .remove(&(ptr as *const u8))
            .map(|(_, v)| v)
            .ok_or(MemoryError::DeallocationFailed {
                address: ptr as *const u8,
            })?;

        unsafe { self.deallocate_raw_with_layout(ptr, info.size, info.align)? };
        self.gc.forget_object(ptr as *const u8)?;
        self.in_use_bytes.fetch_sub(info.size, Ordering::Relaxed);

        Ok(())
    }

    /// Trigger garbage collection —— 免快照 mark-sweep。
    ///
    /// 旧实现每轮先把整个 allocations DashMap 拷进一个临时 HashMap（O(N) 哈希
    /// + 分配，还要挨个 shard 拿读锁），map 大时一次收集能拖出几百 ms 的
    /// stop-the-world 尾尖。现在：先 begin_mark() 做可达性标记，然后直接
    /// **迭代 DashMap 本身**挑出不可达指针（shard 级短读锁、零快照分配），
    /// 最后逐个 remove + 释放。
    ///
    /// 双重释放防护：remove 返回 None（已被 deallocate()/并发收集摘走）就
    /// 跳过，不再碰指针 —— DashMap 的 remove 所有权令牌语义保证每个指针
    /// 只会被真正释放一次。
    pub fn trigger_gc(&self) -> MemoryResult<usize> {
        let 开始 = std::time::Instant::now();
        self.gc.begin_mark()?;

        // 挑出不可达对象（is_unreachable 在此刻再查一次 roots，
        // 标记之后才诞生的对象出生即 root，不会被误收）
        let 待收: Vec<*const u8> = self
            .allocations
            .iter()
            .map(|e| *e.key())
            .filter(|ptr| self.gc.is_unreachable(*ptr))
            .collect();

        let mut freed_bytes = 0usize;
        let mut freed_objects = 0u64;
        for ptr in 待收 {
            if let Some((_, info)) = self.allocations.remove(&ptr) {
                unsafe { self.deallocate_raw_with_layout(ptr as *mut u8, info.size, info.align)? };
                self.gc.forget_object(ptr)?;
                freed_bytes += info.size;
                freed_objects += 1;
                self.in_use_bytes.fetch_sub(info.size, Ordering::Relaxed);
            }
        }

        self.gc.record_cycle(
            freed_objects,
            freed_bytes as u64,
            开始.elapsed().as_secs_f64() * 1000.0,
        );
        self.gc_cycles.fetch_add(1, Ordering::Relaxed);
        Ok(freed_bytes)
    }

    /// Check if garbage collection should be triggered based on memory usage
    pub fn should_collect(&self) -> bool {
        self.get_usage_ratio() > self.gc_threshold
    }

    /// 累计分配字节（单调递增）。后台 GC 用它算「自上次收集以来新分配了多少」，
    /// 按增量而非占用比触发——把每次收集前的堆增长封顶，避免堆涨到阈值才收那一下巨扫。
    pub fn total_allocated_bytes(&self) -> usize {
        self.total_allocated.load(Ordering::Relaxed)
    }

    /// Trigger garbage collection and return bytes freed
    pub fn collect(&self) -> MemoryResult<usize> {
        self.trigger_gc()
    }

    /// Register an allocation as a GC root
    pub fn add_root(&self, ptr: *mut u8) -> MemoryResult<()> {
        self.gc.add_root(ptr as *const u8)
    }

    /// Remove an allocation from the GC root set
    pub fn remove_root(&self, ptr: *mut u8) -> MemoryResult<()> {
        self.gc.remove_root(ptr as *const u8)
    }

    /// Add a reference edge from one heap object to another
    pub fn add_reference(&self, from: *mut u8, to: *mut u8) -> MemoryResult<()> {
        self.gc.add_reference(from as *const u8, to as *const u8)
    }

    /// Replace all outgoing references for an object
    pub fn set_references(&self, from: *mut u8, refs: Vec<*mut u8>) -> MemoryResult<()> {
        let refs: Vec<*const u8> = refs.into_iter().map(|ptr| ptr as *const u8).collect();
        self.gc.set_references(from as *const u8, refs)
    }

    /// Clear outgoing references for an object
    pub fn clear_references(&self, ptr: *mut u8) -> MemoryResult<()> {
        self.gc.clear_references(ptr as *const u8)
    }

    /// Get current memory usage in megabytes
    pub fn get_current_usage_mb(&self) -> f64 {
        self.in_use_bytes.load(Ordering::Relaxed) as f64 / (1024.0 * 1024.0)
    }

    /// Get total allocated bytes
    pub fn get_total_allocated(&self) -> usize {
        self.total_allocated.load(Ordering::Relaxed)
    }

    /// Get currently in-use bytes
    pub fn get_in_use_bytes(&self) -> usize {
        self.in_use_bytes.load(Ordering::Relaxed)
    }

    /// Get available memory bytes
    pub fn get_available_memory(&self) -> usize {
        self.max_memory_bytes
            .saturating_sub(self.get_in_use_bytes())
    }

    /// Get memory usage statistics — slow path，构造一次 snapshot
    pub fn get_usage(&self) -> MemoryUsage {
        let mut u = self.usage_meta.lock().unwrap().clone();
        u.in_use = self.in_use_bytes.load(Ordering::Relaxed);
        u.total_allocated = self.total_allocated.load(Ordering::Relaxed);
        u
    }

    /// Set allocation strategy
    pub fn set_allocation_strategy(&mut self, strategy: AllocatorType) {
        self.allocation_strategy = strategy;
    }

    /// Get allocation strategy
    pub fn get_allocation_strategy(&self) -> AllocatorType {
        self.allocation_strategy
    }

    fn get_usage_ratio(&self) -> f64 {
        if self.max_memory_bytes == 0 {
            0.0
        } else {
            self.get_in_use_bytes() as f64 / self.max_memory_bytes as f64
        }
    }

    fn should_trigger_gc(&self) -> bool {
        self.should_collect()
    }

    unsafe fn allocate_raw(&self, size: usize, align: usize) -> MemoryResult<*mut u8> {
        let layout = std::alloc::Layout::from_size_align(size, align).map_err(|_| {
            MemoryError::AllocationFailed {
                requested: size,
                available: self.get_available_memory(),
            }
        })?;

        let ptr = std::alloc::alloc(layout);
        if ptr.is_null() {
            return Err(MemoryError::AllocationFailed {
                requested: size,
                available: self.get_available_memory(),
            });
        }

        Ok(ptr)
    }

    unsafe fn deallocate_raw_with_layout(
        &self,
        ptr: *mut u8,
        size: usize,
        align: usize,
    ) -> MemoryResult<()> {
        if ptr.is_null() {
            return Ok(());
        }

        let layout = std::alloc::Layout::from_size_align(size, align)
            .map_err(|_| MemoryError::CorruptedMemory)?;
        std::alloc::dealloc(ptr, layout);
        Ok(())
    }
}

impl Drop for MemoryManager {
    fn drop(&mut self) {
        // 把所有还活着的 alloc 一次性收掉
        let remaining: Vec<(*const u8, AllocationInfo)> = self
            .allocations
            .iter()
            .map(|e| (*e.key(), e.value().clone()))
            .collect();
        self.allocations.clear();

        for (ptr, info) in remaining {
            let _ = self.gc.forget_object(ptr);
            let _ =
                unsafe { self.deallocate_raw_with_layout(ptr as *mut u8, info.size, info.align) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_manager_creation() {
        let manager = MemoryManager::new(1024, 0.8).unwrap();
        assert_eq!(manager.max_memory_bytes, 1024 * 1024 * 1024);
        assert_eq!(manager.gc_threshold, 0.8);
    }

    #[test]
    fn test_memory_allocation_and_deallocation() {
        let mut manager = MemoryManager::new(1024, 0.8).unwrap();
        manager.initialize().unwrap();

        let ptr = manager.allocate(1024, None).unwrap();
        assert!(!ptr.is_null());
        assert_eq!(manager.get_usage().in_use, 1024);

        manager.deallocate(ptr).unwrap();
        assert_eq!(manager.get_usage().in_use, 0);
    }

    #[test]
    fn test_memory_limits() {
        let mut manager = MemoryManager::new(1, 0.8).unwrap();
        manager.initialize().unwrap();

        let result = manager.allocate(2 * 1024 * 1024, None);
        assert!(matches!(
            result.unwrap_err(),
            MemoryError::OutOfMemory { .. }
        ));
    }

    #[test]
    fn test_gc_collects_unreachable_allocations() {
        let mut manager = MemoryManager::new(1024, 0.1).unwrap();
        manager.initialize().unwrap();

        let root = manager.allocate(64, None).unwrap();
        let child = manager.allocate(64, None).unwrap();
        let orphan = manager.allocate(64, None).unwrap();

        manager.add_reference(root, child).unwrap();
        manager.remove_root(child).unwrap();
        manager.remove_root(orphan).unwrap();

        let freed = manager.trigger_gc().unwrap();
        assert_eq!(freed, 64);
        assert_eq!(manager.get_usage().in_use, 128);

        manager.deallocate(root).unwrap();
        manager.deallocate(child).unwrap();
    }

    #[test]
    fn test_gc_collects_root_graph_when_root_removed() {
        let mut manager = MemoryManager::new(1024, 0.1).unwrap();
        manager.initialize().unwrap();

        let root = manager.allocate(64, None).unwrap();
        let child = manager.allocate(64, None).unwrap();

        manager.add_reference(root, child).unwrap();
        manager.remove_root(child).unwrap();
        manager.remove_root(root).unwrap();

        let freed = manager.trigger_gc().unwrap();
        assert_eq!(freed, 128);
        assert_eq!(manager.get_usage().in_use, 0);
    }
}
