//! Task queue implementation for the async runtime.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use super::task::{TaskId, TaskPriority};

/// Shared handle to the task queue
pub type QueueHandle = Arc<TaskQueue>;

/// A simple multi-producer, multi-consumer task queue.
#[derive(Debug)]
pub struct TaskQueue {
    inner: Mutex<VecDeque<(TaskId, TaskPriority)>>,
}

impl TaskQueue {
    /// Create a new empty queue
    pub fn new() -> QueueHandle {
        Arc::new(Self {
            inner: Mutex::new(VecDeque::new()),
        })
    }

    /// Push a task into the queue
    pub fn push(&self, id: TaskId, priority: TaskPriority) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.push_back((id, priority));
        }
    }

    /// Pop the next task from the queue
    pub fn pop(&self) -> Option<(TaskId, TaskPriority)> {
        self.inner
            .lock()
            .ok()
            .and_then(|mut guard| guard.pop_front())
    }

    /// Check if the queue is empty
    pub fn is_empty(&self) -> bool {
        self.inner
            .lock()
            .map(|guard| guard.is_empty())
            .unwrap_or(true)
    }

    /// Get the number of tasks in the queue
    pub fn len(&self) -> usize {
        self.inner.lock().map(|guard| guard.len()).unwrap_or(0)
    }

    /// Remove a task by ID
    pub fn remove(&self, id: TaskId) -> bool {
        if let Ok(mut guard) = self.inner.lock() {
            if let Some(position) = guard.iter().position(|(task_id, _)| *task_id == id) {
                guard.remove(position);
                return true;
            }
        }
        false
    }

    /// Clear all tasks
    pub fn clear(&self) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_operations() {
        let queue = TaskQueue::new();
        assert!(queue.is_empty());

        let id1 = TaskId::new();
        let id2 = TaskId::new();

        queue.push(id1, TaskPriority::Normal);
        queue.push(id2, TaskPriority::High);

        assert_eq!(queue.len(), 2);

        let first = queue.pop().unwrap();
        assert_eq!(first.0, id1);
        assert_eq!(first.1, TaskPriority::Normal);

        let second = queue.pop().unwrap();
        assert_eq!(second.0, id2);
        assert_eq!(second.1, TaskPriority::High);

        assert!(queue.is_empty());
    }
}
