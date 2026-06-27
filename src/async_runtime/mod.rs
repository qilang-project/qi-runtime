//! Qi 异步运行时 (Async Runtime)
//!
//! This module implements a high-performance async runtime for the Qi programming language.
//! It uses Rust for the executor, scheduler, and task management, with C for low-level syscalls.
//!
//! # Architecture
//!
//! - **Executor**: Multi-threaded work-stealing task executor
//! - **Scheduler**: Cooperative coroutine scheduler with priority support
//! - **Task Queue**: Lock-free concurrent task queues
//! - **Pool**: Worker thread pool with adaptive sizing
//! - **FFI**: C bindings for low-level syscalls (epoll, kqueue, IOCP)
//!
//! # Features
//!
//! - Work-stealing task scheduler
//! - Lock-free task queues
//! - Coroutine support with stack pooling
//! - Platform-specific I/O event loops (epoll on Linux, kqueue on macOS, IOCP on Windows)
//! - Chinese keyword support for async operations
//!
//! # Example
//!
//! ```rust,no_run
//! use qi_compiler::runtime::async_runtime::{Runtime, RuntimeConfig};
//!
//! let config = RuntimeConfig::default();
//! let runtime = Runtime::new(config).unwrap();
//! runtime.block_on(async {
//!     println!("异步任务执行中...");
//! });
//! ```

pub mod executor;
pub mod ffi;
pub mod future;
pub mod pool;
pub mod queue;
pub mod scheduler;
pub mod state;
pub mod state_machine;
pub mod state_machine_poc;
pub mod task;

// Re-export core types
pub use executor::{Executor, ExecutorHandle};
pub use future::Future;
pub use pool::{PoolConfig, WorkerPool};
pub use queue::{QueueHandle, TaskQueue};
pub use scheduler::{Scheduler, SchedulerConfig};
pub use state::{AsyncState, StateManager};
pub use task::{TaskHandle, TaskId, TaskPriority, TaskStatus};

use crate::RuntimeResult;
use std::sync::Arc;
use std::time::Duration;

/// Async runtime configuration
#[derive(Debug, Clone)]
pub struct RuntimeConfig {
    /// Number of worker threads (0 = auto-detect based on CPU cores)
    pub worker_threads: usize,
    /// Task queue capacity per worker
    pub queue_capacity: usize,
    /// Maximum coroutine stack size in bytes
    pub max_stack_size: usize,
    /// Stack pool size (number of pre-allocated stacks)
    pub stack_pool_size: usize,
    /// Task polling interval
    pub poll_interval: Duration,
    /// Enable work-stealing
    pub enable_work_stealing: bool,
    /// Enable debug logging
    pub debug: bool,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        let worker_threads = num_cpus::get();
        Self {
            worker_threads,
            queue_capacity: 1024,
            max_stack_size: 2 * 1024 * 1024, // 2MB
            stack_pool_size: 128,
            poll_interval: Duration::from_millis(1),
            enable_work_stealing: true,
            debug: false,
        }
    }
}

/// Main async runtime
pub struct Runtime {
    config: RuntimeConfig,
    executor: Arc<Executor>,
    scheduler: Arc<Scheduler>,
    pool: Arc<WorkerPool>,
    state_manager: Arc<StateManager>,
}

impl Runtime {
    /// Create a new async runtime with the given configuration
    pub fn new(config: RuntimeConfig) -> RuntimeResult<Self> {
        let pool = Arc::new(WorkerPool::new(PoolConfig {
            worker_count: config.worker_threads,
            queue_capacity: config.queue_capacity,
            enable_work_stealing: config.enable_work_stealing,
        })?);

        let scheduler = Arc::new(Scheduler::new(SchedulerConfig {
            max_stack_size: config.max_stack_size,
            stack_pool_size: config.stack_pool_size,
            poll_interval: config.poll_interval,
        })?);

        let executor = Arc::new(Executor::new(Arc::clone(&pool), Arc::clone(&scheduler))?);

        let state_manager = Arc::new(StateManager::new());

        Ok(Self {
            config,
            executor,
            scheduler,
            pool,
            state_manager,
        })
    }

    /// Spawn a new async task
    pub fn spawn<F>(&self, future: F) -> TaskHandle
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.executor.spawn(future)
    }

    /// Spawn a task with priority
    pub fn spawn_with_priority<F>(&self, future: F, priority: TaskPriority) -> TaskHandle
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        self.executor.spawn_with_priority(future, priority)
    }

    /// Block on a future until it completes
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: std::future::Future,
    {
        self.executor.block_on(future)
    }

    /// Shutdown the runtime gracefully
    pub fn shutdown(self) -> RuntimeResult<()> {
        self.executor.shutdown()?;
        self.pool.shutdown()?;
        Ok(())
    }

    /// Get runtime statistics
    pub fn stats(&self) -> RuntimeStats {
        RuntimeStats {
            active_tasks: self.scheduler.active_task_count(),
            queued_tasks: self.pool.pending_tasks(),
            worker_threads: self.config.worker_threads,
            completed_tasks: self.scheduler.total_completed(),
        }
    }
}

/// Runtime statistics
#[derive(Debug, Clone)]
pub struct RuntimeStats {
    pub active_tasks: usize,
    pub queued_tasks: usize,
    pub worker_threads: usize,
    pub completed_tasks: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_creation() {
        let config = RuntimeConfig::default();
        let runtime = Runtime::new(config);
        assert!(runtime.is_ok());
    }

    #[test]
    fn test_runtime_config_default() {
        let config = RuntimeConfig::default();
        assert!(config.worker_threads > 0);
        assert_eq!(config.queue_capacity, 1024);
        assert_eq!(config.max_stack_size, 2 * 1024 * 1024);
    }
}
