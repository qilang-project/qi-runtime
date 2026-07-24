//! Tool execution control shared by runtime workers and C callers.
//!
//! Handles are positive 64-bit values containing a slot and generation. Releasing a
//! handle invalidates it immediately; reusing its slot produces a different handle.

use crate::stdlib::qi_str::rc_cstr_from_string;
use std::collections::VecDeque;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

const MAX_GENERATION: u32 = 0x7fff_ffff;

/// Result from waiting for a terminal or cancellation notification.
#[repr(i32)]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WaitResult {
    InvalidHandle = -1,
    TimedOut = 0,
    Finished = 1,
    Cancelled = 2,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FinishRecord {
    pub code: i64,
    pub result: String,
}

struct ControlData {
    cancel_reason: Option<String>,
    finish: Option<FinishRecord>,
    progress: VecDeque<String>,
    dropped_progress: u64,
}

struct ToolControl {
    deadline: Option<Instant>,
    progress_capacity: usize,
    cancelled: AtomicBool,
    finished: AtomicBool,
    data: Mutex<ControlData>,
    changed: Condvar,
}

impl ToolControl {
    fn new(deadline_after_ms: i64, progress_capacity: usize) -> Self {
        let deadline = (deadline_after_ms >= 0)
            .then(|| Instant::now() + Duration::from_millis(deadline_after_ms as u64));
        Self {
            deadline,
            progress_capacity,
            cancelled: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            data: Mutex::new(ControlData {
                cancel_reason: None,
                finish: None,
                progress: VecDeque::with_capacity(progress_capacity),
                dropped_progress: 0,
            }),
            changed: Condvar::new(),
        }
    }

    fn expire_if_needed(&self) {
        if self
            .deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.cancel("deadline_exceeded".to_string());
        }
    }

    fn cancel(&self, reason: String) -> bool {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if self
            .cancelled
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        data.cancel_reason = Some(reason);
        drop(data);
        self.changed.notify_all();
        true
    }

    fn is_cancelled(&self) -> bool {
        self.expire_if_needed();
        self.cancelled.load(Ordering::Acquire)
    }

    fn cancel_reason(&self) -> Option<String> {
        self.expire_if_needed();
        self.data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .cancel_reason
            .clone()
    }

    fn remaining_ms(&self) -> i64 {
        let Some(deadline) = self.deadline else {
            return -1;
        };
        let now = Instant::now();
        if now >= deadline {
            self.expire_if_needed();
            return 0;
        }
        deadline
            .saturating_duration_since(now)
            .as_millis()
            .min(i64::MAX as u128) as i64
    }

    fn push_progress(&self, progress: String) -> bool {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if self.progress_capacity == 0 {
            data.dropped_progress = data.dropped_progress.saturating_add(1);
            return false;
        }
        if data.progress.len() == self.progress_capacity {
            data.progress.pop_front();
            data.dropped_progress = data.dropped_progress.saturating_add(1);
        }
        data.progress.push_back(progress);
        drop(data);
        self.changed.notify_all();
        true
    }

    fn pop_progress(&self) -> Option<String> {
        self.data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .progress
            .pop_front()
    }

    fn dropped_progress(&self) -> u64 {
        self.data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .dropped_progress
    }

    fn finish(&self, code: i64, result: String) -> bool {
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());
        if self
            .finished
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return false;
        }
        data.finish = Some(FinishRecord { code, result });
        drop(data);
        self.changed.notify_all();
        true
    }

    fn finish_record(&self) -> Option<FinishRecord> {
        self.data
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .finish
            .clone()
    }

    fn wait(&self, timeout: Option<Duration>) -> WaitResult {
        self.expire_if_needed();
        let started = Instant::now();
        let mut data = self.data.lock().unwrap_or_else(|e| e.into_inner());

        loop {
            if data.finish.is_some() {
                return WaitResult::Finished;
            }
            if data.cancel_reason.is_some() {
                return WaitResult::Cancelled;
            }

            let caller_remaining = timeout.map(|limit| limit.saturating_sub(started.elapsed()));
            if caller_remaining.is_some_and(|remaining| remaining.is_zero()) {
                return WaitResult::TimedOut;
            }
            let deadline_remaining = self
                .deadline
                .map(|deadline| deadline.saturating_duration_since(Instant::now()));
            let wait_for = match (caller_remaining, deadline_remaining) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (Some(a), None) => Some(a),
                (None, Some(b)) => Some(b),
                (None, None) => None,
            };

            if let Some(duration) = wait_for {
                if duration.is_zero() {
                    drop(data);
                    self.expire_if_needed();
                    data = self.data.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                let (next, _) = self
                    .changed
                    .wait_timeout(data, duration)
                    .unwrap_or_else(|e| e.into_inner());
                data = next;
            } else {
                data = self.changed.wait(data).unwrap_or_else(|e| e.into_inner());
            }

            if self
                .deadline
                .is_some_and(|deadline| Instant::now() >= deadline)
                && data.cancel_reason.is_none()
            {
                drop(data);
                self.expire_if_needed();
                data = self.data.lock().unwrap_or_else(|e| e.into_inner());
            }
        }
    }
}

struct Slot {
    generation: u32,
    control: Option<Arc<ToolControl>>,
}

#[derive(Default)]
struct Registry {
    slots: Vec<Slot>,
    free: Vec<u32>,
}

fn registry() -> &'static Mutex<Registry> {
    static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(Registry::default()))
}

fn encode_handle(slot: u32, generation: u32) -> i64 {
    (((generation as u64) << 32) | (slot as u64 + 1)) as i64
}

fn decode_handle(handle: i64) -> Option<(usize, u32)> {
    if handle <= 0 {
        return None;
    }
    let raw = handle as u64;
    let generation = (raw >> 32) as u32;
    let slot = (raw as u32).checked_sub(1)? as usize;
    (generation != 0).then_some((slot, generation))
}

fn get_control(handle: i64) -> Option<Arc<ToolControl>> {
    let (slot, generation) = decode_handle(handle)?;
    let registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    let entry = registry.slots.get(slot)?;
    (entry.generation == generation)
        .then(|| entry.control.as_ref().cloned())
        .flatten()
}

fn create_control(deadline_after_ms: i64, progress_capacity: usize) -> i64 {
    let control = Arc::new(ToolControl::new(deadline_after_ms, progress_capacity));
    let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(slot) = registry.free.pop() {
        let entry = &mut registry.slots[slot as usize];
        entry.control = Some(control);
        encode_handle(slot, entry.generation)
    } else {
        let slot = registry.slots.len() as u32;
        registry.slots.push(Slot {
            generation: 1,
            control: Some(control),
        });
        encode_handle(slot, 1)
    }
}

fn release_control(handle: i64) -> bool {
    let Some((slot, generation)) = decode_handle(handle) else {
        return false;
    };
    let control = {
        let mut registry = registry().lock().unwrap_or_else(|e| e.into_inner());
        let Some(entry) = registry.slots.get_mut(slot) else {
            return false;
        };
        if entry.generation != generation || entry.control.is_none() {
            return false;
        }
        let control = entry.control.take().unwrap();
        entry.generation = if entry.generation == MAX_GENERATION {
            1
        } else {
            entry.generation + 1
        };
        registry.free.push(slot as u32);
        control
    };
    control.cancel("released".to_string());
    true
}

fn c_string(ptr: *const c_char) -> Option<String> {
    (!ptr.is_null()).then(|| unsafe { CStr::from_ptr(ptr).to_string_lossy().into_owned() })
}

/// ABI symbols:
/// - `qi_tool_control_create`, `qi_tool_control_release`
/// - `qi_tool_control_cancel`, `qi_tool_control_is_cancelled`, `qi_tool_control_cancel_reason`
/// - `qi_tool_control_remaining_ms`, `qi_tool_control_wait`
/// - `qi_tool_control_progress_push`, `qi_tool_control_progress_pop`,
///   `qi_tool_control_progress_dropped`
/// - `qi_tool_control_finish`, `qi_tool_control_is_finished`,
///   `qi_tool_control_finish_code`, `qi_tool_control_finish_result`
/// - `qi_tool_control_free_string`
#[no_mangle]
pub extern "C" fn qi_tool_control_create(deadline_after_ms: i64, progress_capacity: i64) -> i64 {
    create_control(deadline_after_ms, progress_capacity.max(0) as usize)
}

#[no_mangle]
pub extern "C" fn qi_tool_control_release(handle: i64) -> i32 {
    release_control(handle) as i32
}

/// Returns 1 if this call won cancellation, 0 if already cancelled, -1 for a stale handle.
#[no_mangle]
pub extern "C" fn qi_tool_control_cancel(handle: i64, reason: *const c_char) -> i32 {
    let Some(control) = get_control(handle) else {
        return -1;
    };
    control.cancel(c_string(reason).unwrap_or_default()) as i32
}

#[no_mangle]
pub extern "C" fn qi_tool_control_is_cancelled(handle: i64) -> i32 {
    get_control(handle)
        .map(|control| control.is_cancelled() as i32)
        .unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn qi_tool_control_cancel_reason(handle: i64) -> *mut c_char {
    match get_control(handle) {
        Some(control) => rc_cstr_from_string(control.cancel_reason().unwrap_or_default()),
        None => std::ptr::null_mut(),
    }
}

/// Returns -1 for no deadline and -2 for a stale handle.
#[no_mangle]
pub extern "C" fn qi_tool_control_remaining_ms(handle: i64) -> i64 {
    get_control(handle)
        .map(|control| control.remaining_ms())
        .unwrap_or(-2)
}

/// Adds progress, dropping the oldest queued item when full. Returns -1 for a stale handle.
#[no_mangle]
pub extern "C" fn qi_tool_control_progress_push(handle: i64, progress: *const c_char) -> i32 {
    let Some(control) = get_control(handle) else {
        return -1;
    };
    let Some(progress) = c_string(progress) else {
        return 0;
    };
    control.push_progress(progress) as i32
}

/// Returns null when the queue is empty or the handle is stale.
#[no_mangle]
pub extern "C" fn qi_tool_control_progress_pop(handle: i64) -> *mut c_char {
    get_control(handle)
        .and_then(|control| control.pop_progress())
        .map(rc_cstr_from_string)
        .unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "C" fn qi_tool_control_progress_dropped(handle: i64) -> i64 {
    get_control(handle)
        .map(|control| control.dropped_progress().min(i64::MAX as u64) as i64)
        .unwrap_or(-1)
}

/// Returns 1 if this call recorded the finish, 0 if another call won, -1 if stale.
#[no_mangle]
pub extern "C" fn qi_tool_control_finish(handle: i64, code: i64, result: *const c_char) -> i32 {
    let Some(control) = get_control(handle) else {
        return -1;
    };
    control.finish(code, c_string(result).unwrap_or_default()) as i32
}

#[no_mangle]
pub extern "C" fn qi_tool_control_is_finished(handle: i64) -> i32 {
    get_control(handle)
        .map(|control| control.finished.load(Ordering::Acquire) as i32)
        .unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn qi_tool_control_finish_code(handle: i64) -> i64 {
    get_control(handle)
        .and_then(|control| control.finish_record())
        .map(|finish| finish.code)
        .unwrap_or(i64::MIN)
}

#[no_mangle]
pub extern "C" fn qi_tool_control_finish_result(handle: i64) -> *mut c_char {
    match get_control(handle) {
        Some(control) => rc_cstr_from_string(
            control
                .finish_record()
                .map(|finish| finish.result)
                .unwrap_or_default(),
        ),
        None => std::ptr::null_mut(),
    }
}

/// Wait result: -1 stale handle, 0 timeout, 1 finished, 2 cancelled.
/// A negative timeout waits indefinitely, bounded by the control deadline if present.
#[no_mangle]
pub extern "C" fn qi_tool_control_wait(handle: i64, timeout_ms: i64) -> i32 {
    let Some(control) = get_control(handle) else {
        return WaitResult::InvalidHandle as i32;
    };
    let timeout = (timeout_ms >= 0).then(|| Duration::from_millis(timeout_ms as u64));
    control.wait(timeout) as i32
}

#[no_mangle]
pub extern "C" fn qi_tool_control_free_string(value: *mut c_char) {
    if !value.is_null() {
        crate::stdlib::qi_str::rc_cstr_release(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::sync::{Arc, Barrier};
    use std::thread;

    fn text(value: *mut c_char) -> String {
        assert!(!value.is_null());
        let result = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        qi_tool_control_free_string(value);
        result
    }

    #[test]
    fn cancellation_wakes_waiter_and_first_reason_wins() {
        let handle = qi_tool_control_create(-1, 4);
        let waiter = thread::spawn(move || qi_tool_control_wait(handle, 5_000));
        thread::sleep(Duration::from_millis(30));
        let first = CString::new("caller_cancelled").unwrap();
        let second = CString::new("later_reason").unwrap();
        assert_eq!(qi_tool_control_cancel(handle, first.as_ptr()), 1);
        assert_eq!(qi_tool_control_cancel(handle, second.as_ptr()), 0);
        assert_eq!(waiter.join().unwrap(), WaitResult::Cancelled as i32);
        assert_eq!(
            text(qi_tool_control_cancel_reason(handle)),
            "caller_cancelled"
        );
        assert_eq!(qi_tool_control_release(handle), 1);
    }

    #[test]
    fn progress_preserves_retained_order_and_counts_drops() {
        let handle = qi_tool_control_create(-1, 2);
        for value in ["one", "two", "three"] {
            let value = CString::new(value).unwrap();
            assert_eq!(qi_tool_control_progress_push(handle, value.as_ptr()), 1);
        }
        assert_eq!(qi_tool_control_progress_dropped(handle), 1);
        assert_eq!(text(qi_tool_control_progress_pop(handle)), "two");
        assert_eq!(text(qi_tool_control_progress_pop(handle)), "three");
        assert!(qi_tool_control_progress_pop(handle).is_null());
        assert_eq!(qi_tool_control_release(handle), 1);
    }

    #[test]
    fn finish_race_records_exactly_one_result() {
        let handle = qi_tool_control_create(-1, 1);
        let racers = 12;
        let barrier = Arc::new(Barrier::new(racers));
        let mut threads = Vec::new();
        for code in 0..racers as i64 {
            let barrier = barrier.clone();
            threads.push(thread::spawn(move || {
                let result = CString::new(format!("result-{code}")).unwrap();
                barrier.wait();
                (code, qi_tool_control_finish(handle, code, result.as_ptr()))
            }));
        }
        let outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
        assert_eq!(outcomes.iter().filter(|(_, won)| *won == 1).count(), 1);
        let winning_code = outcomes.iter().find(|(_, won)| *won == 1).unwrap().0;
        assert_eq!(qi_tool_control_finish_code(handle), winning_code);
        assert_eq!(
            text(qi_tool_control_finish_result(handle)),
            format!("result-{winning_code}")
        );
        assert_eq!(qi_tool_control_wait(handle, 0), WaitResult::Finished as i32);
        assert_eq!(qi_tool_control_release(handle), 1);
    }

    #[test]
    fn wait_times_out_without_busy_waiting() {
        let handle = qi_tool_control_create(-1, 1);
        let start = Instant::now();
        assert_eq!(
            qi_tool_control_wait(handle, 60),
            WaitResult::TimedOut as i32
        );
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_millis(45));
        assert!(elapsed < Duration::from_millis(500));
        assert_eq!(qi_tool_control_release(handle), 1);
    }

    #[test]
    fn deadline_cancels_and_wakes_waiter() {
        let handle = qi_tool_control_create(40, 1);
        assert_eq!(
            qi_tool_control_wait(handle, 1_000),
            WaitResult::Cancelled as i32
        );
        assert_eq!(
            text(qi_tool_control_cancel_reason(handle)),
            "deadline_exceeded"
        );
        assert_eq!(qi_tool_control_remaining_ms(handle), 0);
        assert_eq!(qi_tool_control_release(handle), 1);
    }

    #[test]
    fn released_handle_is_stale_and_reused_slot_gets_new_generation() {
        let stale = qi_tool_control_create(-1, 1);
        assert_eq!(qi_tool_control_release(stale), 1);
        assert_eq!(qi_tool_control_release(stale), 0);
        assert_eq!(qi_tool_control_is_cancelled(stale), -1);

        let fresh = qi_tool_control_create(-1, 1);
        assert_ne!(fresh, stale);
        let reason = CString::new("fresh").unwrap();
        assert_eq!(qi_tool_control_cancel(stale, reason.as_ptr()), -1);
        assert_eq!(qi_tool_control_is_cancelled(fresh), 0);
        assert_eq!(qi_tool_control_release(fresh), 1);
    }

    #[test]
    fn release_wakes_an_indefinite_waiter() {
        let handle = qi_tool_control_create(-1, 1);
        let waiter = thread::spawn(move || qi_tool_control_wait(handle, -1));
        thread::sleep(Duration::from_millis(30));
        assert_eq!(qi_tool_control_release(handle), 1);
        assert_eq!(waiter.join().unwrap(), WaitResult::Cancelled as i32);
    }
}
