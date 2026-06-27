//! Async Task Abstraction
//!
//! This module defines the core task types and their lifecycle management.

use std::fmt;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::task::JoinHandle;

use crate::{RuntimeError, RuntimeResult};

static TASK_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Unique task identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(u64);

impl TaskId {
    /// Generate a new unique task ID
    pub fn new() -> Self {
        Self(TASK_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }

    /// Get the raw ID value
    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl Default for TaskId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for TaskId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Task({})", self.0)
    }
}

/// Task priority levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TaskPriority {
    /// Low priority background task
    Low = 0,
    /// Normal priority task (default)
    Normal = 1,
    /// High priority task
    High = 2,
    /// Critical priority task
    Critical = 3,
}

impl Default for TaskPriority {
    fn default() -> Self {
        TaskPriority::Normal
    }
}

/// Task execution status
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskStatus {
    Pending,
    Running,
    Waiting,
    Completed,
    Cancelled,
    Failed,
}

/// Internal task state
pub(crate) struct TaskInner {
    id: TaskId,
    priority: TaskPriority,
    status: AtomicUsize,
}

impl TaskInner {
    pub fn new(id: TaskId, priority: TaskPriority) -> Self {
        Self {
            id,
            priority,
            status: AtomicUsize::new(TaskStatus::Pending as usize),
        }
    }

    pub fn id(&self) -> TaskId {
        self.id
    }

    pub fn priority(&self) -> TaskPriority {
        self.priority
    }

    pub fn status(&self) -> TaskStatus {
        match self.status.load(Ordering::Acquire) {
            0 => TaskStatus::Pending,
            1 => TaskStatus::Running,
            2 => TaskStatus::Waiting,
            3 => TaskStatus::Completed,
            4 => TaskStatus::Cancelled,
            5 => TaskStatus::Failed,
            _ => TaskStatus::Pending,
        }
    }

    pub fn set_status(&self, status: TaskStatus) {
        self.status.store(status as usize, Ordering::Release);
    }
}

/// Handle to a spawned task
pub struct TaskHandle {
    id: TaskId,
    inner: Arc<TaskInner>,
    join_handle: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl TaskHandle {
    pub(crate) fn new(id: TaskId, inner: Arc<TaskInner>, join_handle: JoinHandle<()>) -> Self {
        Self {
            id,
            inner,
            join_handle: Arc::new(Mutex::new(Some(join_handle))),
        }
    }

    /// Get the task ID
    pub fn id(&self) -> TaskId {
        self.id
    }

    /// Get the current task status
    pub fn status(&self) -> TaskStatus {
        self.inner.status()
    }

    /// Cancel the task (best-effort)
    pub fn cancel(&self) -> RuntimeResult<()> {
        if let Ok(mut guard) = self.join_handle.lock() {
            if let Some(handle) = guard.take() {
                handle.abort();
            }
        }
        self.inner.set_status(TaskStatus::Cancelled);
        Ok(())
    }

    /// Wait for the task to complete
    pub async fn join(&self) -> RuntimeResult<()> {
        let join_handle = {
            let mut guard = self.join_handle.lock().map_err(|_| {
                RuntimeError::lock_error("任务等待锁失败".to_string(), "锁错误".to_string())
            })?;
            guard.take()
        };

        if let Some(handle) = join_handle {
            match handle.await {
                Ok(()) => {
                    self.inner.set_status(TaskStatus::Completed);
                    Ok(())
                }
                Err(err) => {
                    self.inner.set_status(TaskStatus::Failed);
                    Err(RuntimeError::task_error(
                        format!("任务执行失败: {}", err),
                        "任务执行失败".to_string(),
                    ))
                }
            }
        } else {
            // Already awaited; return current status
            match self.inner.status() {
                TaskStatus::Completed => Ok(()),
                TaskStatus::Cancelled => Err(RuntimeError::task_error(
                    "任务已取消".to_string(),
                    "任务取消".to_string(),
                )),
                TaskStatus::Failed => Err(RuntimeError::task_error(
                    "任务执行失败".to_string(),
                    "任务失败".to_string(),
                )),
                _ => Ok(()),
            }
        }
    }
}

impl Clone for TaskHandle {
    fn clone(&self) -> Self {
        Self {
            id: self.id,
            inner: Arc::clone(&self.inner),
            join_handle: Arc::clone(&self.join_handle),
        }
    }
}

impl fmt::Debug for TaskHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskHandle")
            .field("id", &self.id)
            .field("status", &self.status())
            .finish()
    }
}

/// Task metadata used by the scheduler
#[derive(Debug, Clone)]
pub struct TaskMetadata {
    pub id: TaskId,
    pub priority: TaskPriority,
}

impl TaskMetadata {
    pub fn new(id: TaskId, priority: TaskPriority) -> Self {
        Self { id, priority }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_task_id_generation() {
        let id1 = TaskId::new();
        let id2 = TaskId::new();
        assert_ne!(id1, id2);
        assert!(id2.as_u64() > id1.as_u64());
    }

    #[test]
    fn test_task_priority() {
        assert!(TaskPriority::Critical > TaskPriority::High);
        assert!(TaskPriority::High > TaskPriority::Normal);
        assert!(TaskPriority::Normal > TaskPriority::Low);
    }

    #[test]
    fn test_task_status_transitions() {
        let inner = TaskInner::new(TaskId::new(), TaskPriority::Normal);
        assert_eq!(inner.status(), TaskStatus::Pending);
        inner.set_status(TaskStatus::Running);
        assert_eq!(inner.status(), TaskStatus::Running);
    }
}
