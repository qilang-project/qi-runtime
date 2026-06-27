//! Memory Interface Module
//!
//! This module provides a unified interface for all memory management
//! operations including allocation, garbage collection, and memory statistics.

use std::sync::Arc;
use std::sync::Mutex;

use super::{GcConfig, MemoryManager, MemoryUsage};
use crate::{RuntimeError, RuntimeResult};

/// Unified memory interface that provides access to all memory management functionality
#[derive(Debug)]
pub struct MemoryInterface {
    /// Memory manager
    manager: Arc<Mutex<MemoryManager>>,
    /// GC configuration
    gc_config: Arc<Mutex<GcConfig>>,
    /// Memory statistics
    stats: Arc<Mutex<MemoryStats>>,
    /// Memory limits
    limits: Arc<Mutex<MemoryLimits>>,
}

/// Memory limits configuration
#[derive(Debug, Clone)]
pub struct MemoryLimits {
    /// Maximum memory usage in bytes
    pub max_memory_bytes: usize,
    /// Maximum allocation size
    pub max_allocation_size: usize,
    /// Memory usage threshold for triggering GC (percentage)
    pub gc_threshold: f64,
    /// Emergency cleanup threshold
    pub emergency_threshold: f64,
}

impl Default for MemoryLimits {
    fn default() -> Self {
        Self {
            max_memory_bytes: 1024 * 1024 * 1024,   // 1GB
            max_allocation_size: 100 * 1024 * 1024, // 100MB
            gc_threshold: 0.8,                      // 80%
            emergency_threshold: 0.95,              // 95%
        }
    }
}

impl MemoryInterface {
    /// Create new memory interface
    pub fn new() -> RuntimeResult<Self> {
        let gc_config = GcConfig::default();
        let manager = MemoryManager::new(1024, 0.8)?;
        let limits = MemoryLimits::default();

        Ok(Self {
            manager: Arc::new(Mutex::new(manager)),
            gc_config: Arc::new(Mutex::new(gc_config)),
            stats: Arc::new(Mutex::new(MemoryStats::new())),
            limits: Arc::new(Mutex::new(limits)),
        })
    }

    /// Create memory interface with custom limits
    pub fn with_limits(limits: MemoryLimits) -> RuntimeResult<Self> {
        let gc_config = GcConfig::default();
        let manager = MemoryManager::new(1024, 0.8)?;

        Ok(Self {
            manager: Arc::new(Mutex::new(manager)),
            gc_config: Arc::new(Mutex::new(gc_config)),
            stats: Arc::new(Mutex::new(MemoryStats::new())),
            limits: Arc::new(Mutex::new(limits)),
        })
    }

    /// Allocate memory
    pub fn allocate(&self, size: usize) -> RuntimeResult<*mut u8> {
        // Check limits
        {
            let limits = self.limits.lock().unwrap();
            if size > limits.max_allocation_size {
                return Err(RuntimeError::memory_error(
                    "内存分配错误",
                    &format!(
                        "请求的内存大小 {} 超过最大限制 {}",
                        size, limits.max_allocation_size
                    ),
                ));
            }
        }

        // Check memory usage and trigger GC if needed
        self.check_and_trigger_gc()?;

        // Allocate through manager
        let manager = self.manager.lock().unwrap();
        let result = manager.allocate(size, None);

        if result.is_ok() {
            self.update_allocation_stats(size, true)?;
        }

        result.map_err(|e| RuntimeError::from(e))
    }

    /// Deallocate memory
    pub fn deallocate(&self, ptr: *mut u8, _size: usize) -> RuntimeResult<()> {
        let manager = self.manager.lock().unwrap();
        let result = manager.deallocate(ptr);

        if result.is_ok() {
            self.update_allocation_stats(_size, false)?;
        }

        result.map_err(|e| RuntimeError::from(e))
    }

    /// Run garbage collection
    pub fn run_gc(&self) -> RuntimeResult<usize> {
        let manager = self.manager.lock().unwrap();
        let collected = manager.trigger_gc().map_err(|e| RuntimeError::from(e))?;

        // Update GC statistics
        {
            let mut stats = self.stats.lock().unwrap();
            stats.add_gc_cycle();
            stats.add_memory_collected(collected);
        }

        Ok(collected)
    }

    /// Get memory usage statistics
    pub fn get_memory_usage(&self) -> RuntimeResult<MemoryUsage> {
        let manager = self.manager.lock().unwrap();
        Ok(manager.get_usage())
    }

    /// Get detailed memory statistics
    pub fn get_memory_stats(&self) -> RuntimeResult<MemoryStats> {
        let stats = self.stats.lock().unwrap();
        Ok(stats.clone())
    }

    /// Set GC configuration
    pub fn set_gc_config(&self, config: GcConfig) {
        *self.gc_config.lock().unwrap() = config;
    }

    /// Get GC configuration
    pub fn get_gc_config(&self) -> GcConfig {
        self.gc_config.lock().unwrap().clone()
    }

    /// Set memory limits
    pub fn set_limits(&self, limits: MemoryLimits) {
        *self.limits.lock().unwrap() = limits;
    }

    /// Get memory limits
    pub fn get_limits(&self) -> MemoryLimits {
        self.limits.lock().unwrap().clone()
    }

    /// Get currently allocated bytes (allocated - deallocated)
    pub fn get_allocated_bytes(&self) -> u64 {
        self.stats.lock().unwrap().get_current_usage()
    }

    /// Check if GC should be triggered and run it if needed
    fn check_and_trigger_gc(&self) -> RuntimeResult<()> {
        let usage = self.get_memory_usage()?;
        let limits = self.limits.lock().unwrap();

        let usage_ratio = usage.total_allocated as f64 / limits.max_memory_bytes as f64;

        if usage_ratio > limits.gc_threshold {
            drop(limits);
            self.run_gc()?;
        }

        Ok(())
    }

    /// Update allocation statistics
    fn update_allocation_stats(&self, size: usize, is_allocation: bool) -> RuntimeResult<()> {
        let mut stats = self.stats.lock().unwrap();
        if is_allocation {
            stats.add_allocation(size);
        } else {
            stats.add_deallocation(size);
        }
        Ok(())
    }

    /// Get allocation rate (allocations per second)
    pub fn get_allocation_rate(&self) -> f64 {
        let stats = self.stats.lock().unwrap();
        stats.get_allocation_rate()
    }

    /// Get memory efficiency (used / total ratio)
    pub fn get_memory_efficiency(&self) -> f64 {
        let usage = match self.get_memory_usage() {
            Ok(u) => u,
            Err(_) => return 0.0,
        };

        if usage.total_allocated == 0 {
            return 1.0;
        }

        usage.in_use as f64 / usage.total_allocated as f64
    }

    /// Force emergency cleanup
    pub fn emergency_cleanup(&self) -> RuntimeResult<usize> {
        let mut total_collected = 0;

        // Run multiple GC cycles if needed
        for _ in 0..3 {
            match self.run_gc() {
                Ok(collected) => {
                    total_collected += collected;
                    if collected == 0 {
                        break;
                    }
                }
                Err(_) => break,
            }
        }

        Ok(total_collected)
    }

    /// Check if memory pressure is high
    pub fn is_under_pressure(&self) -> bool {
        let usage = match self.get_memory_usage() {
            Ok(u) => u,
            Err(_) => return true, // Assume pressure if we can't get stats
        };

        let limits = self.limits.lock().unwrap();
        let usage_ratio = usage.total_allocated as f64 / limits.max_memory_bytes as f64;

        usage_ratio > limits.emergency_threshold
    }

    /// Get recommended GC frequency based on allocation patterns
    pub fn get_gc_frequency_recommendation(&self) -> f64 {
        let stats = self.stats.lock().unwrap();
        let allocation_rate = stats.get_allocation_rate();

        // Base frequency on allocation rate
        if allocation_rate > 1000.0 {
            0.5 // Every 2 seconds
        } else if allocation_rate > 100.0 {
            1.0 // Every second
        } else if allocation_rate > 10.0 {
            2.0 // Every 0.5 seconds
        } else {
            5.0 // Every 0.2 seconds
        }
    }

    /// Initialize the memory interface
    pub fn initialize(&self) -> RuntimeResult<()> {
        // Reset statistics
        let mut stats = self.stats.lock().unwrap();
        *stats = MemoryStats::default();
        Ok(())
    }
}

impl Default for MemoryInterface {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

/// Memory statistics tracking
#[derive(Debug, Clone)]
pub struct MemoryStats {
    /// Total allocations performed
    pub total_allocations: u64,
    /// Total deallocations performed
    pub total_deallocations: u64,
    /// Total bytes allocated
    pub total_bytes_allocated: u64,
    /// Total bytes deallocated
    pub total_bytes_deallocated: u64,
    /// GC cycles performed
    pub gc_cycles: u64,
    /// Total memory collected by GC
    pub total_memory_collected: u64,
    /// Start time for rate calculations
    start_time: std::time::Instant,
    /// Last update time
    last_update: std::time::Instant,
}

impl MemoryStats {
    /// Create new memory statistics
    pub fn new() -> Self {
        let now = std::time::Instant::now();
        Self {
            total_allocations: 0,
            total_deallocations: 0,
            total_bytes_allocated: 0,
            total_bytes_deallocated: 0,
            gc_cycles: 0,
            total_memory_collected: 0,
            start_time: now,
            last_update: now,
        }
    }

    /// Add allocation to statistics
    pub fn add_allocation(&mut self, size: usize) {
        self.total_allocations += 1;
        self.total_bytes_allocated += size as u64;
        self.last_update = std::time::Instant::now();
    }

    /// Add deallocation to statistics
    pub fn add_deallocation(&mut self, size: usize) {
        self.total_deallocations += 1;
        self.total_bytes_deallocated += size as u64;
        self.last_update = std::time::Instant::now();
    }

    /// Add GC cycle
    pub fn add_gc_cycle(&mut self) {
        self.gc_cycles += 1;
        self.last_update = std::time::Instant::now();
    }

    /// Add memory collected by GC
    pub fn add_memory_collected(&mut self, bytes: usize) {
        self.total_memory_collected += bytes as u64;
        self.last_update = std::time::Instant::now();
    }

    /// Get allocation rate (allocations per second)
    pub fn get_allocation_rate(&self) -> f64 {
        let elapsed = self
            .last_update
            .duration_since(self.start_time)
            .as_secs_f64();
        if elapsed > 0.0 {
            self.total_allocations as f64 / elapsed
        } else {
            0.0
        }
    }

    /// Get deallocation rate (deallocations per second)
    pub fn get_deallocation_rate(&self) -> f64 {
        let elapsed = self
            .last_update
            .duration_since(self.start_time)
            .as_secs_f64();
        if elapsed > 0.0 {
            self.total_deallocations as f64 / elapsed
        } else {
            0.0
        }
    }

    /// Get current memory usage (allocated - deallocated)
    pub fn get_current_usage(&self) -> u64 {
        self.total_bytes_allocated
            .saturating_sub(self.total_bytes_deallocated)
    }

    /// Get GC efficiency (memory collected per cycle)
    pub fn get_gc_efficiency(&self) -> f64 {
        if self.gc_cycles > 0 {
            self.total_memory_collected as f64 / self.gc_cycles as f64
        } else {
            0.0
        }
    }

    /// Reset all statistics
    pub fn reset(&mut self) {
        let now = std::time::Instant::now();
        *self = Self {
            start_time: now,
            last_update: now,
            ..Default::default()
        };
    }
}

impl Default for MemoryStats {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_interface_creation() {
        let mem_interface = MemoryInterface::new().unwrap();
        let limits = mem_interface.get_limits();
        assert_eq!(limits.max_memory_bytes, 1024 * 1024 * 1024);
    }

    #[test]
    fn test_memory_interface_with_limits() {
        let custom_limits = MemoryLimits {
            max_memory_bytes: 512 * 1024 * 1024,
            max_allocation_size: 50 * 1024 * 1024,
            gc_threshold: 0.7,
            emergency_threshold: 0.9,
        };

        let mem_interface = MemoryInterface::with_limits(custom_limits).unwrap();
        let retrieved_limits = mem_interface.get_limits();
        assert_eq!(retrieved_limits.max_memory_bytes, 512 * 1024 * 1024);
    }

    #[test]
    fn test_memory_stats() {
        let mut stats = MemoryStats::new();

        stats.add_allocation(1024);
        stats.add_allocation(2048);
        stats.add_deallocation(512);
        stats.add_gc_cycle();
        stats.add_memory_collected(1024);

        assert_eq!(stats.total_allocations, 2);
        assert_eq!(stats.total_deallocations, 1);
        assert_eq!(stats.total_bytes_allocated, 3072);
        assert_eq!(stats.total_bytes_deallocated, 512);
        assert_eq!(stats.gc_cycles, 1);
        assert_eq!(stats.total_memory_collected, 1024);
        assert_eq!(stats.get_current_usage(), 2560);
    }

    #[test]
    fn test_allocation_rate() {
        let mut stats = MemoryStats::new();

        // Simulate some allocations
        for _ in 0..10 {
            stats.add_allocation(1024);
        }

        let rate = stats.get_allocation_rate();
        assert!(rate >= 0.0);
    }

    #[test]
    fn test_memory_efficiency() {
        let mem_interface = MemoryInterface::new().unwrap();

        // Initial efficiency should be 1.0 (no usage)
        let efficiency = mem_interface.get_memory_efficiency();
        assert_eq!(efficiency, 1.0);
    }
}
