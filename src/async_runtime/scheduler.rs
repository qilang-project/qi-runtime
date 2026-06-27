//! Coroutine scheduler for the async runtime

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use crate::{RuntimeError, RuntimeResult};

use super::task::{TaskId, TaskMetadata};

/// Scheduler configuration
#[derive(Debug, Clone)]
pub struct SchedulerConfig {
    /// Maximum stack size for coroutines
    pub max_stack_size: usize,
    /// Number of pre-allocated stacks in the pool
    pub stack_pool_size: usize,
    /// Task polling interval
    pub poll_interval: Duration,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            max_stack_size: 2 * 1024 * 1024, // 2MB
            stack_pool_size: 128,
            poll_interval: Duration::from_millis(1),
        }
    }
}

/// Coroutine scheduler
pub struct Scheduler {
    config: SchedulerConfig,
    tasks: Mutex<HashMap<TaskId, TaskMetadata>>,
    scheduled_count: AtomicU64,
    completed_count: AtomicU64,
}

impl Scheduler {
    /// Create a new scheduler
    pub fn new(config: SchedulerConfig) -> RuntimeResult<Self> {
        Ok(Self {
            config,
            tasks: Mutex::new(HashMap::new()),
            scheduled_count: AtomicU64::new(0),
            completed_count: AtomicU64::new(0),
        })
    }

    /// Register a task with the scheduler
    pub fn register_task(&self, metadata: TaskMetadata) -> RuntimeResult<()> {
        let mut tasks = self.tasks.lock().map_err(|_| {
            RuntimeError::lock_error("无法获取任务锁".to_string(), "锁错误".to_string())
        })?;

        tasks.insert(metadata.id, metadata);
        self.scheduled_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Unregister a task from the scheduler
    pub fn unregister_task(&self, task_id: TaskId) -> RuntimeResult<()> {
        let mut tasks = self.tasks.lock().map_err(|_| {
            RuntimeError::lock_error("无法获取任务锁".to_string(), "锁错误".to_string())
        })?;

        tasks.remove(&task_id);
        self.completed_count.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Get a task by ID
    pub fn get_task(&self, task_id: TaskId) -> Option<TaskMetadata> {
        self.tasks.lock().ok()?.get(&task_id).cloned()
    }

    /// Get the number of currently scheduled tasks
    pub fn active_task_count(&self) -> usize {
        self.tasks.lock().map(|t| t.len()).unwrap_or(0)
    }

    /// Get the total number of scheduled tasks
    pub fn total_scheduled(&self) -> u64 {
        self.scheduled_count.load(Ordering::Relaxed)
    }

    /// Get the total number of completed tasks
    pub fn total_completed(&self) -> u64 {
        self.completed_count.load(Ordering::Relaxed)
    }

    /// Get the poll interval
    pub fn poll_interval(&self) -> Duration {
        self.config.poll_interval
    }
}

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler")
            .field("config", &self.config)
            .field("active_tasks", &self.active_task_count())
            .field("total_scheduled", &self.total_scheduled())
            .field("total_completed", &self.total_completed())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::super::task::{TaskId, TaskMetadata, TaskPriority};
    use super::*;

    #[tokio::test]
    async fn test_scheduler_creation() {
        let config = SchedulerConfig::default();
        let scheduler = Scheduler::new(config);
        assert!(scheduler.is_ok());
    }

    #[tokio::test]
    async fn test_scheduler_task_registration() {
        let scheduler = Scheduler::new(SchedulerConfig::default()).unwrap();
        let task_id = TaskId::new();
        let metadata = TaskMetadata::new(task_id, TaskPriority::Normal);

        assert!(scheduler.register_task(metadata).is_ok());
        assert_eq!(scheduler.active_task_count(), 1);

        assert!(scheduler.unregister_task(task_id).is_ok());
        assert_eq!(scheduler.active_task_count(), 0);
    }
}
