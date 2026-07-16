//! Garbage Collection Implementation
//!
//! This module provides tracing garbage collection primitives for the Qi runtime.
//! The collector is an exact mark-and-sweep collector over an explicit object
//! graph maintained by the runtime.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use dashmap::{DashMap, DashSet};

use super::MemoryResult;

/// Garbage collection strategies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcStrategy {
    /// Traditional mark-and-sweep algorithm
    MarkAndSweep,
    /// Reference-counting compatibility mode
    ReferenceCounting,
    /// Generational GC placeholder
    Generational,
}

/// Garbage collection configuration
#[derive(Debug, Clone)]
pub struct GcConfig {
    /// GC strategy to use
    pub strategy: GcStrategy,
    /// Maximum pause time in milliseconds
    pub max_pause_time_ms: u64,
    /// Collection frequency threshold
    pub collection_threshold: f64,
    /// Enable incremental collection
    pub incremental: bool,
    /// Enable parallel collection
    pub parallel: bool,
}

impl Default for GcConfig {
    fn default() -> Self {
        Self {
            strategy: GcStrategy::MarkAndSweep,
            max_pause_time_ms: 100,
            collection_threshold: 0.8,
            incremental: false,
            parallel: false,
        }
    }
}

/// Garbage collection statistics
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    pub collections_performed: u64,
    pub objects_collected: u64,
    pub bytes_collected: u64,
    pub avg_collection_time_ms: f64,
    pub max_collection_time_ms: u64,
    pub collection_efficiency: f64,
}

impl GcStats {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn record_collection(&mut self, objects: u64, bytes: u64, time_ms: f64) {
        self.collections_performed += 1;
        self.objects_collected += objects;
        self.bytes_collected += bytes;

        if self.collections_performed == 1 {
            self.avg_collection_time_ms = time_ms;
        } else {
            self.avg_collection_time_ms =
                (self.avg_collection_time_ms * (self.collections_performed - 1) as f64 + time_ms)
                    / self.collections_performed as f64;
        }

        self.max_collection_time_ms = self.max_collection_time_ms.max(time_ms as u64);

        if time_ms > 0.0 {
            self.collection_efficiency = bytes as f64 / time_ms;
        }
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// Result of a garbage collection operation
#[derive(Debug, Clone)]
pub struct GcResult {
    pub objects_collected: u64,
    pub bytes_collected: u64,
    pub cycle_number: u64,
    pub strategy_used: GcStrategy,
    pub collected_objects: Vec<*const u8>,
}

impl GcResult {
    pub fn is_success(&self) -> bool {
        self.objects_collected > 0 || self.bytes_collected > 0
    }

    pub fn efficiency(&self) -> f64 {
        if self.cycle_number == 0 {
            0.0
        } else {
            self.bytes_collected as f64 / self.cycle_number as f64
        }
    }
}

/// Garbage collector implementation.
///
/// Lock-free 路径：roots / references / marked_objects 用 DashMap/DashSet，
/// hot path（add_root/add_reference/forget_object）只触 sharded lock，
/// 多线程并发不再排队等同一把全局锁。
///
/// stats 仍用 Mutex（写入很少 — 只在 GC cycle 时才写）。
#[derive(Debug)]
pub struct GarbageCollector {
    config: GcConfig,
    stats: Arc<Mutex<GcStats>>,
    roots: Arc<DashSet<*const u8>>,
    references: Arc<DashMap<*const u8, HashSet<*const u8>>>,
    marked_objects: Arc<DashSet<*const u8>>,
    current_cycle: std::sync::atomic::AtomicU64,
}

impl GarbageCollector {
    pub fn new(config: GcConfig) -> Self {
        Self {
            config,
            stats: Arc::new(Mutex::new(GcStats::new())),
            roots: Arc::new(DashSet::new()),
            references: Arc::new(DashMap::new()),
            marked_objects: Arc::new(DashSet::new()),
            current_cycle: std::sync::atomic::AtomicU64::new(0),
        }
    }

    pub fn initialize(&self) -> MemoryResult<()> {
        self.stats.lock().unwrap().reset();
        self.roots.clear();
        self.references.clear();
        self.marked_objects.clear();
        self.current_cycle
            .store(0, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    pub fn add_root(&self, ptr: *const u8) -> MemoryResult<()> {
        if !ptr.is_null() {
            self.roots.insert(ptr);
        }
        Ok(())
    }

    pub fn remove_root(&self, ptr: *const u8) -> MemoryResult<()> {
        self.roots.remove(&ptr);
        Ok(())
    }

    pub fn set_references(&self, ptr: *const u8, refs: Vec<*const u8>) -> MemoryResult<()> {
        if ptr.is_null() {
            return Ok(());
        }
        let mut new_set = HashSet::new();
        for r in refs {
            if !r.is_null() {
                new_set.insert(r);
            }
        }
        self.references.insert(ptr, new_set);
        Ok(())
    }

    pub fn add_reference(&self, from: *const u8, to: *const u8) -> MemoryResult<()> {
        if from.is_null() || to.is_null() {
            return Ok(());
        }
        self.references
            .entry(from)
            .or_insert_with(HashSet::new)
            .insert(to);
        Ok(())
    }

    pub fn clear_references(&self, ptr: *const u8) -> MemoryResult<()> {
        self.references.remove(&ptr);
        Ok(())
    }

    pub fn forget_object(&self, ptr: *const u8) -> MemoryResult<()> {
        self.remove_root(ptr)?;
        self.clear_references(ptr)?;
        // 把所有还引用 ptr 的边都摘掉 — 必须遍历整个 references map
        for mut entry in self.references.iter_mut() {
            entry.value_mut().remove(&ptr);
        }
        Ok(())
    }

    pub fn roots_snapshot(&self) -> HashSet<*const u8> {
        self.roots.iter().map(|r| *r).collect()
    }

    pub fn references_snapshot(&self) -> HashMap<*const u8, HashSet<*const u8>> {
        self.references
            .iter()
            .map(|e| (*e.key(), e.value().clone()))
            .collect()
    }

    pub fn get_stats(&self) -> GcStats {
        self.stats.lock().unwrap().clone()
    }

    pub fn current_cycle(&self) -> u64 {
        self.current_cycle
            .load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 开始一轮标记：清空上轮标记 + 从 roots 做可达性标记，返回本轮 cycle 号。
    /// 与 [`is_unreachable`] / [`record_cycle`] 配合，供 MemoryManager 做
    /// **免快照**清扫（直接迭代它自己的 allocations DashMap，不再构建全堆
    /// HashMap 快照）。
    pub fn begin_mark(&self) -> MemoryResult<u64> {
        let cycle = self
            .current_cycle
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;
        self.marked_objects.clear();
        self.mark_phase()?;
        Ok(cycle)
    }

    /// 清扫判定：既没被本轮标记、也不是 root（root 在清扫时再查一次，
    /// 保护标记之后才新分配的对象 —— allocate() 出生即 add_root）。
    pub fn is_unreachable(&self, ptr: *const u8) -> bool {
        !self.marked_objects.contains(&ptr) && !self.roots.contains(&ptr)
    }

    /// 记录一轮收集的统计（免快照路径由 MemoryManager 汇总后回填）。
    pub fn record_cycle(&self, objects: u64, bytes: u64, time_ms: f64) {
        self.stats
            .lock()
            .unwrap()
            .record_collection(objects, bytes, time_ms);
    }

    pub fn collect(&self, heap: &HashMap<*const u8, usize>) -> MemoryResult<GcResult> {
        let start_time = std::time::Instant::now();
        let cycle = self
            .current_cycle
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            + 1;

        self.marked_objects.clear();
        self.mark_phase()?;

        let collected_objects = self.sweep_candidates(heap);
        let bytes_collected: u64 = collected_objects
            .iter()
            .filter_map(|ptr| heap.get(ptr).copied())
            .map(|size| size as u64)
            .sum();

        let elapsed = start_time.elapsed();
        let time_ms = elapsed.as_millis() as f64;

        let result = GcResult {
            objects_collected: collected_objects.len() as u64,
            bytes_collected,
            cycle_number: cycle,
            strategy_used: self.config.strategy,
            collected_objects,
        };

        self.stats.lock().unwrap().record_collection(
            result.objects_collected,
            result.bytes_collected,
            time_ms,
        );

        Ok(result)
    }

    fn mark_phase(&self) -> MemoryResult<()> {
        let roots: Vec<*const u8> = self.roots.iter().map(|r| *r).collect();
        for root in roots {
            self.mark_object(root);
        }
        Ok(())
    }

    fn mark_object(&self, obj: *const u8) {
        if obj.is_null() {
            return;
        }
        if !self.marked_objects.insert(obj) {
            return; // 已经 marked
        }

        let children: Vec<*const u8> = self
            .references
            .get(&obj)
            .map(|refs| refs.iter().copied().collect())
            .unwrap_or_default();

        for child in children {
            self.mark_object(child);
        }
    }

    fn sweep_candidates(&self, heap: &HashMap<*const u8, usize>) -> Vec<*const u8> {
        heap.keys()
            .copied()
            .filter(|ptr| !self.marked_objects.contains(ptr) && !self.roots.contains(ptr))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gc_config_default() {
        let config = GcConfig::default();
        assert_eq!(config.strategy, GcStrategy::MarkAndSweep);
        assert_eq!(config.max_pause_time_ms, 100);
        assert_eq!(config.collection_threshold, 0.8);
    }

    #[test]
    fn test_gc_creation_and_init() {
        let mut gc = GarbageCollector::new(GcConfig::default());
        assert_eq!(gc.current_cycle(), 0);
        gc.initialize().unwrap();
        assert_eq!(gc.current_cycle(), 0);
    }

    #[test]
    fn test_root_and_reference_graph() {
        let gc = GarbageCollector::new(GcConfig::default());
        let a = 0x1000 as *const u8;
        let b = 0x2000 as *const u8;

        gc.add_root(a).unwrap();
        gc.add_reference(a, b).unwrap();

        assert!(gc.roots_snapshot().contains(&a));
        assert!(gc.references_snapshot().get(&a).unwrap().contains(&b));
    }

    #[test]
    fn test_mark_and_sweep_collects_unreachable() {
        let mut gc = GarbageCollector::new(GcConfig::default());
        gc.initialize().unwrap();

        let a = 0x1000 as *const u8;
        let b = 0x2000 as *const u8;
        let c = 0x3000 as *const u8;

        gc.add_root(a).unwrap();
        gc.add_reference(a, b).unwrap();

        let mut heap = HashMap::new();
        heap.insert(a, 64);
        heap.insert(b, 64);
        heap.insert(c, 64);

        let result = gc.collect(&heap).unwrap();
        assert_eq!(result.objects_collected, 1);
        assert_eq!(result.bytes_collected, 64);
        assert_eq!(result.collected_objects, vec![c]);
    }

    #[test]
    fn test_forget_object() {
        let gc = GarbageCollector::new(GcConfig::default());
        let a = 0x1000 as *const u8;
        let b = 0x2000 as *const u8;

        gc.add_root(a).unwrap();
        gc.add_reference(a, b).unwrap();
        gc.forget_object(b).unwrap();

        let refs = gc.references_snapshot();
        assert!(!refs.get(&a).map(|s| s.contains(&b)).unwrap_or(false));
    }
}
