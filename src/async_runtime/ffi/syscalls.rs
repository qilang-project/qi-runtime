//! FFI bindings and platform-specific event loop implementations

use std::collections::{HashMap, HashSet};
use std::os::raw::{c_int, c_longlong};

use crate::RuntimeError;

/// Result type for syscalls
pub type SyscallResult<T> = Result<T, RuntimeError>;

/// Event type flags
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EventType {
    Readable,
    Writable,
    Error,
    Custom(u32),
}

/// Event information returned from the event loop
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpollEvent {
    pub fd: i32,
    pub event_type: EventType,
}

extern "C" {
    fn qi_async_sys_sleep_ms(ms: c_int) -> c_int;
    fn qi_async_sys_monotonic_time_ns() -> c_longlong;
}

/// Sleep for the specified milliseconds using the C syscall wrapper
pub fn sleep_ms(ms: i32) -> SyscallResult<()> {
    let result = unsafe { qi_async_sys_sleep_ms(ms as c_int) };
    if result == 0 {
        Ok(())
    } else {
        Err(RuntimeError::system_error(
            format!("系统睡眠调用失败: {}", result),
            "系统调用失败".to_string(),
        ))
    }
}

/// Get monotonic time from the C syscall wrapper
pub fn monotonic_time_ns() -> SyscallResult<i64> {
    let value = unsafe { qi_async_sys_monotonic_time_ns() };
    if value >= 0 {
        Ok(value as i64)
    } else {
        Err(RuntimeError::system_error(
            "获取单调时间失败".to_string(),
            "系统调用失败".to_string(),
        ))
    }
}

/// Generic event loop implementation used as a fallback
#[derive(Debug, Default)]
pub struct GenericEventLoop {
    registrations: HashMap<i32, HashSet<EventType>>,
}

impl GenericEventLoop {
    /// Create a new generic event loop
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a file descriptor with event types
    pub fn register_fd(&mut self, fd: i32, events: EventType) -> SyscallResult<()> {
        self.registrations
            .entry(fd)
            .or_insert_with(HashSet::new)
            .insert(events);
        Ok(())
    }

    /// Unregister a file descriptor
    pub fn unregister_fd(&mut self, fd: i32) -> SyscallResult<()> {
        self.registrations.remove(&fd);
        Ok(())
    }

    /// Wait for events (simulated using sleep)
    pub fn wait_events(&mut self, timeout_ms: i32) -> SyscallResult<Vec<EpollEvent>> {
        if timeout_ms > 0 {
            let _ = sleep_ms(timeout_ms);
        }

        let mut events = Vec::new();
        for (&fd, event_types) in &self.registrations {
            for event in event_types {
                events.push(EpollEvent {
                    fd,
                    event_type: *event,
                });
            }
        }
        Ok(events)
    }

    /// Shutdown the event loop
    pub fn shutdown(&mut self) -> SyscallResult<()> {
        self.registrations.clear();
        Ok(())
    }
}

/// Linux epoll implementation (delegates to generic for now)
#[cfg(target_os = "linux")]
pub struct LinuxEpoll {
    inner: GenericEventLoop,
}

#[cfg(target_os = "linux")]
impl LinuxEpoll {
    pub fn new() -> Self {
        Self {
            inner: GenericEventLoop::new(),
        }
    }

    pub fn initialize(&mut self) -> SyscallResult<()> {
        Ok(())
    }

    pub fn register_fd(&mut self, fd: i32, events: EventType) -> SyscallResult<()> {
        self.inner.register_fd(fd, events)
    }

    pub fn unregister_fd(&mut self, fd: i32) -> SyscallResult<()> {
        self.inner.unregister_fd(fd)
    }

    pub fn wait_events(&mut self, timeout_ms: i32) -> SyscallResult<Vec<EpollEvent>> {
        self.inner.wait_events(timeout_ms)
    }

    pub fn shutdown(&mut self) -> SyscallResult<()> {
        self.inner.shutdown()
    }
}

/// macOS kqueue implementation (delegates to generic)
#[cfg(target_os = "macos")]
pub struct MacOsKqueue {
    inner: GenericEventLoop,
}

#[cfg(target_os = "macos")]
impl MacOsKqueue {
    pub fn new() -> Self {
        Self {
            inner: GenericEventLoop::new(),
        }
    }

    pub fn initialize(&mut self) -> SyscallResult<()> {
        Ok(())
    }

    pub fn register_fd(&mut self, fd: i32, events: EventType) -> SyscallResult<()> {
        self.inner.register_fd(fd, events)
    }

    pub fn unregister_fd(&mut self, fd: i32) -> SyscallResult<()> {
        self.inner.unregister_fd(fd)
    }

    pub fn wait_events(&mut self, timeout_ms: i32) -> SyscallResult<Vec<EpollEvent>> {
        self.inner.wait_events(timeout_ms)
    }

    pub fn shutdown(&mut self) -> SyscallResult<()> {
        self.inner.shutdown()
    }
}

/// Windows IOCP implementation (delegates to generic)
#[cfg(target_os = "windows")]
pub struct WindowsIocp {
    inner: GenericEventLoop,
}

#[cfg(target_os = "windows")]
impl WindowsIocp {
    pub fn new() -> Self {
        Self {
            inner: GenericEventLoop::new(),
        }
    }

    pub fn initialize(&mut self) -> SyscallResult<()> {
        Ok(())
    }

    pub fn register_fd(&mut self, fd: i32, events: EventType) -> SyscallResult<()> {
        self.inner.register_fd(fd, events)
    }

    pub fn unregister_fd(&mut self, fd: i32) -> SyscallResult<()> {
        self.inner.unregister_fd(fd)
    }

    pub fn wait_events(&mut self, timeout_ms: i32) -> SyscallResult<Vec<EpollEvent>> {
        self.inner.wait_events(timeout_ms)
    }

    pub fn shutdown(&mut self) -> SyscallResult<()> {
        self.inner.shutdown()
    }
}
