//! Task executor for the async runtime

use std::future::Future;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use crate::RuntimeResult;

#[allow(unused_imports)]
use super::pool::{PoolConfig, WorkerPool};
use super::scheduler::Scheduler;
use super::task::{TaskHandle, TaskId, TaskInner, TaskMetadata, TaskPriority};

#[derive(Default)]
struct ExecutorMetrics {
    active_tasks: AtomicU64,
    queued_tasks: AtomicU64,
    completed_tasks: AtomicU64,
}

/// Handle to the executor
pub type ExecutorHandle = Arc<Executor>;

/// Task executor
pub struct Executor {
    pool: Arc<WorkerPool>,
    scheduler: Arc<Scheduler>,
    active_tasks: AtomicU64,
    queued_tasks: AtomicU64,
    completed_tasks: AtomicU64,
    next_worker: AtomicUsize,
}

impl Executor {
    /// Create a new executor
    pub fn new(pool: Arc<WorkerPool>, scheduler: Arc<Scheduler>) -> RuntimeResult<Self> {
        Ok(Self {
            pool,
            scheduler,
            active_tasks: AtomicU64::new(0),
            queued_tasks: AtomicU64::new(0),
            completed_tasks: AtomicU64::new(0),
            next_worker: AtomicUsize::new(0),
        })
    }

    /// Spawn a new task
    pub fn spawn<F>(&self, future: F) -> TaskHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.spawn_with_priority(future, TaskPriority::Normal)
    }

    /// Spawn a task with a specific priority
    pub fn spawn_with_priority<F>(&self, future: F, priority: TaskPriority) -> TaskHandle
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let task_id = TaskId::new();
        let inner = Arc::new(TaskInner::new(task_id, priority));

        // Spawn using tokio's runtime
        let join_handle = tokio::spawn(future);

        // Register with the scheduler
        let metadata = TaskMetadata::new(task_id, priority);
        let _ = self.scheduler.register_task(metadata);

        // Track in the worker pool queue
        let worker_idx = self.next_worker.fetch_add(1, Ordering::Relaxed);
        let queue = self.pool.get_queue(worker_idx);
        queue.push(task_id, priority);

        self.queued_tasks.fetch_add(1, Ordering::Relaxed);
        self.active_tasks.fetch_add(1, Ordering::Relaxed);

        TaskHandle::new(task_id, inner, join_handle)
    }

    /// Block on a future until completion
    pub fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future,
    {
        tokio::runtime::Handle::current().block_on(future)
    }

    /// Get the number of active tasks
    pub fn active_task_count(&self) -> usize {
        self.active_tasks.load(Ordering::Relaxed) as usize
    }

    /// Get the number of queued tasks
    pub fn queued_task_count(&self) -> usize {
        self.queued_tasks.load(Ordering::Relaxed) as usize
    }

    /// Get the number of completed tasks
    pub fn completed_task_count(&self) -> u64 {
        self.completed_tasks.load(Ordering::Relaxed)
    }

    /// Shutdown the executor
    pub fn shutdown(&self) -> RuntimeResult<()> {
        self.pool.shutdown()?;
        Ok(())
    }
}

impl std::fmt::Debug for Executor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Executor")
            .field("active_tasks", &self.active_task_count())
            .field("queued_tasks", &self.queued_task_count())
            .field("completed_tasks", &self.completed_task_count())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::super::scheduler::SchedulerConfig;
    use super::*;

    #[tokio::test]
    async fn test_executor_spawn() {
        let pool_config = PoolConfig {
            worker_count: 2,
            ..Default::default()
        };
        let pool = Arc::new(WorkerPool::new(pool_config).unwrap());
        let scheduler = Arc::new(Scheduler::new(SchedulerConfig::default()).unwrap());
        let executor = Executor::new(pool, scheduler).unwrap();

        let _handle = executor.spawn(async {
            // Simple async task
        });

        assert!(executor.active_task_count() > 0);
    }

    #[tokio::test]
    async fn test_executor_shutdown() {
        let pool_config = PoolConfig {
            worker_count: 2,
            ..Default::default()
        };
        let pool = Arc::new(WorkerPool::new(pool_config).unwrap());
        let scheduler = Arc::new(Scheduler::new(SchedulerConfig::default()).unwrap());
        let executor = Executor::new(pool, scheduler).unwrap();

        assert!(executor.shutdown().is_ok());
    }
}
