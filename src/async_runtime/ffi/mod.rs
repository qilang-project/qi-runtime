//! FFI bindings for C syscalls and async runtime interface
//!
//! This module provides the interface to low-level C syscalls for platform-specific
//! async I/O operations, as well as C interface functions for the async runtime.

use std::collections::HashMap;
use std::ffi::c_void;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::task::{Context, Poll};

pub mod syscalls;

pub use syscalls::{EpollEvent, EventType, SyscallResult};

/// Task handle for C interface
pub type TaskHandle = *mut c_void;

/// Simple task storage for C interface
static TASK_STORE: OnceLock<
    Mutex<HashMap<u64, Pin<Box<dyn Future<Output = *mut c_void> + Send + 'static>>>>,
> = OnceLock::new();
static mut NEXT_TASK_ID: u64 = 1;
static RUNTIME_INIT: OnceLock<()> = OnceLock::new();

/// 全局 tokio runtime —— 所有 `启动` 派发的 goroutine 都跑在这里。
/// async-aware IO（比如 时间.异步睡眠毫秒）能在 task 之间真 yield；
/// sync IO 仍会 pin 一个 worker（这是 qi 同步代码的固有限制，要语言级 async/await 才能解开）。
pub(crate) static 全局TOKIO: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

pub(crate) fn 全局异步运行时() -> &'static tokio::runtime::Runtime {
    全局TOKIO.get_or_init(|| {
        let workers = std::env::var("QI_ASYNC_WORKERS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or_else(|| num_cpus::get());
        let max_blocking = std::env::var("QI_ASYNC_MAX_BLOCKING")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(2048);
        if debug_enabled() {
            eprintln!(
                "DEBUG: tokio runtime: {} workers, {} max blocking",
                workers, max_blocking
            );
        }
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(workers)
            .max_blocking_threads(max_blocking)
            .thread_name("qi-tokio")
            .build()
            .expect("failed to start tokio runtime")
    })
}

/// Check if debug mode is enabled
fn debug_enabled() -> bool {
    std::env::var("QI_DEBUG").is_ok() || std::env::var("QI_DEBUG_RUNTIME").is_ok()
}

/// Initialize the async runtime for C interface
fn ensure_runtime_initialized() {
    RUNTIME_INIT.get_or_init(|| {
        // Initialize the task store
        let _ = TASK_STORE.set(Mutex::new(HashMap::new()));
        // Initialize the channel registry
        let _ = CHANNEL_REGISTRY.set(Mutex::new(HashMap::new()));
        // Initialize the timer registry
        let _ = TIMER_REGISTRY.set(Mutex::new(HashMap::new()));
    });
    // 预热全局 tokio runtime，避免第一个 启动 付 runtime-build 成本
    let _ = 全局异步运行时();
}

/// Create a new async task
#[no_mangle]
pub extern "C" fn qi_runtime_create_task(
    function_ptr: *const c_void,
    arg_count: i64,
) -> TaskHandle {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!("DEBUG: create_task called");
    }

    let task_id = unsafe {
        let id = NEXT_TASK_ID;
        NEXT_TASK_ID += 1;
        id
    };
    if debug_enabled() {
        eprintln!("DEBUG: task_id = {}", task_id);
    }

    // Convert function_ptr to usize to make it Send
    let function_addr = function_ptr as usize;

    // Create a future that calls the async function and returns its result
    let future = async move {
        if debug_enabled() {
            eprintln!("DEBUG: Inside async block");
        }
        // Call the async function and return its result
        unsafe {
            let func =
                std::mem::transmute::<usize, extern "C" fn() -> *const c_void>(function_addr);
            if debug_enabled() {
                eprintln!("DEBUG: About to call function");
            }
            let result = func();
            if debug_enabled() {
                eprintln!("DEBUG: Function returned {:?}", result);
            }

            // Allocate memory to store the result pointer so caller can load from it
            let result_ptr = Box::into_raw(Box::new(result)) as *mut c_void;
            if debug_enabled() {
                eprintln!("DEBUG: Returning result_ptr {:?}", result_ptr);
            }
            result_ptr
        }
    };

    // Store the future
    if let Some(store) = TASK_STORE.get() {
        if let Ok(mut store_guard) = store.lock() {
            store_guard.insert(task_id, Box::pin(future));
            if debug_enabled() {
                eprintln!("DEBUG: Future stored with task_id {}", task_id);
            }
        }
    }

    if debug_enabled() {
        eprintln!("DEBUG: create_task returning {}", task_id);
    }
    task_id as TaskHandle
}

/// Await the completion of an async task
#[no_mangle]
pub extern "C" fn qi_runtime_await(task: TaskHandle) -> *mut c_void {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!("DEBUG: await called with task {:?}", task);
    }

    let task_id = task as u64;

    // Try to get the future from the store
    if let Some(store) = TASK_STORE.get() {
        if debug_enabled() {
            eprintln!("DEBUG: Got task store");
        }
        if let Ok(mut store_guard) = store.lock() {
            if debug_enabled() {
                eprintln!("DEBUG: Locked store, contains {} tasks", store_guard.len());
            }
            if let Some(mut future) = store_guard.remove(&task_id) {
                if debug_enabled() {
                    eprintln!("DEBUG: Found future for task {}", task_id);
                }
                // Create a waker that does nothing - our futures are immediately ready
                // since they just wrap synchronous function calls
                use std::task::{RawWaker, RawWakerVTable, Waker};

                unsafe fn noop_clone(_: *const ()) -> RawWaker {
                    noop_raw_waker()
                }
                unsafe fn noop_wake(_: *const ()) {}
                unsafe fn noop_wake_by_ref(_: *const ()) {}
                unsafe fn noop_drop(_: *const ()) {}

                const NOOP_WAKER_VTABLE: RawWakerVTable =
                    RawWakerVTable::new(noop_clone, noop_wake, noop_wake_by_ref, noop_drop);

                fn noop_raw_waker() -> RawWaker {
                    RawWaker::new(std::ptr::null(), &NOOP_WAKER_VTABLE)
                }

                let waker = unsafe { Waker::from_raw(noop_raw_waker()) };
                let mut context = Context::from_waker(&waker);

                // Poll the future - it should be immediately ready since async {} blocks
                // without await points complete on first poll
                if debug_enabled() {
                    eprintln!("DEBUG: About to poll future");
                }
                match Pin::new(&mut future).poll(&mut context) {
                    Poll::Ready(result) => {
                        if debug_enabled() {
                            eprintln!("DEBUG: Future is Ready, returning result");
                        }
                        return result;
                    }
                    Poll::Pending => {
                        // This shouldn't happen for our simple futures, but handle it
                        if debug_enabled() {
                            eprintln!("Warning: Future returned Pending unexpectedly");
                        }
                        return std::ptr::null_mut();
                    }
                }
            } else {
                if debug_enabled() {
                    eprintln!("DEBUG: No future found for task {}", task_id);
                }
            }
        } else {
            if debug_enabled() {
                eprintln!("DEBUG: Failed to lock store");
            }
        }
    } else {
        if debug_enabled() {
            eprintln!("DEBUG: No task store");
        }
    }

    if debug_enabled() {
        eprintln!("DEBUG: await returning null");
    }
    std::ptr::null_mut()
}

/// Spawn an async task to start execution
#[no_mangle]
pub extern "C" fn qi_runtime_spawn_task(task: TaskHandle) -> i32 {
    ensure_runtime_initialized();

    // For now, just return success
    // In a real implementation, this would add the task to the executor
    0
}

/// Spawn a goroutine on the global tokio runtime.
///
/// The wrapper is a sync extern "C" function — it runs synchronously inside an
/// async block on the tokio runtime. Tokio's scheduler picks which OS thread
/// executes it; multiple goroutines share the same `worker_threads` pool.
///
/// **Real M:N kicks in only for goroutines that call async-aware FFI** (like
/// `时间.异步睡眠毫秒`) — those .await yield to the scheduler. Sync goroutine
/// code that does blocking syscalls still pins a tokio worker for the duration.
#[no_mangle]
pub extern "C" fn qi_runtime_spawn_goroutine(function_ptr: *const c_void) {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!(
            "DEBUG: spawn_goroutine called with function pointer {:?}",
            function_ptr
        );
    }

    let func_addr = function_ptr as usize;

    // sync wrapper → spawn_blocking 走 tokio 的 blocking pool（默认上限可调）。
    // 这样 sync 的 goroutine body 不会 pin 主 worker pool；同时 blocking pool
    // 自己能伸缩到 max_blocking_threads。
    全局异步运行时().spawn_blocking(move || {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let func = std::mem::transmute::<usize, fn()>(func_addr);
            func();
        }));
        if let Err(payload) = result {
            let msg = panic_message(payload);
            eprintln!("[qi] goroutine panic: {}", msg);
        }
    });
}

/// Generic goroutine spawn with wrapper function — same model as above but
/// the wrapper takes a *const i64 args array.
#[no_mangle]
pub extern "C" fn qi_runtime_spawn_goroutine_with_args(
    wrapper_fn: *const c_void,
    args: *const i64,
    arg_count: i64,
) {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!(
            "DEBUG: spawn_goroutine_with_args wrapper {:?}, args {:?}, count {}",
            wrapper_fn, args, arg_count
        );
    }

    let wrapper_addr = wrapper_fn as usize;
    let count = if arg_count < 0 { 0 } else { arg_count as usize };
    let copied: Box<[i64]> = if count == 0 || args.is_null() {
        Vec::new().into_boxed_slice()
    } else {
        unsafe {
            let slice = std::slice::from_raw_parts(args, count);
            slice.to_vec().into_boxed_slice()
        }
    };

    全局异步运行时().spawn_blocking(move || {
        let copied = copied;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            let wrapper = std::mem::transmute::<usize, fn(*const i64)>(wrapper_addr);
            wrapper(copied.as_ptr());
        }));
        if let Err(payload) = result {
            let msg = panic_message(payload);
            eprintln!("[qi] goroutine panic: {}", msg);
        }
    });
}

fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".to_string()
    }
}

// Channel implementation
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Global channel registry
static CHANNEL_REGISTRY: OnceLock<Mutex<HashMap<u64, Arc<ChannelInstance>>>> = OnceLock::new();
static mut NEXT_CHANNEL_ID: u64 = 1;

/// Global timer registry
static TIMER_REGISTRY: OnceLock<Mutex<HashMap<u64, Arc<Mutex<TimerInstance>>>>> = OnceLock::new();
static mut NEXT_TIMER_ID: u64 = 1;

/// Channel instance for runtime
struct ChannelInstance {
    sender: Arc<Mutex<Sender<*mut c_void>>>,
    receiver: Arc<Mutex<Receiver<*mut c_void>>>,
    buffer_size: i32,
}

unsafe impl Send for ChannelInstance {}
unsafe impl Sync for ChannelInstance {}

/// Timer instance for timeout operations
struct TimerInstance {
    deadline_ms: i64, // Absolute deadline in milliseconds since UNIX_EPOCH
    stopped: bool,
}

unsafe impl Send for TimerInstance {}
unsafe impl Sync for TimerInstance {}

/// Create a new channel
/// buffer_size: Channel buffer size (i64 for compatibility with LLVM IR)
#[no_mangle]
pub extern "C" fn qi_runtime_create_channel(buffer_size: i64) -> *mut c_void {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!(
            "DEBUG: create_channel called with buffer_size {}",
            buffer_size
        );
    }

    let (sender, receiver) = mpsc::channel();
    let channel = Arc::new(ChannelInstance {
        sender: Arc::new(Mutex::new(sender)),
        receiver: Arc::new(Mutex::new(receiver)),
        buffer_size: buffer_size as i32,
    });

    let channel_id = unsafe {
        let id = NEXT_CHANNEL_ID;
        NEXT_CHANNEL_ID += 1;
        id
    };

    if let Some(registry) = CHANNEL_REGISTRY.get() {
        if let Ok(mut registry_guard) = registry.lock() {
            registry_guard.insert(channel_id, channel);
            if debug_enabled() {
                eprintln!("DEBUG: Created channel with ID {}", channel_id);
            }
            return channel_id as *mut c_void;
        }
    }

    std::ptr::null_mut()
}

/// Send a value to a channel (i64 value)
#[no_mangle]
pub extern "C" fn qi_runtime_channel_send(channel: *mut c_void, value: i64) -> i32 {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!(
            "DEBUG: channel_send called with channel {:?}, value {}",
            channel, value
        );
    }

    let channel_id = channel as u64;

    // Box the i64 value to send through the channel
    let value_ptr = Box::into_raw(Box::new(value)) as *mut c_void;

    // Clone the Arc out and drop the registry lock before touching the sender, so
    // a send never contends with a blocking recv() (which holds only the per-channel
    // receiver mutex, not the registry).
    let channel_instance = {
        let registry = match CHANNEL_REGISTRY.get() {
            Some(r) => r,
            None => {
                unsafe {
                    let _ = Box::from_raw(value_ptr as *mut i64);
                }
                return -1;
            }
        };
        let registry_guard = match registry.lock() {
            Ok(g) => g,
            Err(_) => {
                unsafe {
                    let _ = Box::from_raw(value_ptr as *mut i64);
                }
                return -1;
            }
        };
        match registry_guard.get(&channel_id) {
            Some(inst) => Arc::clone(inst),
            None => {
                if debug_enabled() {
                    eprintln!("DEBUG: Channel not found for ID {}", channel_id);
                }
                unsafe {
                    let _ = Box::from_raw(value_ptr as *mut i64);
                }
                return -1;
            }
        }
        // registry_guard dropped here
    };

    if let Ok(sender) = channel_instance.sender.lock() {
        if sender.send(value_ptr).is_err() {
            if debug_enabled() {
                eprintln!("DEBUG: Failed to send value to channel - channel might be closed");
            }
            // Clean up the boxed value on error
            unsafe {
                let _ = Box::from_raw(value_ptr as *mut i64);
            }
            return -1;
        }
        if debug_enabled() {
            eprintln!("DEBUG: Successfully sent value to channel");
        }
        return 0; // Success
    }
    unsafe {
        let _ = Box::from_raw(value_ptr as *mut i64);
    }
    -1 // Error
}

/// Receive a value from a channel (blocking)
/// result_ptr: Output parameter - will be filled with a pointer to the received value
#[no_mangle]
pub extern "C" fn qi_runtime_channel_receive(
    channel: *mut c_void,
    result_ptr: *mut *mut c_void,
) -> i32 {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!(
            "DEBUG: channel_receive called with channel {:?}, result_ptr {:?}",
            channel, result_ptr
        );
    }

    let channel_id = channel as u64;

    // IMPORTANT: clone the Arc<ChannelInstance> out of the registry and DROP the
    // registry lock BEFORE the blocking `recv()`. Holding the global
    // CHANNEL_REGISTRY mutex across a blocking recv() deadlocks: a goroutine doing
    // `qi_runtime_channel_send` must also lock the same registry to look up the
    // channel, so it could never deliver the value, and recv() would block forever.
    let channel_instance = {
        let registry = match CHANNEL_REGISTRY.get() {
            Some(r) => r,
            None => return -1,
        };
        let registry_guard = match registry.lock() {
            Ok(g) => g,
            Err(_) => return -1,
        };
        match registry_guard.get(&channel_id) {
            Some(inst) => Arc::clone(inst),
            None => {
                if debug_enabled() {
                    eprintln!("DEBUG: Channel not found for ID {}", channel_id);
                }
                return -1;
            }
        }
        // registry_guard dropped here
    };

    // Lock only the channel's own receiver mutex (independent of the sender mutex),
    // so a concurrent send can proceed and unblock this recv().
    let receiver = match channel_instance.receiver.lock() {
        Ok(r) => r,
        Err(_) => return -1,
    };
    match receiver.recv() {
        Ok(value_ptr) => {
            if debug_enabled() {
                eprintln!("DEBUG: Received value_ptr {:?} from channel", value_ptr);
            }
            // Write the received pointer to the output parameter
            unsafe {
                *result_ptr = value_ptr;
            }
            0 // Success
        }
        Err(_) => {
            if debug_enabled() {
                eprintln!("DEBUG: Failed to receive value from channel - channel might be closed");
            }
            -1 // Error
        }
    }
}

/// Try to receive a value from a channel WITHOUT blocking.
/// result_ptr: Output parameter - on success filled with a pointer to the received value.
/// Returns: 0 on success (a value was received), 1 if the channel is currently empty,
///          -1 on error (channel not found / disconnected).
#[no_mangle]
pub extern "C" fn qi_runtime_channel_try_receive(
    channel: *mut c_void,
    result_ptr: *mut *mut c_void,
) -> i32 {
    ensure_runtime_initialized();
    let channel_id = channel as u64;

    if let Some(registry) = CHANNEL_REGISTRY.get() {
        if let Ok(registry_guard) = registry.lock() {
            if let Some(channel_instance) = registry_guard.get(&channel_id) {
                if let Ok(receiver) = channel_instance.receiver.lock() {
                    match receiver.try_recv() {
                        Ok(value_ptr) => {
                            unsafe {
                                *result_ptr = value_ptr;
                            }
                            return 0; // got a value
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => {
                            return 1; // not ready, but not an error
                        }
                        Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            return -1; // closed
                        }
                    }
                }
            }
        }
    }
    -1
}

/// Try to send a value to a channel WITHOUT blocking.
/// (std::sync::mpsc is unbounded, so a send essentially never blocks; this mirrors
///  the try-receive ABI so select can treat sends uniformly.)
/// Returns: 0 on success, 1 if not ready (never, for unbounded), -1 on error.
#[no_mangle]
pub extern "C" fn qi_runtime_channel_try_send(channel: *mut c_void, value: i64) -> i32 {
    ensure_runtime_initialized();
    let channel_id = channel as u64;
    let value_ptr = Box::into_raw(Box::new(value)) as *mut c_void;

    if let Some(registry) = CHANNEL_REGISTRY.get() {
        if let Ok(registry_guard) = registry.lock() {
            if let Some(channel_instance) = registry_guard.get(&channel_id) {
                if let Ok(sender) = channel_instance.sender.lock() {
                    if sender.send(value_ptr).is_ok() {
                        return 0;
                    }
                    // closed: reclaim the box
                    unsafe {
                        let _ = Box::from_raw(value_ptr as *mut i64);
                    }
                    return -1;
                }
            }
        }
    }
    unsafe {
        let _ = Box::from_raw(value_ptr as *mut i64);
    }
    -1
}

/// Sleep for the select poll loop's back-off interval (~1ms), so a select that is
/// waiting on channels does not busy-spin. Kept as a tiny FFI helper so codegen's
/// poll loop stays branch-only.
#[no_mangle]
pub extern "C" fn qi_runtime_select_backoff() {
    std::thread::sleep(Duration::from_millis(1));
}

/// Select statement implementation — real non-blocking poll over a set of channel
/// cases with a bounded wait.
///
/// ABI of `select_cases`: a pointer to an array of `QiSelectCase` records:
///   { channel: *mut c_void, direction: i64 (0=recv,1=send), value: i64,
///     result_out: *mut *mut c_void }
/// followed by trailing metadata is NOT used here — codegen drives the actual
/// case-body dispatch in LLVM IR using `qi_runtime_channel_try_receive` /
/// `qi_runtime_channel_try_send` + `qi_runtime_select_backoff`. This entry point
/// is retained for the declared symbol and for callers that pass a single-case
/// array; it polls each case once (non-blocking) and returns the index of the
/// first ready case (as a tagged pointer) or null if none were ready.
#[no_mangle]
pub extern "C" fn qi_runtime_select(select_cases: *mut c_void) -> *mut c_void {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!("DEBUG: select called with cases {:?}", select_cases);
    }
    // No structured case array is passed by the current codegen (it drives the
    // poll loop inline in IR). Return null to indicate "no case taken here".
    std::ptr::null_mut()
}

// ===== Timeout Functions =====

/// Get current time in milliseconds since UNIX epoch
#[no_mangle]
pub extern "C" fn qi_runtime_get_time_ms() -> i64 {
    ensure_runtime_initialized();
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as i64,
        Err(_) => 0,
    }
}

/// Set a timeout (sleep for specified milliseconds)
/// timeout_ms: Duration to sleep in milliseconds
/// Returns: 0 on success, -1 on error
#[no_mangle]
pub extern "C" fn qi_runtime_set_timeout(timeout_ms: i64) -> i64 {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!("DEBUG: set_timeout called with timeout_ms {}", timeout_ms);
    }

    if timeout_ms < 0 {
        return -1;
    }

    std::thread::sleep(Duration::from_millis(timeout_ms as u64));
    0
}

/// Create a timer with a deadline in milliseconds from now
/// deadline_ms: Milliseconds from now when the timer should expire
/// Returns: Timer handle (pointer) or null on error
#[no_mangle]
pub extern "C" fn qi_runtime_timer_create(deadline_ms: i64) -> *mut c_void {
    ensure_runtime_initialized();
    if debug_enabled() {
        eprintln!(
            "DEBUG: timer_create called with deadline_ms {}",
            deadline_ms
        );
    }

    if deadline_ms < 0 {
        return std::ptr::null_mut();
    }

    // Calculate absolute deadline
    let current_time_ms = qi_runtime_get_time_ms();
    let absolute_deadline = current_time_ms + deadline_ms;

    let timer = Arc::new(Mutex::new(TimerInstance {
        deadline_ms: absolute_deadline,
        stopped: false,
    }));

    let timer_id = unsafe {
        let id = NEXT_TIMER_ID;
        NEXT_TIMER_ID += 1;
        id
    };

    if let Some(registry) = TIMER_REGISTRY.get() {
        if let Ok(mut registry_guard) = registry.lock() {
            registry_guard.insert(timer_id, timer);
            if debug_enabled() {
                eprintln!(
                    "DEBUG: Created timer with ID {}, absolute deadline {}",
                    timer_id, absolute_deadline
                );
            }
            return timer_id as *mut c_void;
        }
    }

    std::ptr::null_mut()
}

/// Check if a timer has expired
/// timer: Timer handle returned by qi_runtime_timer_create
/// Returns: 1 if expired, 0 if not expired, -1 on error
#[no_mangle]
pub extern "C" fn qi_runtime_timer_expired(timer: *mut c_void) -> i64 {
    ensure_runtime_initialized();
    if timer.is_null() {
        return -1;
    }

    let timer_id = timer as u64;

    if let Some(registry) = TIMER_REGISTRY.get() {
        if let Ok(registry_guard) = registry.lock() {
            if let Some(timer_instance) = registry_guard.get(&timer_id) {
                if let Ok(timer_guard) = timer_instance.lock() {
                    if timer_guard.stopped {
                        if debug_enabled() {
                            eprintln!("DEBUG: Timer {} has been stopped", timer_id);
                        }
                        return 1; // Treat stopped timers as expired
                    }

                    let current_time_ms = qi_runtime_get_time_ms();
                    let expired = current_time_ms >= timer_guard.deadline_ms;

                    if debug_enabled() {
                        eprintln!(
                            "DEBUG: Timer {} check: current={}, deadline={}, expired={}",
                            timer_id, current_time_ms, timer_guard.deadline_ms, expired
                        );
                    }

                    return if expired { 1 } else { 0 };
                }
            } else {
                if debug_enabled() {
                    eprintln!("DEBUG: Timer not found for ID {}", timer_id);
                }
            }
        }
    }

    -1 // Error
}

/// Stop/cancel a timer
/// timer: Timer handle returned by qi_runtime_timer_create
/// Returns: 0 on success, -1 on error
#[no_mangle]
pub extern "C" fn qi_runtime_timer_stop(timer: *mut c_void) -> i64 {
    ensure_runtime_initialized();
    if timer.is_null() {
        return -1;
    }

    if debug_enabled() {
        eprintln!("DEBUG: timer_stop called with timer {:?}", timer);
    }

    let timer_id = timer as u64;

    if let Some(registry) = TIMER_REGISTRY.get() {
        if let Ok(registry_guard) = registry.lock() {
            if let Some(timer_instance) = registry_guard.get(&timer_id) {
                if let Ok(mut timer_guard) = timer_instance.lock() {
                    timer_guard.stopped = true;
                    if debug_enabled() {
                        eprintln!("DEBUG: Timer {} stopped", timer_id);
                    }
                    return 0; // Success
                }
            } else {
                if debug_enabled() {
                    eprintln!("DEBUG: Timer not found for ID {}", timer_id);
                }
            }
        }
    }

    -1 // Error
}

/// Platform-specific I/O event loop interface
pub trait IoEventLoop {
    /// Initialize the event loop
    fn initialize(&mut self) -> SyscallResult<()>;

    /// Register a file descriptor for monitoring
    fn register_fd(&mut self, fd: i32, events: EventType) -> SyscallResult<()>;

    /// Unregister a file descriptor
    fn unregister_fd(&mut self, fd: i32) -> SyscallResult<()>;

    /// Wait for events with optional timeout (in milliseconds)
    fn wait_events(&mut self, timeout_ms: i32) -> SyscallResult<Vec<EpollEvent>>;

    /// Cleanup and shutdown the event loop
    fn shutdown(&mut self) -> SyscallResult<()>;
}

#[cfg(target_os = "linux")]
pub type PlatformEventLoop = syscalls::LinuxEpoll;

#[cfg(target_os = "macos")]
pub type PlatformEventLoop = syscalls::MacOsKqueue;

#[cfg(target_os = "windows")]
pub type PlatformEventLoop = syscalls::WindowsIocp;

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
pub type PlatformEventLoop = syscalls::GenericEventLoop;
