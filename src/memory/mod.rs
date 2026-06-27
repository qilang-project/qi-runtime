//! Memory Management Subsystem
//!
//! This module provides comprehensive memory management for the Qi runtime,
//! including allocation strategies, garbage collection, and resource tracking.

pub mod allocator;
pub mod gc;
pub mod interface;
pub mod manager;

// Re-export main components
pub use allocator::{AllocationStrategy, ArenaAllocator, BumpAllocator, HybridAllocator};
pub use gc::{GarbageCollector, GcConfig, GcStats, GcStrategy};
pub use interface::{MemoryInterface, MemoryLimits, MemoryStats};
pub use manager::MemoryManager;

/// Memory allocation result type
pub type MemoryResult<T> = Result<T, MemoryError>;

/// Memory management errors
#[derive(Debug, thiserror::Error)]
pub enum MemoryError {
    #[error("内存分配失败: 请求 {requested} 字节，可用 {available} 字节")]
    AllocationFailed { requested: usize, available: usize },

    #[error("内存释放失败: 地址 {address:p} 无效")]
    DeallocationFailed { address: *const u8 },

    #[error("垃圾回收失败: {reason}")]
    GarbageCollectionFailed { reason: String },

    #[error("内存不足: 无法分配 {size} 字节")]
    OutOfMemory { size: usize },

    #[error("内存损坏: 检测到无效的内存状态")]
    CorruptedMemory,
}

/// Memory allocation strategies
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocatorType {
    /// Fast allocation for short-lived objects
    Bump,
    /// Region-based allocation for program lifetime objects
    Arena,
    /// General-purpose allocator
    Generic,
    /// Hybrid strategy combining multiple approaches
    Hybrid,
}

/// Memory usage statistics
#[derive(Debug, Clone, Default)]
pub struct MemoryUsage {
    /// Total allocated bytes
    pub total_allocated: usize,
    /// Currently in-use bytes
    pub in_use: usize,
    /// Peak usage bytes
    pub peak_usage: usize,
    /// Number of allocations
    pub allocation_count: u64,
    /// Number of deallocations
    pub deallocation_count: u64,
    /// Number of garbage collections performed
    pub gc_count: u64,
}

impl MemoryUsage {
    /// Create new memory usage statistics
    pub fn new() -> Self {
        Self::default()
    }

    /// Get memory usage in megabytes
    pub fn usage_mb(&self) -> f64 {
        self.in_use as f64 / (1024.0 * 1024.0)
    }

    /// Get allocation efficiency (in_use / total_allocated)
    pub fn efficiency(&self) -> f64 {
        if self.total_allocated == 0 {
            1.0
        } else {
            self.in_use as f64 / self.total_allocated as f64
        }
    }

    /// Update statistics after allocation
    pub fn record_allocation(&mut self, size: usize) {
        self.total_allocated += size;
        self.in_use += size;
        self.allocation_count += 1;
        self.peak_usage = self.peak_usage.max(self.in_use);
    }

    /// Update statistics after deallocation
    pub fn record_deallocation(&mut self, size: usize) {
        self.in_use = self.in_use.saturating_sub(size);
        self.deallocation_count += 1;
    }

    /// Update statistics after garbage collection
    pub fn record_gc(&mut self, freed_bytes: usize) {
        self.in_use = self.in_use.saturating_sub(freed_bytes);
        self.gc_count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_usage() {
        let mut usage = MemoryUsage::new();

        assert_eq!(usage.total_allocated, 0);
        assert_eq!(usage.in_use, 0);
        assert_eq!(usage.efficiency(), 1.0);

        usage.record_allocation(1024);
        assert_eq!(usage.total_allocated, 1024);
        assert_eq!(usage.in_use, 1024);
        assert_eq!(usage.allocation_count, 1);

        usage.record_deallocation(512);
        assert_eq!(usage.in_use, 512);
        assert_eq!(usage.deallocation_count, 1);

        assert!(usage.efficiency() > 0.0 && usage.efficiency() <= 1.0);
    }

    #[test]
    fn test_memory_usage_mb() {
        let mut usage = MemoryUsage::new();
        usage.record_allocation(1024 * 1024); // 1 MB

        assert_eq!(usage.usage_mb(), 1.0);
    }

    #[test]
    fn test_allocator_type() {
        assert_eq!(AllocatorType::Bump, AllocatorType::Bump);
        assert_ne!(AllocatorType::Bump, AllocatorType::Arena);
    }

    #[test]
    fn test_memory_error_display() {
        let error = MemoryError::OutOfMemory { size: 1024 };
        let message = error.to_string();
        assert!(message.contains("1024"));
        assert!(message.contains("内存不足"));
    }
}
