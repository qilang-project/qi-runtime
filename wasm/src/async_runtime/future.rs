//! Future（wasm 版）—— 同步、一出生就完成
//!
//! 主运行时的 Future 靠 tokio Notify 在线程间唤醒。wasm 单线程、没有 tokio，而且
//! 所有会产出 Future 的原语（睡眠、IO）在 wasm 里都是同步的，所以这里的 Future
//! 在构造时就已经 Completed，`等待` 只是把值取出来。
//!
//! 结构体字段名与主运行时一致（`state` / `value` / `error` / `sm_wakers`），
//! 因为 `coro.rs` 是从主运行时原样编进来的，它直接读 `f.state`。
//!
//! C ABI 与主运行时逐个对齐：`qi_future_ready_*` / `qi_future_await_*` /
//! `qi_future_failed` / `qi_future_is_completed` / `qi_future_free` / `qi_string_free`。

use std::os::raw::{c_char, c_void};
use std::sync::{Arc, Mutex};

use crate::async_runtime::coro;

#[derive(Debug, Clone, PartialEq)]
pub enum FutureState {
    Pending,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub enum FutureValue {
    Integer(i64),
    Float(f64),
    Boolean(bool),
    String(String),
    Pointer(*mut u8),
    None,
}

unsafe impl Send for FutureValue {}

/// 状态机唤醒器（codegen 状态机模式用）。wasm 里没人会 pending，登记了也不会被叫到，
/// 保留只为让复用的源码编译通过。
#[derive(Clone)]
pub struct StateMachineWaker {
    pub poll_fn: extern "C" fn(*mut u8),
    pub frame: usize,
}

unsafe impl Send for StateMachineWaker {}
unsafe impl Sync for StateMachineWaker {}

#[repr(C)]
pub struct Future {
    pub state: Arc<Mutex<FutureState>>,
    pub value: Arc<Mutex<Option<FutureValue>>>,
    pub error: Arc<Mutex<Option<String>>>,
    pub sm_wakers: Arc<Mutex<Vec<StateMachineWaker>>>,
}

impl Future {
    pub fn pending() -> Self {
        Future {
            state: Arc::new(Mutex::new(FutureState::Pending)),
            value: Arc::new(Mutex::new(None)),
            error: Arc::new(Mutex::new(None)),
            sm_wakers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    pub fn complete(&self, value: FutureValue) {
        *self.value.lock().unwrap() = Some(value);
        *self.state.lock().unwrap() = FutureState::Completed;
        self.fire_wakers();
    }

    pub fn fail(&self, error: String) {
        *self.error.lock().unwrap() = Some(error);
        *self.state.lock().unwrap() = FutureState::Failed;
        self.fire_wakers();
    }

    pub fn register_sm_waker(&self, waker: StateMachineWaker) {
        // 已经完成的直接叫醒，跟主运行时一致
        if self.is_completed() {
            (waker.poll_fn)(waker.frame as *mut u8);
            return;
        }
        self.sm_wakers.lock().unwrap().push(waker);
    }

    fn fire_wakers(&self) {
        let wakers: Vec<StateMachineWaker> = std::mem::take(&mut *self.sm_wakers.lock().unwrap());
        for w in wakers {
            (w.poll_fn)(w.frame as *mut u8);
        }
    }

    fn ready(value: FutureValue) -> Self {
        let f = Future::pending();
        f.complete(value);
        f
    }

    pub fn ready_i64(value: i64) -> Self {
        Future::ready(FutureValue::Integer(value))
    }
    pub fn ready_f64(value: f64) -> Self {
        Future::ready(FutureValue::Float(value))
    }
    pub fn ready_bool(value: bool) -> Self {
        Future::ready(FutureValue::Boolean(value))
    }
    pub fn ready_string(value: String) -> Self {
        Future::ready(FutureValue::String(value))
    }
    pub fn ready_ptr(ptr: *mut u8) -> Self {
        Future::ready(FutureValue::Pointer(ptr))
    }
    pub fn failed(error: String) -> Self {
        let f = Future::pending();
        f.fail(error);
        f
    }

    pub fn is_completed(&self) -> bool {
        !matches!(*self.state.lock().unwrap(), FutureState::Pending)
    }

    /// 取值。wasm 里不存在真正的 Pending：若碰到，说明有生产者忘了 complete，
    /// 报错而不是死等（单线程死等就是死锁）。
    pub fn await_value(&self) -> Result<FutureValue, String> {
        let st = self.state.lock().unwrap().clone();
        match st {
            FutureState::Completed => Ok(self
                .value
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(FutureValue::None)),
            FutureState::Failed => Err(self
                .error
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "future failed".to_string())),
            FutureState::Pending => Err("wasm 运行时里的 Future 不该处于 Pending".to_string()),
        }
    }
}

fn boxed(f: Future) -> *mut Future {
    Box::into_raw(Box::new(f))
}

// ── 构造 ────────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn qi_future_ready_i64(value: i64) -> *mut Future {
    boxed(Future::ready_i64(value))
}

#[no_mangle]
pub extern "C" fn qi_future_ready_f64(value: f64) -> *mut Future {
    boxed(Future::ready_f64(value))
}

#[no_mangle]
pub extern "C" fn qi_future_ready_bool(value: i32) -> *mut Future {
    boxed(Future::ready_bool(value != 0))
}

#[no_mangle]
pub extern "C" fn qi_future_ready_string(str_ptr: *const u8, str_len: usize) -> *mut Future {
    let s = if str_ptr.is_null() {
        String::new()
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(str_ptr, str_len) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    boxed(Future::ready_string(s))
}

#[no_mangle]
pub extern "C" fn qi_future_ready_ptr(ptr: *mut u8) -> *mut Future {
    boxed(Future::ready_ptr(ptr))
}

#[no_mangle]
pub extern "C" fn qi_future_failed(error_ptr: *const u8, error_len: usize) -> *mut Future {
    let s = if error_ptr.is_null() {
        String::from("error")
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(error_ptr, error_len) };
        String::from_utf8_lossy(bytes).into_owned()
    };
    boxed(Future::failed(s))
}

// ── 等待 ────────────────────────────────────────────────────────────
//
// 指针可能是协程（QiCoro）也可能是 eager Future，跟主运行时一样先问 coro::is_coro。
// 失败的 Future 在主运行时会 qi_exc_throw；wasm 上就是打印并退出。

fn take(future: *mut Future) -> Result<FutureValue, String> {
    if future.is_null() {
        return Err("null future".to_string());
    }
    let f = unsafe { &*future };
    f.await_value()
}

fn raise(err: String) -> ! {
    let c = std::ffi::CString::new(err).unwrap_or_default();
    crate::stdlib::exception_ffi::qi_exc_throw(c.as_ptr())
}

#[no_mangle]
pub extern "C-unwind" fn qi_future_await_i64(future: *mut Future) -> i64 {
    if !future.is_null() && unsafe { coro::is_coro(future as *const c_void) } {
        return coro::qi_coro_await_i64(future as *mut _);
    }
    match take(future) {
        Ok(FutureValue::Integer(v)) => v,
        Ok(FutureValue::Boolean(b)) => b as i64,
        Ok(FutureValue::Float(x)) => x as i64,
        Ok(_) => 0,
        Err(e) => raise(e),
    }
}

#[no_mangle]
pub extern "C-unwind" fn qi_future_await_f64(future: *mut Future) -> f64 {
    if !future.is_null() && unsafe { coro::is_coro(future as *const c_void) } {
        return f64::from_bits(coro::qi_coro_await_i64(future as *mut _) as u64);
    }
    match take(future) {
        Ok(FutureValue::Float(v)) => v,
        Ok(FutureValue::Integer(i)) => i as f64,
        Ok(_) => 0.0,
        Err(e) => raise(e),
    }
}

#[no_mangle]
pub extern "C-unwind" fn qi_future_await_bool(future: *mut Future) -> i32 {
    if !future.is_null() && unsafe { coro::is_coro(future as *const c_void) } {
        return (coro::qi_coro_await_i64(future as *mut _) != 0) as i32;
    }
    match take(future) {
        Ok(FutureValue::Boolean(b)) => b as i32,
        Ok(FutureValue::Integer(i)) => (i != 0) as i32,
        Ok(_) => 0,
        Err(e) => raise(e),
    }
}

#[no_mangle]
pub extern "C-unwind" fn qi_future_await_string(future: *mut Future) -> *const c_char {
    if !future.is_null() && unsafe { coro::is_coro(future as *const c_void) } {
        return coro::qi_coro_take_ptr(future as *mut _) as *const c_char;
    }
    match take(future) {
        Ok(FutureValue::String(s)) => crate::stdlib::qi_str::rc_cstr_from_string(s),
        Ok(FutureValue::Pointer(p)) => p as *const c_char,
        Ok(_) => crate::stdlib::qi_str::rc_cstr_from_str(""),
        Err(e) => raise(e),
    }
}

#[no_mangle]
pub extern "C-unwind" fn qi_future_await_ptr(future: *mut Future) -> *mut u8 {
    if !future.is_null() && unsafe { coro::is_coro(future as *const c_void) } {
        return coro::qi_coro_take_ptr(future as *mut _) as *mut u8;
    }
    match take(future) {
        Ok(FutureValue::Pointer(p)) => p,
        Ok(FutureValue::String(s)) => crate::stdlib::qi_str::rc_cstr_from_string(s) as *mut u8,
        Ok(_) => std::ptr::null_mut(),
        Err(e) => raise(e),
    }
}

/// 释放字符串函数返回的 RC 串（主运行时把这个符号放在 future.rs 里，位置照搬）
#[no_mangle]
pub extern "C" fn qi_string_free(str_ptr: *mut c_char) {
    if !str_ptr.is_null() {
        crate::stdlib::qi_str::rc_cstr_release(str_ptr);
    }
}

#[no_mangle]
pub extern "C" fn qi_future_is_completed(future: *mut Future) -> i32 {
    if future.is_null() {
        return 0;
    }
    unsafe { (*future).is_completed() as i32 }
}

#[no_mangle]
pub extern "C" fn qi_future_free(future: *mut Future) {
    if !future.is_null() {
        unsafe {
            drop(Box::from_raw(future));
        }
    }
}
