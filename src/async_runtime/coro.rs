//! 真协程最小 executor（Round 1）—— 配合 LLVM `llvm.coro.*` stackless 状态机。
//!
//! FFI 边界：与 future.rs 等既有 runtime FFI 同风格（裸指针入参）。`run_until_done`
//! 的循环条件 `done` 经裸指针在 `step()` 内变更，clippy 静态看不到 —— 显式放行。
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::while_immutable_condition)]
//!
//! QI_CORO=1 时 codegen 把「返回 `未来<T>` 且函数体含 `等待`」的用户函数编译成
//! LLVM coroutine（switched-resume ABI）。协程内的 `等待 让出()` / `等待 异步睡眠(ms)`
//! 是真正的挂起点：控制权交回本 executor，由它轮转恢复。
//!
//! 本文件只做**最小可用**的单线程轮转调度 + 定时器：
//!   - `qi_coro_spawn(hdl)`   把 ramp 返回的 coroutine handle 包成 QiCoro future 并入队。
//!   - `qi_coro_yield_ready()` / `qi_coro_sleep(ms)` 协程挂起前设「下次恢复意愿」。
//!   - `qi_coro_run_all()`     驱动所有排队 coroutine 直到全部完成（round-robin + 定时）。
//!   - `qi_coro_await_i64(c)`  同步 `等待` 一个 coroutine future：未完成先驱动，再取值。
//!   - `qi_coro_register_ops`  注册 codegen 在模块里 emit 的 coro intrinsic 包装函数指针
//!     （resume/done/destroy/promise）—— runtime 无法直接发 llvm.coro.* intrinsic，
//!     故由生成代码提供薄封装，启动时注册进来。
//!
//! Round 1 限制：只支持**标量**（整数/浮点/布尔，经 i64 位模式）跨挂起点与返回值；
//! 跨挂起点的 RC 对象（字符串/结构体/数组）留待 Round 2；真 IO / 大模型 pending
//! future 集成留待 Round 3。QiCoro 本体是裸 malloc（非 RC，不带 magic header 之外的
//! 引用计数），不进入 QI_RC_REPORT 统计。

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::os::raw::c_void;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// QiCoro future 对象魔数（放在结构体**首 8 字节**，供 `qi_future_await_*` 区分
/// coro future 与 eager `Future` —— eager `Future` 首字段是 `Arc` 堆指针，绝不等于此值）。
pub const CORO_MAGIC: u64 = 0xC0DE_F00D_C0A0_5151u64; // 固定的 64 位哨兵常量

/// coro intrinsic 包装函数指针（codegen 在模块里 emit，主程序启动时注册）。
#[derive(Clone, Copy)]
struct CoroOps {
    resume: extern "C" fn(*mut c_void),
    done: extern "C" fn(*mut c_void) -> bool,
    destroy: extern "C" fn(*mut c_void),
    promise: extern "C" fn(*mut c_void) -> i64,
}

// fn 指针是 Send + Sync（只是代码地址）。
static OPS: Mutex<Option<CoroOps>> = Mutex::new(None);

/// 注册 coro intrinsic 包装（codegen 在 `入口` 序言里调一次）。
#[no_mangle]
pub extern "C" fn qi_coro_register_ops(
    resume: extern "C" fn(*mut c_void),
    done: extern "C" fn(*mut c_void) -> bool,
    destroy: extern "C" fn(*mut c_void),
    promise: extern "C" fn(*mut c_void) -> i64,
) {
    *OPS.lock().unwrap() = Some(CoroOps {
        resume,
        done,
        destroy,
        promise,
    });
}

fn ops() -> CoroOps {
    OPS.lock()
        .unwrap()
        .expect("qi_coro_register_ops 未调用（QI_CORO 模式下 codegen 应在入口注册）")
}

/// coro future 对象。`#[repr(C)]` + 首字段 `magic` 是与 eager `Future` 区分的契约。
#[repr(C)]
pub struct QiCoro {
    magic: u64,
    hdl: *mut c_void, // LLVM coroutine handle（frame 指针）
    wake_at: u64,     // 恢复时刻（绝对 ms）；0 = 立即就绪
    done: bool,
    value: i64, // 完成值（标量位模式；浮点 bitcast、布尔 0/1）
}

thread_local! {
    /// 待驱动的 coroutine 队列（单线程轮转）。
    static PENDING: RefCell<VecDeque<*mut QiCoro>> = RefCell::new(VecDeque::new());
    /// 本线程最近一次挂起意愿：0 = 让出（立即就绪）；>0 = 睡到该绝对 ms。
    static LAST_WAKE: Cell<u64> = const { Cell::new(0) };
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

/// 协程内 `等待 让出()` 前调：标记「下次立即就绪」。
#[no_mangle]
pub extern "C" fn qi_coro_yield_ready() {
    LAST_WAKE.with(|w| w.set(0));
}

/// 协程内 `等待 异步睡眠(ms)` 前调：标记「睡到 now+ms 再恢复」。
#[no_mangle]
pub extern "C" fn qi_coro_sleep(ms: i64) {
    let t = if ms <= 0 { 0 } else { now_ms() + ms as u64 };
    LAST_WAKE.with(|w| w.set(t));
}

/// coroutine ramp 初始挂起时由**调用点**调：把 handle 包成 future 并入队。
/// 返回 `QiCoro*`（作为 `未来<T>` 值）。此刻协程已跑到首个挂起点，
/// 首个 `让出/睡眠` 已设好 `LAST_WAKE`。
///
/// # Safety
/// `hdl` 必须是刚由 coroutine ramp 返回的有效 handle。
#[no_mangle]
pub extern "C" fn qi_coro_spawn(hdl: *mut c_void) -> *mut QiCoro {
    let wake = LAST_WAKE.with(|w| w.get());
    let c = Box::into_raw(Box::new(QiCoro {
        magic: CORO_MAGIC,
        hdl,
        wake_at: wake,
        done: false,
        value: 0,
    }));
    PENDING.with(|p| p.borrow_mut().push_back(c));
    c
}

enum Pick {
    Run(*mut QiCoro),
    Sleep(u64),
}

/// 单步调度：从队列取一个就绪 coroutine resume 一次；若无就绪则睡到最早者。
/// 返回 false 表示队列空（全部完成）。
fn step() -> bool {
    let picked = PENDING.with(|p| {
        let mut q = p.borrow_mut();
        if q.is_empty() {
            return None;
        }
        let now = now_ms();
        let mut idx = None;
        let mut earliest = u64::MAX;
        for (i, &c) in q.iter().enumerate() {
            let w = unsafe { (*c).wake_at };
            if w <= now {
                idx = Some(i);
                break;
            }
            if w < earliest {
                earliest = w;
            }
        }
        match idx {
            Some(i) => Some(Pick::Run(q.remove(i).unwrap())),
            None => Some(Pick::Sleep(earliest.saturating_sub(now))),
        }
    });

    match picked {
        None => false,
        Some(Pick::Sleep(ms)) => {
            std::thread::sleep(Duration::from_millis(ms.max(1)));
            true
        }
        Some(Pick::Run(c)) => {
            let ops = ops();
            // 默认「立即就绪」——协程若不再挂起就直接跑到完成；若挂起，
            // 挂起前的 让出/睡眠 会覆盖 LAST_WAKE。
            LAST_WAKE.with(|w| w.set(0));
            let hdl = unsafe { (*c).hdl };
            (ops.resume)(hdl);
            if (ops.done)(hdl) {
                let v = (ops.promise)(hdl);
                unsafe {
                    (*c).value = v;
                    (*c).done = true;
                }
                (ops.destroy)(hdl);
                // 不回收 QiCoro：留给 `等待` 读值。QiCoro 非 RC，不影响 QI_RC_REPORT。
            } else {
                let w = LAST_WAKE.with(|x| x.get());
                unsafe {
                    (*c).wake_at = w;
                }
                PENDING.with(|p| p.borrow_mut().push_back(c));
            }
            true
        }
    }
}

/// 驱动所有排队 coroutine 直到全部完成。
#[no_mangle]
pub extern "C" fn qi_coro_run_all() {
    while step() {}
}

/// 驱动直到指定 coroutine 完成（简化：轮转整个队列直到它 done）。
///
/// # Safety
/// `c` 必须是 `qi_coro_spawn` 返回的有效 QiCoro 指针（或 null）。
#[no_mangle]
pub extern "C" fn qi_coro_run_until_done(c: *mut QiCoro) {
    if c.is_null() {
        return;
    }
    while !unsafe { (*c).done } {
        if !step() {
            break;
        }
    }
}

/// 判断某 future 指针是否为 coro future（读首 8 字节 magic）。
///
/// # Safety
/// `p` 要么为 null，要么指向至少 8 字节可读内存（eager `Future` 或 QiCoro）。
#[inline]
pub unsafe fn is_coro(p: *const c_void) -> bool {
    !p.is_null() && *(p as *const u64) == CORO_MAGIC
}

/// 同步 `等待` 一个 coroutine future 的整数值（未完成先驱动执行器）。
///
/// # Safety
/// `c` 必须是 `qi_coro_spawn` 返回的有效 QiCoro 指针（或 null）。
#[no_mangle]
pub extern "C" fn qi_coro_await_i64(c: *mut QiCoro) -> i64 {
    if c.is_null() {
        return 0;
    }
    qi_coro_run_until_done(c);
    unsafe { (*c).value }
}
