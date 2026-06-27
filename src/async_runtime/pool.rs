//! Worker pool abstraction for the async runtime

use std::fmt;

use crate::{RuntimeError, RuntimeResult};

/// Worker pool configuration
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Number of worker threads to use for the runtime
    pub worker_count: usize,
    /// Maximum number of tasks allowed in the queue
    pub queue_capacity: usize,
    /// Whether work stealing is enabled
    pub enable_work_stealing: bool,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            worker_count: num_cpus::get(),
            queue_capacity: 1024,
            enable_work_stealing: true,
        }
    }
}

/// Logical worker pool used to configure the async runtime
pub struct WorkerPool {
    config: PoolConfig,
    queue: super::queue::QueueHandle,
}

impl WorkerPool {
    /// Create a new worker pool configuration
    pub fn new(config: PoolConfig) -> RuntimeResult<Self> {
        if config.worker_count == 0 {
            return Err(RuntimeError::configuration_error(
                "工作线程数量必须大于0".to_string(),
                "配置错误".to_string(),
            ));
        }

        Ok(Self {
            config,
            queue: super::queue::TaskQueue::new(),
        })
    }

    /// Get the number of worker threads
    pub fn worker_count(&self) -> usize {
        self.config.worker_count
    }

    /// Get the configured queue capacity
    pub fn queue_capacity(&self) -> usize {
        self.config.queue_capacity
    }

    /// Work stealing enabled flag
    pub fn work_stealing_enabled(&self) -> bool {
        self.config.enable_work_stealing
    }

    /// Shutdown the worker pool (no-op for logical pool)
    pub fn shutdown(&self) -> RuntimeResult<()> {
        Ok(())
    }

    /// Get a queue for a given worker index
    pub fn get_queue(&self, _index: usize) -> super::queue::QueueHandle {
        // Return a clone of the shared queue
        std::sync::Arc::clone(&self.queue)
    }

    /// Get number of pending tasks in the queue
    pub fn pending_tasks(&self) -> usize {
        self.queue.len()
    }
}

impl fmt::Debug for WorkerPool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WorkerPool")
            .field("worker_count", &self.config.worker_count)
            .field("queue_capacity", &self.config.queue_capacity)
            .field("enable_work_stealing", &self.config.enable_work_stealing)
            .field("pending_tasks", &self.queue.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pool_creation() {
        let config = PoolConfig::default();
        let pool = WorkerPool::new(config);
        assert!(pool.is_ok());
    }

    #[test]
    fn test_pool_validation() {
        let config = PoolConfig {
            worker_count: 0,
            ..Default::default()
        };
        let pool = WorkerPool::new(config);
        assert!(pool.is_err());
    }

    #[test]
    fn test_pool_properties() {
        let config = PoolConfig {
            worker_count: 4,
            queue_capacity: 4096,
            enable_work_stealing: false,
        };
        let pool = WorkerPool::new(config.clone()).unwrap();
        assert_eq!(pool.worker_count(), 4);
        assert_eq!(pool.queue_capacity(), 4096);
        assert!(!pool.work_stealing_enabled());
        assert!(format!("{:?}", pool).contains("WorkerPool"));
    }
}
