//! Future type implementation for Qi async runtime
//!
//! Provides Future<T> support for async operations

#[cfg(test)]
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::{Arc, Mutex};

/// Future state enumeration
#[repr(C)]
#[derive(Debug, Clone, PartialEq)]
pub enum FutureState {
    Pending,   // 等待中
    Completed, // 已完成
    Failed,    // 已失败
}

/// Value types that Future can hold
/// Future 可以持有的值类型
#[derive(Debug, Clone)]
pub enum FutureValue {
    Integer(i64),     // 整数
    Float(f64),       // 浮点数
    Boolean(bool),    // 布尔值
    String(String),   // 字符串
    Pointer(*mut u8), // 指针（用于结构体等）
    None,             // 无值
}

// SAFETY: FutureValue is Send-safe when used with String/Integer/Float/Boolean variants.
// The Pointer variant requires careful usage - only send across threads if the pointed data is thread-safe.
unsafe impl Send for FutureValue {}

/// State-machine waker — 由 codegen 生成的 poll fn + frame 指针组成。
/// 编译器异步状态机里程碑（docs/编译器异步状态机里程碑.md §3）需要此字段。
/// 当前 sync `等待` 路径不用，仅在状态机模式下注册。
#[derive(Clone)]
pub struct StateMachineWaker {
    pub poll_fn: extern "C" fn(*mut u8),
    pub frame: usize, // *mut u8 stored as usize for Send/Sync
}

unsafe impl Send for StateMachineWaker {}
unsafe impl Sync for StateMachineWaker {}

/// Future structure - heap allocated
/// 未来结构 - 堆分配
#[repr(C)]
pub struct Future {
    pub state: Arc<Mutex<FutureState>>,
    pub value: Arc<Mutex<Option<FutureValue>>>,
    pub error: Arc<Mutex<Option<String>>>,
    /// Notification primitive — completers call notify_waiters(), awaiters
    /// .notified().await. Replaces the old yield_now busy-wait.
    pub notify: Arc<tokio::sync::Notify>,
    /// State-machine wakers — pushed by qi_future_register_waker, fired on
    /// complete/fail. 仅 codegen 状态机路径使用。
    pub sm_wakers: Arc<Mutex<Vec<StateMachineWaker>>>,
}

impl Future {
    /// Internal — construct a Pending Future with empty value/error and a fresh Notify.
    /// 用于异步 IO 的入口：先返回 Pending，tokio task 完成后调 complete()。
    pub fn pending() -> Self {
        Future {
            state: Arc::new(Mutex::new(FutureState::Pending)),
            value: Arc::new(Mutex::new(None)),
            error: Arc::new(Mutex::new(None)),
            notify: Arc::new(tokio::sync::Notify::new()),
            sm_wakers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// 把 pending future 标记为完成，唤醒 awaiter
    pub fn complete(&self, value: FutureValue) {
        *self.value.lock().unwrap() = Some(value);
        *self.state.lock().unwrap() = FutureState::Completed;
        self.notify.notify_waiters();
        // 触发所有状态机 wakers — 调用方应该在 spawn / 异步上下文里 invoke poll
        let wakers = std::mem::take(&mut *self.sm_wakers.lock().unwrap());
        for w in wakers {
            (w.poll_fn)(w.frame as *mut u8);
        }
    }

    /// 标记失败，唤醒 awaiter
    pub fn fail(&self, error: String) {
        *self.error.lock().unwrap() = Some(error);
        *self.state.lock().unwrap() = FutureState::Failed;
        self.notify.notify_waiters();
        let wakers = std::mem::take(&mut *self.sm_wakers.lock().unwrap());
        for w in wakers {
            (w.poll_fn)(w.frame as *mut u8);
        }
    }

    /// State-machine waker 注册：pending 时 push，已完成则立即调用
    pub fn register_sm_waker(&self, waker: StateMachineWaker) {
        let st = self.state.lock().unwrap().clone();
        match st {
            FutureState::Completed | FutureState::Failed => {
                drop(st);
                (waker.poll_fn)(waker.frame as *mut u8);
            }
            FutureState::Pending => {
                drop(st);
                self.sm_wakers.lock().unwrap().push(waker);
            }
        }
    }

    /// 创建已完成的整数 future
    pub fn ready_i64(value: i64) -> Self {
        let f = Self::pending();
        f.complete(FutureValue::Integer(value));
        f
    }
    pub fn ready_f64(value: f64) -> Self {
        let f = Self::pending();
        f.complete(FutureValue::Float(value));
        f
    }
    pub fn ready_bool(value: bool) -> Self {
        let f = Self::pending();
        f.complete(FutureValue::Boolean(value));
        f
    }
    pub fn ready_string(value: String) -> Self {
        let f = Self::pending();
        f.complete(FutureValue::String(value));
        f
    }
    pub fn ready_ptr(ptr: *mut u8) -> Self {
        let f = Self::pending();
        f.complete(FutureValue::Pointer(ptr));
        f
    }
    pub fn failed(error: String) -> Self {
        let f = Self::pending();
        f.fail(error);
        f
    }

    pub fn is_completed(&self) -> bool {
        let state = self.state.lock().unwrap();
        *state == FutureState::Completed
    }

    /// Async-aware await — uses tokio::sync::Notify, no busy-wait
    async fn await_value_async(&self) -> Result<FutureValue, String> {
        loop {
            // 先订阅再检查 state，避免 race: complete() 可能在我们检查 state
            // 之后但订阅之前 fire，notified() 不能监听已经发生的事件。
            let notified = self.notify.notified();
            tokio::pin!(notified);
            // 启用订阅（内部是注册 waker）
            notified.as_mut().enable();
            // 然后检查 state
            {
                let state = self.state.lock().unwrap();
                match *state {
                    FutureState::Completed => {
                        return Ok(self
                            .value
                            .lock()
                            .unwrap()
                            .clone()
                            .unwrap_or(FutureValue::None));
                    }
                    FutureState::Failed => {
                        return Err(self
                            .error
                            .lock()
                            .unwrap()
                            .clone()
                            .unwrap_or_else(|| "Unknown error".to_string()));
                    }
                    FutureState::Pending => {}
                }
            }
            // 状态还是 Pending，等通知
            notified.await;
            // 醒来重新检查 state
        }
    }

    /// 同步入口 - bridge to async via runtime block_on。
    /// - 在 tokio task 上下文里：block_in_place + Handle::block_on（worker 暂时进入"阻塞 IO"模式）
    /// - 在普通 OS 线程：全局 runtime block_on
    pub fn await_value(&self) -> Result<FutureValue, String> {
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            tokio::task::block_in_place(|| handle.block_on(self.await_value_async()))
        } else {
            crate::async_runtime::ffi::全局异步运行时().block_on(self.await_value_async())
        }
    }

    /// 兼容老接口：保持 await_value 的签名给已有调用方，behavior 已切换
    #[allow(dead_code)]
    fn _legacy_busy_wait_await(&self) -> Result<FutureValue, String> {
        loop {
            let state = self.state.lock().unwrap();
            match *state {
                FutureState::Completed => {
                    drop(state);
                    let value = self.value.lock().unwrap();
                    return Ok(value.clone().unwrap_or(FutureValue::None));
                }
                FutureState::Failed => {
                    drop(state);
                    let error = self.error.lock().unwrap();
                    return Err(error.clone().unwrap_or_else(|| "Unknown error".to_string()));
                }
                FutureState::Pending => {
                    drop(state);
                    std::thread::yield_now();
                }
            }
        }
    }
}

// ===== FFI Functions for LLVM IR =====

/// failed future 被 `等待`：错误沿 Qi 异常机制传播（qi_exc_throw 不返回 ——
/// 有 `尝试` frame 时 longjmp 进 catch；goroutine 内无 frame 转 panic 进协程
/// 异常队列；主线程无 frame 打印后 abort）。
fn throw_future_error(err: String) -> ! {
    let c = std::ffi::CString::new(err).unwrap_or_default();
    crate::stdlib::exception_ffi::qi_exc_throw(c.as_ptr())
}

/// Create a ready future with an i64 value
/// FFI: qi_future_ready_i64(value: i64) -> *mut Future
#[no_mangle]
pub extern "C" fn qi_future_ready_i64(value: i64) -> *mut Future {
    let future = Box::new(Future::ready_i64(value));
    Box::into_raw(future)
}

/// Create a ready future with a f64 value
/// FFI: qi_future_ready_f64(value: f64) -> *mut Future
#[no_mangle]
pub extern "C" fn qi_future_ready_f64(value: f64) -> *mut Future {
    let future = Box::new(Future::ready_f64(value));
    Box::into_raw(future)
}

/// Create a ready future with a boolean value
/// FFI: qi_future_ready_bool(value: i32) -> *mut Future
/// Note: Use i32 for FFI compatibility (0 = false, non-zero = true)
#[no_mangle]
pub extern "C" fn qi_future_ready_bool(value: i32) -> *mut Future {
    let future = Box::new(Future::ready_bool(value != 0));
    Box::into_raw(future)
}

/// Create a ready future with a string value
/// FFI: qi_future_ready_string(str_ptr: *const u8, str_len: usize) -> *mut Future
#[no_mangle]
pub extern "C" fn qi_future_ready_string(str_ptr: *const u8, str_len: usize) -> *mut Future {
    let string_value = if str_ptr.is_null() {
        String::new()
    } else {
        unsafe {
            let slice = std::slice::from_raw_parts(str_ptr, str_len);
            String::from_utf8_lossy(slice).to_string()
        }
    };

    let future = Box::new(Future::ready_string(string_value));
    Box::into_raw(future)
}

/// Create a ready future with a pointer value (for structs, etc.)
/// FFI: qi_future_ready_ptr(ptr: *mut u8) -> *mut Future
#[no_mangle]
pub extern "C" fn qi_future_ready_ptr(ptr: *mut u8) -> *mut Future {
    let future = Box::new(Future::ready_ptr(ptr));
    Box::into_raw(future)
}

/// Create a failed future with an error message
/// FFI: qi_future_failed(error_ptr: *const u8, error_len: usize) -> *mut Future
#[no_mangle]
pub extern "C" fn qi_future_failed(error_ptr: *const u8, error_len: usize) -> *mut Future {
    let error_msg = if error_ptr.is_null() {
        "Unknown error".to_string()
    } else {
        unsafe {
            let slice = std::slice::from_raw_parts(error_ptr, error_len);
            String::from_utf8_lossy(slice).to_string()
        }
    };

    let future = Box::new(Future::failed(error_msg));
    Box::into_raw(future)
}

/// Await a future and get its i64 value (blocking)
/// FFI: qi_future_await_i64(future: *mut Future) -> i64
/// Returns: value on success, -1 on failure
#[no_mangle]
pub extern "C-unwind" fn qi_future_await_i64(future: *mut Future) -> i64 {
    if future.is_null() {
        return -1;
    }
    // coro future（QI_CORO）：首 8 字节 magic 命中 → 交协程 executor 驱动取值。
    if unsafe { crate::async_runtime::coro::is_coro(future as *const _) } {
        return crate::async_runtime::coro::qi_coro_await_i64(future as *mut _);
    }

    unsafe {
        let future_ref = &*future;
        match future_ref.await_value() {
            Ok(FutureValue::Integer(value)) => value,
            Err(e) => throw_future_error(e),
            _ => -1,
        }
    }
}

/// Await a future and get its f64 value (blocking)
/// FFI: qi_future_await_f64(future: *mut Future) -> f64
/// Returns: value on success, 0.0 on failure
#[no_mangle]
pub extern "C-unwind" fn qi_future_await_f64(future: *mut Future) -> f64 {
    if future.is_null() {
        return 0.0;
    }
    // coro future（QI_CORO）：magic 命中 → 取 i64 位模式再 bitcast 回 f64。
    if unsafe { crate::async_runtime::coro::is_coro(future as *const _) } {
        let bits = crate::async_runtime::coro::qi_coro_await_i64(future as *mut _);
        return f64::from_bits(bits as u64);
    }

    unsafe {
        let future_ref = &*future;
        match future_ref.await_value() {
            Ok(FutureValue::Float(value)) => value,
            Err(e) => throw_future_error(e),
            _ => 0.0,
        }
    }
}

/// Await a future and get its boolean value (blocking)
/// FFI: qi_future_await_bool(future: *mut Future) -> i32
/// Returns: 1 for true, 0 for false/failure
#[no_mangle]
pub extern "C-unwind" fn qi_future_await_bool(future: *mut Future) -> i32 {
    if future.is_null() {
        return 0;
    }
    // coro future（QI_CORO）：magic 命中 → i64 值 !=0 即 true。
    if unsafe { crate::async_runtime::coro::is_coro(future as *const _) } {
        return (crate::async_runtime::coro::qi_coro_await_i64(future as *mut _) != 0) as i32;
    }

    unsafe {
        let future_ref = &*future;
        match future_ref.await_value() {
            Ok(FutureValue::Boolean(value)) => {
                if value {
                    1
                } else {
                    0
                }
            }
            Err(e) => throw_future_error(e),
            _ => 0,
        }
    }
}

/// Await a future and get its string value (blocking)
/// FFI: qi_future_await_string(future: *mut Future) -> *const c_char
/// Returns: null-terminated C string, caller must free with qi_string_free
#[no_mangle]
pub extern "C-unwind" fn qi_future_await_string(future: *mut Future) -> *const c_char {
    if future.is_null() {
        return std::ptr::null();
    }
    // coro future（QI_CORO）：promise 里是 +1 RC 字符串指针 —— take 移交调用方。
    if unsafe { crate::async_runtime::coro::is_coro(future as *const _) } {
        return crate::async_runtime::coro::qi_coro_take_ptr(future as *mut _) as *const c_char;
    }

    unsafe {
        let future_ref = &*future;
        match future_ref.await_value() {
            Ok(FutureValue::String(s)) => {
                // Allocate RC C string that caller must free (qi_string_free)
                crate::stdlib::qi_str::rc_cstr_from_string(s)
            }
            Err(e) => throw_future_error(e),
            _ => std::ptr::null(),
        }
    }
}

/// Await a future and get its pointer value (blocking)
/// FFI: qi_future_await_ptr(future: *mut Future) -> *mut u8
/// Returns: pointer value on success, null on failure
///
/// Round E 所有权语义（与 codegen 的 ARC 纪律对齐）：
/// - Pointer payload：**take** —— 把指针从 future 内部取走（置 None），
///   所有权（创建时转移进 future 的那份 +1）随返回值移交调用方。
///   再次 await 同一 future 返回 null（take 语义，杜绝双释放）。
/// - String payload：每次 await 返回**新分配**的 rc C 串（+1 交调用方，
///   可重复 await，各自独立释放）——顺带修复老的 Pointer-only 匹配漏洞
///   （`未来<字符串>` 由 ready_string/FFI 完成时 payload 是 String）。
#[no_mangle]
pub extern "C-unwind" fn qi_future_await_ptr(future: *mut Future) -> *mut u8 {
    if future.is_null() {
        return std::ptr::null_mut();
    }
    // coro future（QI_CORO）：promise 里是 +1 RC 指针（字符串/结构体）—— take
    // 移交调用方（二次 take 得 null，杜绝双释放）。
    if unsafe { crate::async_runtime::coro::is_coro(future as *const _) } {
        return crate::async_runtime::coro::qi_coro_take_ptr(future as *mut _);
    }

    unsafe {
        let future_ref = &*future;
        match future_ref.await_value() {
            Ok(FutureValue::Pointer(_)) => {
                // take：在锁内原子取走所有权并置空 —— 并发双 await 时只有
                // 一方拿到指针，另一方得 null（绝不双释放）。
                let mut guard = future_ref.value.lock().unwrap();
                if let Some(FutureValue::Pointer(p)) = *guard {
                    *guard = None;
                    p
                } else {
                    std::ptr::null_mut()
                }
            }
            Ok(FutureValue::String(s)) => crate::stdlib::qi_str::rc_cstr_from_string(s) as *mut u8,
            Err(e) => throw_future_error(e),
            _ => std::ptr::null_mut(),
        }
    }
}

/// Free a C string returned by qi_future_await_string
/// FFI: qi_string_free(str_ptr: *mut c_char)
///
/// 委托 rc_cstr_release：只释放带 RC header 的运行时分配串；
/// 非 RC 指针（历史 CString::into_raw / 外部串）一次性警告后静默泄漏，不崩溃。
#[no_mangle]
pub extern "C" fn qi_string_free(str_ptr: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(str_ptr);
}

/// Check if a future is completed
/// FFI: qi_future_is_completed(future: *mut Future) -> i32
/// Returns: 1 if completed, 0 otherwise
#[no_mangle]
pub extern "C" fn qi_future_is_completed(future: *mut Future) -> i32 {
    if future.is_null() {
        return 0;
    }

    unsafe {
        let future_ref = &*future;
        if future_ref.is_completed() {
            1
        } else {
            0
        }
    }
}

/// Free a future
/// FFI: qi_future_free(future: *mut Future)
///
/// Round E：future 私藏的 Pointer payload 若从未被 await 取走，释放前
/// 用 qi_rc_release_any 归还那份 +1（STR magic 完整释放 / OBJ 浅释放 /
/// 其余静默）。await 取走过的（value 已置 None）自然跳过 —— 无双释放。
#[no_mangle]
pub extern "C" fn qi_future_free(future: *mut Future) {
    if !future.is_null() {
        unsafe {
            let boxed = Box::from_raw(future);
            let leftover = boxed.value.lock().ok().and_then(|mut guard| guard.take());
            if let Some(FutureValue::Pointer(p)) = leftover {
                crate::stdlib::rc_obj::qi_rc_release_any(p as *const u8);
            }
            drop(boxed);
        }
    }
}

// ===== Tests =====

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;
    use std::os::raw::c_char;

    #[test]
    fn test_future_ready_i64() {
        let future = Future::ready_i64(42);
        assert!(future.is_completed());
        match future.await_value().unwrap() {
            FutureValue::Integer(v) => assert_eq!(v, 42),
            _ => panic!("Expected integer value"),
        }
    }

    #[test]
    fn test_future_ready_f64() {
        let future = Future::ready_f64(3.14);
        assert!(future.is_completed());
        match future.await_value().unwrap() {
            FutureValue::Float(v) => assert!((v - 3.14).abs() < 0.0001),
            _ => panic!("Expected float value"),
        }
    }

    #[test]
    fn test_future_ready_bool() {
        let future = Future::ready_bool(true);
        assert!(future.is_completed());
        match future.await_value().unwrap() {
            FutureValue::Boolean(v) => assert_eq!(v, true),
            _ => panic!("Expected boolean value"),
        }
    }

    #[test]
    fn test_future_ready_string() {
        let future = Future::ready_string("Hello".to_string());
        assert!(future.is_completed());
        match future.await_value().unwrap() {
            FutureValue::String(s) => assert_eq!(s, "Hello"),
            _ => panic!("Expected string value"),
        }
    }

    #[test]
    fn test_future_failed() {
        let future = Future::failed("Test error".to_string());
        assert!(!future.is_completed());
        assert!(future.await_value().is_err());
    }

    #[test]
    fn test_ffi_future_ready_i64() {
        let future_ptr = qi_future_ready_i64(100);
        assert!(!future_ptr.is_null());

        let value = qi_future_await_i64(future_ptr);
        assert_eq!(value, 100);

        qi_future_free(future_ptr);
    }

    #[test]
    fn test_ffi_future_ready_f64() {
        let future_ptr = qi_future_ready_f64(2.718);
        assert!(!future_ptr.is_null());

        let value = qi_future_await_f64(future_ptr);
        assert!((value - 2.718).abs() < 0.0001);

        qi_future_free(future_ptr);
    }

    #[test]
    fn test_ffi_future_ready_bool() {
        let future_ptr = qi_future_ready_bool(1);
        assert!(!future_ptr.is_null());

        let value = qi_future_await_bool(future_ptr);
        assert_eq!(value, 1);

        qi_future_free(future_ptr);
    }

    #[test]
    fn test_ffi_future_ready_string() {
        let test_str = "测试字符串";
        let future_ptr = qi_future_ready_string(test_str.as_ptr(), test_str.len());
        assert!(!future_ptr.is_null());

        let result_ptr = qi_future_await_string(future_ptr);
        assert!(!result_ptr.is_null());

        unsafe {
            let c_str = CStr::from_ptr(result_ptr);
            let rust_str = c_str.to_string_lossy();
            assert_eq!(rust_str, test_str);
            qi_string_free(result_ptr as *mut c_char);
        }

        qi_future_free(future_ptr);
    }

    #[test]
    fn await_ptr_takes_ownership_once() {
        // Pointer payload：第一次 await 取走（future 置空），第二次得 null
        let obj = crate::stdlib::rc_obj::qi_obj_alloc(16);
        let fut = qi_future_ready_ptr(obj);
        let got = qi_future_await_ptr(fut);
        assert_eq!(got, obj);
        let second = qi_future_await_ptr(fut);
        assert!(second.is_null(), "take 后第二次 await 应得 null");
        // free 时 payload 已被取走 → 不双释放
        qi_future_free(fut);
        // 调用方持有唯一 +1，正常释放
        crate::stdlib::rc_obj::qi_rc_release_any(obj as *const u8);
    }

    #[test]
    fn future_free_releases_untaken_ptr_payload() {
        // rc 观测（不依赖全局活跃计数 —— 并行测试会扰动那个）：
        // obj rc=2（我方 +1、future 持 +1）→ free future 归还其 +1 → rc=1
        let obj = crate::stdlib::rc_obj::qi_obj_alloc(8);
        crate::stdlib::rc_obj::qi_obj_retain(obj); // rc=2
        let fut = qi_future_ready_ptr(obj);
        // 从未 await → free 归还 payload 的 +1
        qi_future_free(fut);
        // 只剩我方一份：dec 返回旧值 1 ⇒ future 的 +1 确已释放
        assert_eq!(
            crate::stdlib::rc_obj::qi_obj_dec(obj),
            1,
            "未取走的 Pointer payload 应随 future free 释放"
        );
        crate::stdlib::rc_obj::qi_obj_free(obj);
    }

    #[test]
    fn await_ptr_handles_string_payload() {
        // 未来<字符串>（payload 是 String）经 await_ptr：每次返回新 rc 串
        let s = "串负载";
        let fut = qi_future_ready_string(s.as_ptr(), s.len());
        let p1 = qi_future_await_ptr(fut);
        let p2 = qi_future_await_ptr(fut);
        assert!(!p1.is_null() && !p2.is_null());
        unsafe {
            assert_eq!(CStr::from_ptr(p1 as *const c_char).to_str().unwrap(), s);
            assert_eq!(CStr::from_ptr(p2 as *const c_char).to_str().unwrap(), s);
        }
        qi_string_free(p1 as *mut c_char);
        qi_string_free(p2 as *mut c_char);
        qi_future_free(fut);
    }

    #[test]
    fn test_ffi_is_completed() {
        let future_ptr = qi_future_ready_i64(42);
        let is_completed = qi_future_is_completed(future_ptr);
        assert_eq!(is_completed, 1);
        qi_future_free(future_ptr);
    }
}
