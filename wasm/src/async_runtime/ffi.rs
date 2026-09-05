//! goroutine / 定时器（wasm 版）—— 单线程里就地执行
//!
//! 主运行时的 `启动` 把函数丢进 tokio 的 blocking pool，真并发。wasm32-wasip1 没有
//! 第二根线程（wasi threads 提案不在 rustup 的目标里），所以这里的 `启动` 语义是：
//! **立刻在当前栈上同步跑完再返回**。对「启动几个任务然后 等待组.等待」这种写法，
//! 结果完全一样，只是没有并行；对「启动一个死循环的后台任务再干别的」这种写法，
//! 后台任务会先把主线程占住 —— 这是 wasm 的边界，README 里写着。
//!
//! 可等待句柄（启动并等待协程 / 等待协程）同样退化：句柄发出去时任务已经跑完了。

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::raw::{c_char, c_void};
use std::time::{SystemTime, UNIX_EPOCH};

thread_local! {
    static HANDLES: RefCell<HashMap<i64, Option<String>>> = RefCell::new(HashMap::new());
    static NEXT_HANDLE: RefCell<i64> = const { RefCell::new(1) };
}

/// 无参 goroutine：就地调用
#[no_mangle]
pub extern "C" fn qi_runtime_spawn_goroutine(function_ptr: *const c_void) {
    if function_ptr.is_null() {
        return;
    }
    let _g = crate::stdlib::exception_ffi::GoroutineGuard::new();
    let f: fn() = unsafe { std::mem::transmute::<*const c_void, fn()>(function_ptr) };
    f();
}

/// 带参 goroutine：codegen 把实参打包成 i64 数组，包装函数按约定读取。
/// 主运行时会先把数组拷一份（因为调用方的栈马上就没了）；这里同步执行，
/// 调用方的栈还在，直接用原指针。
#[no_mangle]
pub extern "C" fn qi_runtime_spawn_goroutine_with_args(
    wrapper_fn: *const c_void,
    args: *const i64,
    _arg_count: i64,
) {
    if wrapper_fn.is_null() {
        return;
    }
    let _g = crate::stdlib::exception_ffi::GoroutineGuard::new();
    let wrapper: fn(*const i64) =
        unsafe { std::mem::transmute::<*const c_void, fn(*const i64)>(wrapper_fn) };
    wrapper(args);
}

/// 可等待协程：fat 闭包对象 slot0 = fn 指针，调用约定 fn(env)。就地跑完，发一个已完成句柄。
#[no_mangle]
pub extern "C" fn qi_runtime_spawn_goroutine_handle(closure_obj: *const c_void) -> i64 {
    if closure_obj.is_null() {
        return -1;
    }
    let handle = NEXT_HANDLE.with(|n| {
        let mut n = n.borrow_mut();
        let h = *n;
        *n += 1;
        h
    });
    {
        let _g = crate::stdlib::exception_ffi::GoroutineGuard::new();
        unsafe {
            let fn_addr = *(closure_obj as *const usize);
            let f = std::mem::transmute::<usize, fn(*const c_void)>(fn_addr);
            f(closure_obj);
        }
    }
    HANDLES.with(|h| h.borrow_mut().insert(handle, None));
    handle
}

#[no_mangle]
pub extern "C" fn qi_runtime_goroutine_join(handle: i64) -> i64 {
    HANDLES.with(|h| {
        if h.borrow().contains_key(&handle) {
            0
        } else {
            -1
        }
    })
}

#[no_mangle]
pub extern "C" fn qi_runtime_goroutine_has_exception(handle: i64) -> i64 {
    HANDLES.with(|h| match h.borrow().get(&handle) {
        Some(Some(_)) => 1,
        Some(None) => 0,
        None => -1,
    })
}

#[no_mangle]
pub extern "C" fn qi_runtime_goroutine_take_exception(handle: i64) -> *mut c_char {
    let msg = HANDLES.with(|h| h.borrow_mut().get_mut(&handle).and_then(|slot| slot.take()));
    match msg {
        Some(m) => crate::stdlib::qi_str::rc_cstr_from_string(m),
        None => crate::stdlib::qi_str::rc_cstr_from_str(""),
    }
}

// ── 时间 / 定时器 ────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn qi_runtime_get_time_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 主运行时里是把超时挂到调度器上；单线程没有调度器，记下来即可
#[no_mangle]
pub extern "C" fn qi_runtime_set_timeout(_timeout_ms: i64) -> i64 {
    0
}

/// 定时器 = 一个装着截止时刻的堆上 i64
#[no_mangle]
pub extern "C" fn qi_runtime_timer_create(deadline_ms: i64) -> *mut c_void {
    Box::into_raw(Box::new(deadline_ms)) as *mut c_void
}

#[no_mangle]
pub extern "C" fn qi_runtime_timer_expired(timer: *mut c_void) -> i64 {
    if timer.is_null() {
        return -1;
    }
    let deadline = unsafe { *(timer as *const i64) };
    (qi_runtime_get_time_ms() >= deadline) as i64
}

#[no_mangle]
pub extern "C" fn qi_runtime_timer_stop(timer: *mut c_void) -> i64 {
    if timer.is_null() {
        return -1;
    }
    unsafe { drop(Box::from_raw(timer as *mut i64)) };
    0
}

/// select 空转退避：单线程里没有别的协程会推进状态，睡 1ms 只是让出 CPU 给宿主
#[no_mangle]
pub extern "C" fn qi_runtime_select_backoff() {
    std::thread::sleep(std::time::Duration::from_millis(1));
}
