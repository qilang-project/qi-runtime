//! 异常（wasm 版）—— 只有「抛出」，没有「捕获」
//!
//! 主运行时用 setjmp/longjmp 实现 `尝试 / 捕获 / 最终 / 抛出`。wasi libc **没有**
//! setjmp（要 wasm 异常处理提案 + `-mllvm -wasm-enable-sjlj` + libsetjmp，rustup 自带的
//! sysroot 里没有），所以控制转移这一步在 wasm 上目前做不出来。
//!
//! 这里保留的语义：`抛出` = 把消息打到 stderr，然后以退出码 1 结束进程。
//! 对「没写 尝试、只靠抛出报错退出」的程序这是等价的；写了 `尝试` 的程序会在链接期
//! 报 `setjmp` 未定义 —— 宁可链不过，不要静默地把 `捕获` 当成没写。
//!
//! 同时提供 goroutine 相关的几个内部接口，供 `async_runtime::ffi` 与 `future` 复用
//! 主运行时源码时调用（wasm 里 goroutine 就地跑，这些大多是空转）。

use std::cell::RefCell;
use std::ffi::CStr;
use std::os::raw::c_char;

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
    static GOROUTINE_ERRORS: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

/// 登记消息（与主运行时同名同义）
#[no_mangle]
pub extern "C" fn qi_exc_stage(msg: *const c_char) {
    let s = if msg.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(msg) }
            .to_string_lossy()
            .into_owned()
    };
    LAST_ERROR.with(|e| *e.borrow_mut() = s);
}

fn die() -> ! {
    let msg = LAST_ERROR.with(|e| e.borrow().clone());
    eprintln!("未捕获的异常: {}", msg);
    eprintln!("（wasm 目标暂不支持 尝试/捕获，抛出即退出）");
    std::process::exit(1)
}

#[no_mangle]
pub extern "C-unwind" fn qi_exc_throw_staged() -> ! {
    die()
}

#[no_mangle]
pub extern "C-unwind" fn qi_exc_throw(msg: *const c_char) -> ! {
    qi_exc_stage(msg);
    die()
}

/// 当前消息（RC 串）—— 供不经 longjmp 的路径读消息
#[no_mangle]
pub extern "C" fn qi_exc_message() -> *mut c_char {
    let s = LAST_ERROR.with(|e| e.borrow().clone());
    crate::stdlib::qi_str::rc_cstr_from_string(s)
}

#[no_mangle]
pub extern "C" fn qi_exc_clear() {
    LAST_ERROR.with(|e| e.borrow_mut().clear());
}

// ── goroutine 侧的内部接口（主运行时 ffi/mod.rs 与 future.rs 的复用点）──────

/// 主运行时里它安装一个 panic hook 把 qi 抛出与 Rust panic 区分开；
/// wasm 上 panic = abort（Cargo profile），没有可 unwind 的 panic，无事可做。
pub fn install_qi_panic_hook() {}

/// 主运行时里标记「当前线程在 goroutine 体内」；wasm 单线程，只做占位。
pub struct GoroutineGuard;
impl GoroutineGuard {
    pub fn new() -> Self {
        GoroutineGuard
    }
}
impl Default for GoroutineGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// 把 panic payload 变成消息，并说明是不是 qi 层的抛出。wasm 上 panic 直接 abort，
/// 到不了这里；保留签名让复用的源码编译通过。
pub fn goroutine_panic_message(payload: Box<dyn std::any::Any + Send>) -> (String, bool) {
    if let Some(s) = payload.downcast_ref::<String>() {
        (s.clone(), false)
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        ((*s).to_string(), false)
    } else {
        ("goroutine panic".to_string(), false)
    }
}

pub fn record_goroutine_exception(msg: String) {
    GOROUTINE_ERRORS.with(|q| q.borrow_mut().push(msg));
}

#[no_mangle]
pub extern "C" fn qi_exc_goroutine_count() -> i64 {
    GOROUTINE_ERRORS.with(|q| q.borrow().len() as i64)
}

#[no_mangle]
pub extern "C" fn qi_exc_goroutine_take() -> *mut c_char {
    let next = GOROUTINE_ERRORS.with(|q| {
        let mut q = q.borrow_mut();
        if q.is_empty() {
            None
        } else {
            Some(q.remove(0))
        }
    });
    match next {
        Some(s) => crate::stdlib::qi_str::rc_cstr_from_string(s),
        None => crate::stdlib::qi_str::rc_cstr_from_str(""),
    }
}
