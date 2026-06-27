//! Async runtime state management

use std::sync::atomic::{AtomicU8, Ordering};

/// Async runtime state
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsyncState {
    /// Runtime is initialized and idle
    Idle = 0,
    /// Runtime is actively executing tasks
    Running = 1,
    /// Runtime is shutting down
    ShuttingDown = 2,
    /// Runtime is fully stopped
    Stopped = 3,
}

impl Default for AsyncState {
    fn default() -> Self {
        AsyncState::Idle
    }
}

/// State manager for the async runtime
#[derive(Debug)]
pub struct StateManager {
    state: AtomicU8,
}

impl StateManager {
    /// Create a new state manager
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(AsyncState::Idle as u8),
        }
    }

    /// Get the current state
    pub fn state(&self) -> AsyncState {
        match self.state.load(Ordering::Acquire) {
            0 => AsyncState::Idle,
            1 => AsyncState::Running,
            2 => AsyncState::ShuttingDown,
            3 => AsyncState::Stopped,
            _ => AsyncState::Idle,
        }
    }

    /// Transition to a new state
    pub fn transition(&self, new_state: AsyncState) {
        self.state.store(new_state as u8, Ordering::Release);
    }

    /// Check if the runtime is running
    pub fn is_running(&self) -> bool {
        self.state() == AsyncState::Running
    }

    /// Check if runtime is shutting down
    pub fn is_shutting_down(&self) -> bool {
        self.state() == AsyncState::ShuttingDown
    }

    /// Reset the state to idle
    pub fn reset(&self) {
        self.transition(AsyncState::Idle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_transitions() {
        let state = StateManager::new();
        assert_eq!(state.state(), AsyncState::Idle);

        state.transition(AsyncState::Running);
        assert!(state.is_running());
        assert_eq!(state.state(), AsyncState::Running);

        state.transition(AsyncState::ShuttingDown);
        assert!(state.is_shutting_down());

        state.transition(AsyncState::Stopped);
        assert_eq!(state.state(), AsyncState::Stopped);

        state.reset();
        assert_eq!(state.state(), AsyncState::Idle);
    }
}
