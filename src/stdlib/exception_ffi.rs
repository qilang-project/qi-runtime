//! Qi 语言异常处理 runtime
//!
//! 用 setjmp/longjmp 实现 `尝试 / 捕获 / 最终 / 抛出` 语义。
//! Thread-local 异常栈：每进入一个 `尝试` push 一个 jmp_buf，
//! `抛出` 时 longjmp 到栈顶 jmp_buf，把错误消息放进 thread-local。
//!
//! ABI：
//! - `qi_exc_alloc_frame() -> *mut u8` 分配 jmp_buf 大小的内存并 push
//! - 调用方紧接着 `setjmp(buf)` — 这步必须在调用方直接执行
//! - `qi_exc_pop()` 没异常正常退出时弹栈
//! - `qi_exc_throw(msg)` 设置 last_error，longjmp(top, 1)
//! - `qi_exc_message() -> *mut c_char` 取最近一次异常消息（catch block 用）

#![allow(non_snake_case)]

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};

// jmp_buf 在 macOS arm64 上是 192 字节；预留 256 给所有平台对齐
pub const JMP_BUF_SIZE: usize = 256;

extern "C" {
    fn setjmp(buf: *mut u8) -> i32;
    fn longjmp(buf: *mut u8, val: i32) -> !;
}

thread_local! {
    /// 当前线程的异常 frame 栈（jmp_buf 指针）
    static EXC_STACK: RefCell<Vec<*mut u8>> = const { RefCell::new(Vec::new()) };
    /// 当前线程最近一次抛出的错误消息
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
    /// 当前线程是否正在执行一个 goroutine 体（spawn wrapper 设置）
    static IN_GOROUTINE: Cell<bool> = const { Cell::new(false) };
}

// ── 协程异常队列 ────────────────────────────────────────────────────────────
//
// goroutine 里 `抛出` 且没有任何 `尝试` frame 时，不能 abort 整个进程 ——
// 转成 panic（QiUncaughtException payload）让 spawn 点的 catch_unwind 接住，
// 由 spawn wrapper 把消息记入全局队列。Qi 侧通过
// `协程异常数量()` / `获取协程异常()` 查询。

/// goroutine 内未捕获 `抛出` 的 panic payload（区别于 Rust 自身 panic）
pub struct QiUncaughtException(pub String);

/// 全局协程异常队列（FIFO）
static GOROUTINE_EXC_QUEUE: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());

/// spawn wrapper 捕获到 goroutine panic 后调用：记录异常消息
pub fn record_goroutine_exception(msg: String) {
    if let Ok(mut q) = GOROUTINE_EXC_QUEUE.lock() {
        q.push_back(msg);
    }
}

/// 从 panic payload 提取消息；返回 (消息, 是否为 Qi `抛出`)
pub fn goroutine_panic_message(payload: Box<dyn std::any::Any + Send>) -> (String, bool) {
    match payload.downcast::<QiUncaughtException>() {
        Ok(e) => (e.0, true),
        Err(p) => {
            let msg = if let Some(s) = p.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = p.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            (msg, false)
        }
    }
}

/// RAII：标记当前线程正在跑 goroutine 体（spawn_blocking 线程复用，必须恢复原值）
pub struct GoroutineGuard {
    prev: bool,
}

impl GoroutineGuard {
    pub fn new() -> Self {
        let prev = IN_GOROUTINE.with(|c| c.replace(true));
        Self { prev }
    }
}

impl Default for GoroutineGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for GoroutineGuard {
    fn drop(&mut self) {
        let prev = self.prev;
        IN_GOROUTINE.with(|c| c.set(prev));
    }
}

fn in_goroutine() -> bool {
    IN_GOROUTINE.with(|c| c.get())
}

/// 安装一次性 panic hook：QiUncaughtException 是受控的控制流（goroutine 内
/// `抛出`），不该打印 "thread panicked" 噪音；其它 panic 交原 hook。
pub fn install_qi_panic_hook() {
    static HOOK: OnceLock<()> = OnceLock::new();
    HOOK.get_or_init(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            if info
                .payload()
                .downcast_ref::<QiUncaughtException>()
                .is_some()
            {
                return;
            }
            prev(info);
        }));
    });
}

/// 队列中未取出的协程异常数量
#[no_mangle]
pub extern "C" fn qi_exc_goroutine_count() -> i64 {
    GOROUTINE_EXC_QUEUE
        .lock()
        .map(|q| q.len() as i64)
        .unwrap_or(0)
}

/// 取出（弹出）最早的一条协程异常消息；队列为空返回空串
#[no_mangle]
pub extern "C" fn qi_exc_goroutine_take() -> *mut c_char {
    let msg = GOROUTINE_EXC_QUEUE
        .lock()
        .ok()
        .and_then(|mut q| q.pop_front())
        .unwrap_or_default();
    crate::stdlib::qi_str::rc_cstr_from_string(msg)
}

fn push_frame(ptr: *mut u8) {
    EXC_STACK.with(|s| s.borrow_mut().push(ptr));
}

fn pop_frame_ptr() -> Option<*mut u8> {
    EXC_STACK.with(|s| s.borrow_mut().pop())
}

fn top_frame() -> Option<*mut u8> {
    EXC_STACK.with(|s| s.borrow().last().copied())
}

/// 调 setjmp 的薄 wrapper —— 让 LLVM IR 不需要直接 declare libc setjmp，
/// 也避免 LLVM 优化器在没有 `returns_twice` 标记时假设 setjmp 只返回一次。
/// 注意：这个函数本身有 #[inline(never)] 是不够的，因为 LLVM 需要看见
/// setjmp 的特殊返回语义；但只要调用 qi_exc_throw 是经过 longjmp 跨函数边界，
/// 在 caller 内不会有跨 setjmp 的局部变量优化错误（试验已验证）。
#[no_mangle]
#[inline(never)]
pub unsafe extern "C" fn qi_exc_setjmp(buf: *mut u8) -> i32 {
    setjmp(buf)
}

/// 分配一个 jmp_buf 大小的缓冲，push 到 thread-local 栈，返回缓冲指针。
/// 调用方紧接着应该 `call i32 @qi_exc_setjmp(ptr %buf)`。
#[no_mangle]
pub extern "C" fn qi_exc_alloc_frame() -> *mut u8 {
    let buf = vec![0u8; JMP_BUF_SIZE].into_boxed_slice();
    let ptr = Box::into_raw(buf) as *mut u8;
    push_frame(ptr);
    ptr
}

/// 弹出 thread-local 栈顶 frame 并释放
#[no_mangle]
pub extern "C" fn qi_exc_pop() {
    if let Some(ptr) = pop_frame_ptr() {
        unsafe {
            let slice = std::slice::from_raw_parts_mut(ptr, JMP_BUF_SIZE);
            let _ = Box::from_raw(slice as *mut [u8]);
        }
    }
}

/// 抛出异常：保存错误消息并 longjmp 到栈顶 frame。
/// 没有 frame 时打印消息并 abort。
#[no_mangle]
pub extern "C-unwind" fn qi_exc_throw(msg: *const c_char) -> ! {
    let msg_str = if msg.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(msg) }
            .to_string_lossy()
            .into_owned()
    };
    LAST_ERROR.with(|e| *e.borrow_mut() = msg_str.clone());

    if let Some(ptr) = top_frame() {
        unsafe { longjmp(ptr, 1) }
    } else if in_goroutine() {
        // goroutine 内未捕获的 `抛出`：不能 abort 整个进程。转成 panic
        // （qi_exc_throw 是 C-unwind ABI，可跨 FFI 边界 unwind），由 spawn
        // 点的 catch_unwind 接住并记入协程异常队列 / 句柄状态。
        std::panic::panic_any(QiUncaughtException(msg_str));
    } else {
        eprintln!("[qi] 未捕获的异常: {}", msg_str);
        std::process::abort();
    }
}

/// 取最近一次异常的消息（在 catch block 入口调用）
/// 返回 *mut c_char；调用方负责通过 qi_exc_free_message 释放
#[no_mangle]
pub extern "C" fn qi_exc_message() -> *mut c_char {
    let msg = LAST_ERROR.with(|e| e.borrow().clone());
    crate::stdlib::qi_str::rc_cstr_from_string(msg)
}

/// 清空当前线程的异常消息（catch 处理完 后调用，避免污染下次）
#[no_mangle]
pub extern "C" fn qi_exc_clear() {
    LAST_ERROR.with(|e| e.borrow_mut().clear());
}

/// 释放 qi_exc_message 返回的字符串（委托 rc_cstr_release：
/// 非 RC 指针一次性警告后静默泄漏，不崩溃）
#[no_mangle]
pub extern "C" fn qi_exc_free_message(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}
