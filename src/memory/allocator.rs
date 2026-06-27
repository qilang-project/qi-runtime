//! Memory Allocation Strategies
//!
//! This module provides different allocation strategies for the Qi runtime,
//! including bump allocator, arena allocator, and hybrid allocator.

use super::{MemoryError, MemoryResult};

/// Allocation strategy interface
pub trait AllocationStrategy {
    /// Allocate memory with given size
    fn allocate(&mut self, size: usize) -> MemoryResult<*mut u8>;

    /// Deallocate memory
    fn deallocate(&mut self, ptr: *mut u8, size: usize) -> MemoryResult<()>;

    /// Get current usage statistics
    fn get_usage(&self) -> usize;

    /// Reset the allocator (clear all allocations)
    fn reset(&mut self);
}

/// Fast bump allocator for short-lived objects
#[derive(Debug)]
pub struct BumpAllocator {
    /// Start of the allocation arena
    start: *mut u8,
    /// Current bump pointer
    current: *mut u8,
    /// End of the allocation arena
    end: *mut u8,
    /// Total allocated bytes
    allocated: usize,
}

impl BumpAllocator {
    /// Create a new bump allocator with specified capacity
    pub fn new(capacity: usize) -> MemoryResult<Self> {
        if capacity == 0 {
            return Err(MemoryError::AllocationFailed {
                requested: 0,
                available: 0,
            });
        }

        unsafe {
            let layout = std::alloc::Layout::from_size_align(capacity, std::mem::align_of::<u8>())
                .map_err(|_| MemoryError::AllocationFailed {
                    requested: capacity,
                    available: 0,
                })?;

            let start = std::alloc::alloc(layout);
            if start.is_null() {
                return Err(MemoryError::AllocationFailed {
                    requested: capacity,
                    available: 0,
                });
            }

            Ok(Self {
                start,
                current: start,
                end: start.add(capacity),
                allocated: 0,
            })
        }
    }

    /// Get remaining capacity
    pub fn remaining(&self) -> usize {
        unsafe { self.end.offset_from(self.current) as usize }
    }
}

impl AllocationStrategy for BumpAllocator {
    fn allocate(&mut self, size: usize) -> MemoryResult<*mut u8> {
        if size == 0 {
            return Err(MemoryError::AllocationFailed {
                requested: 0,
                available: self.remaining(),
            });
        }

        // Align to 8 bytes
        let aligned_size = (size + 7) & !7;

        if self.remaining() < aligned_size {
            return Err(MemoryError::AllocationFailed {
                requested: size,
                available: self.remaining(),
            });
        }

        let ptr = self.current;
        unsafe {
            self.current = self.current.add(aligned_size);
        }
        self.allocated += aligned_size;

        Ok(ptr)
    }

    fn deallocate(&mut self, _ptr: *mut u8, _size: usize) -> MemoryResult<()> {
        // Bump allocator doesn't support individual deallocation
        // This is a no-op
        Ok(())
    }

    fn get_usage(&self) -> usize {
        self.allocated
    }

    fn reset(&mut self) {
        self.current = self.start;
        self.allocated = 0;
    }
}

impl Drop for BumpAllocator {
    fn drop(&mut self) {
        unsafe {
            if !self.start.is_null() {
                let layout = std::alloc::Layout::from_size_align(
                    self.end.offset_from(self.start) as usize,
                    std::mem::align_of::<u8>(),
                )
                .unwrap();
                std::alloc::dealloc(self.start, layout);
            }
        }
    }
}

unsafe impl Send for BumpAllocator {}

/// Arena allocator for program lifetime objects
#[derive(Debug)]
pub struct ArenaAllocator {
    /// Storage for allocated objects
    storage: Vec<u8>,
    /// List of allocated blocks with their sizes
    allocations: Vec<(*mut u8, usize)>,
}

impl ArenaAllocator {
    /// Create a new arena allocator
    pub fn new() -> Self {
        Self {
            storage: Vec::new(),
            allocations: Vec::new(),
        }
    }

    /// Create a new arena allocator with initial capacity
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            storage: Vec::with_capacity(capacity),
            allocations: Vec::new(),
        }
    }
}

impl Default for ArenaAllocator {
    fn default() -> Self {
        Self::new()
    }
}

impl AllocationStrategy for ArenaAllocator {
    fn allocate(&mut self, size: usize) -> MemoryResult<*mut u8> {
        if size == 0 {
            return Err(MemoryError::AllocationFailed {
                requested: 0,
                available: 0,
            });
        }

        // Reserve space if needed
        if self.storage.len() + size > self.storage.capacity() {
            self.storage.reserve(size * 2);
        }

        let start_ptr = self.storage.len();
        self.storage.resize(start_ptr + size, 0);
        let ptr = unsafe { self.storage.as_mut_ptr().add(start_ptr) };

        self.allocations.push((ptr, size));
        Ok(ptr)
    }

    fn deallocate(&mut self, _ptr: *mut u8, _size: usize) -> MemoryResult<()> {
        // Arena allocator doesn't support individual deallocation
        // This is a no-op
        Ok(())
    }

    fn get_usage(&self) -> usize {
        self.storage.len()
    }

    fn reset(&mut self) {
        self.storage.clear();
        self.allocations.clear();
    }
}

unsafe impl Send for ArenaAllocator {}

/// Hybrid allocator that combines multiple strategies
#[derive(Debug)]
pub struct HybridAllocator {
    /// Bump allocator for small, short-lived objects
    bump: BumpAllocator,
    /// Arena allocator for larger, long-lived objects
    arena: ArenaAllocator,
    /// Threshold for switching between allocators
    threshold: usize,
    /// Total allocated bytes
    total_allocated: usize,
}

impl HybridAllocator {
    /// Create a new hybrid allocator
    pub fn new(bump_capacity: usize, threshold: usize) -> MemoryResult<Self> {
        if bump_capacity == 0 || threshold == 0 {
            return Err(MemoryError::AllocationFailed {
                requested: 0,
                available: 0,
            });
        }

        let bump = BumpAllocator::new(bump_capacity)?;

        Ok(Self {
            bump,
            arena: ArenaAllocator::new(),
            threshold,
            total_allocated: 0,
        })
    }
}

impl AllocationStrategy for HybridAllocator {
    fn allocate(&mut self, size: usize) -> MemoryResult<*mut u8> {
        if size == 0 {
            return Err(MemoryError::AllocationFailed {
                requested: 0,
                available: 0,
            });
        }

        let ptr = if size <= self.threshold {
            // Use bump allocator for small allocations
            self.bump.allocate(size)?
        } else {
            // Use arena allocator for large allocations
            self.arena.allocate(size)?
        };

        self.total_allocated += size;
        Ok(ptr)
    }

    fn deallocate(&mut self, ptr: *mut u8, size: usize) -> MemoryResult<()> {
        self.total_allocated = self.total_allocated.saturating_sub(size);

        // Try bump allocator first
        if size <= self.threshold {
            self.bump.deallocate(ptr, size)
        } else {
            self.arena.deallocate(ptr, size)
        }
    }

    fn get_usage(&self) -> usize {
        self.total_allocated
    }

    fn reset(&mut self) {
        self.bump.reset();
        self.arena.reset();
        self.total_allocated = 0;
    }
}

unsafe impl Send for HybridAllocator {}

/// Generic allocator wrapper that can use any allocation strategy
#[derive(Debug)]
pub struct GenericAllocator<T: AllocationStrategy> {
    strategy: T,
}

impl<T: AllocationStrategy> GenericAllocator<T> {
    /// Create a new generic allocator with the given strategy
    pub fn new(strategy: T) -> Self {
        Self { strategy }
    }

    /// Get a reference to the inner strategy
    pub fn inner(&self) -> &T {
        &self.strategy
    }

    /// Get a mutable reference to the inner strategy
    pub fn inner_mut(&mut self) -> &mut T {
        &mut self.strategy
    }
}

impl<T: AllocationStrategy> AllocationStrategy for GenericAllocator<T> {
    fn allocate(&mut self, size: usize) -> MemoryResult<*mut u8> {
        self.strategy.allocate(size)
    }

    fn deallocate(&mut self, ptr: *mut u8, size: usize) -> MemoryResult<()> {
        self.strategy.deallocate(ptr, size)
    }

    fn get_usage(&self) -> usize {
        self.strategy.get_usage()
    }

    fn reset(&mut self) {
        self.strategy.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bump_allocator() {
        let mut allocator = BumpAllocator::new(1024).unwrap();

        let ptr1 = allocator.allocate(100).unwrap();
        let ptr2 = allocator.allocate(200).unwrap();

        assert!(!ptr1.is_null());
        assert!(!ptr2.is_null());
        assert!(ptr2 > ptr1); // Should be allocated after ptr1

        assert_eq!(allocator.get_usage(), 304); // 104 (100 aligned to 8) + 200 (already aligned)

        allocator.reset();
        assert_eq!(allocator.get_usage(), 0);
    }

    #[test]
    fn test_arena_allocator() {
        let mut allocator = ArenaAllocator::new();

        let ptr1 = allocator.allocate(100).unwrap();
        let ptr2 = allocator.allocate(200).unwrap();

        assert!(!ptr1.is_null());
        assert!(!ptr2.is_null());

        assert_eq!(allocator.get_usage(), 300);

        allocator.reset();
        assert_eq!(allocator.get_usage(), 0);
    }

    #[test]
    fn test_hybrid_allocator() {
        let mut allocator = HybridAllocator::new(1024, 256).unwrap();

        // Small allocation should use bump allocator
        let small_ptr = allocator.allocate(100).unwrap();
        assert!(!small_ptr.is_null());

        // Large allocation should use arena allocator
        let large_ptr = allocator.allocate(500).unwrap();
        assert!(!large_ptr.is_null());

        assert_eq!(allocator.get_usage(), 600);

        allocator.reset();
        assert_eq!(allocator.get_usage(), 0);
    }

    #[test]
    fn test_allocator_overflow() {
        let mut allocator = BumpAllocator::new(100).unwrap();

        // This should succeed
        let _ptr = allocator.allocate(50).unwrap();

        // This should fail - not enough space
        let result = allocator.allocate(100);
        assert!(result.is_err());
        assert!(matches!(
            result.unwrap_err(),
            MemoryError::AllocationFailed { .. }
        ));
    }

    #[test]
    fn test_zero_size_allocation() {
        let mut allocator = BumpAllocator::new(1024).unwrap();

        let result = allocator.allocate(0);
        assert!(result.is_err());
    }

    #[test]
    fn test_generic_allocator() {
        let bump = BumpAllocator::new(1024).unwrap();
        let mut allocator = GenericAllocator::new(bump);

        let ptr = allocator.allocate(100).unwrap();
        assert!(!ptr.is_null());

        assert_eq!(allocator.get_usage(), 104); // Aligned to 8 bytes

        allocator.reset();
        assert_eq!(allocator.get_usage(), 0);
    }
}
