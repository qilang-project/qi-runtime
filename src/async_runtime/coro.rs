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
    /// R5 park-wake：step() resume 前设为当前协程；供通道 park 记录「谁在等」。
    static CURRENT: Cell<*mut QiCoro> = const { Cell::new(std::ptr::null_mut()) };
    /// R5：本次 resume 是否 park 了（挂到通道等待者列表）。true → step 不回队。
    static PARKED: Cell<bool> = const { Cell::new(false) };
    /// R5：当前 parked（挂在通道上、不在 PENDING）的协程数 —— 死锁检测。
    static PARKED_COUNT: Cell<i64> = const { Cell::new(0) };
    /// R5 **延迟 park**：顶层 `启动` 跑 ramp 时 CURRENT=null，协程首次 park 无法立刻
    /// 挂到通道（还没 QiCoro 对象）。park_recv/send 此时把「意图」记这里，
    /// qi_coro_spawn 建出 QiCoro 后据此挂到通道等待者列表（而非 PENDING）。
    /// 0=无 / 1=recv / 2=send；目标通道 + send 值。
    static PARK_INTENT_KIND: Cell<i32> = const { Cell::new(0) };
    static PARK_INTENT_CHAN: Cell<*mut QiCoroChan> = const { Cell::new(std::ptr::null_mut()) };
    static PARK_INTENT_VAL: Cell<i64> = const { Cell::new(0) };
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
    // R5 延迟 park：ramp 期间（CURRENT=null）协程已表达 park 意图 → 挂到通道等待者列表，
    // 不入 PENDING（否则会自旋、且与唤醒链交互产生 lost-wakeup/hang —— R5 第一版回退的根因）。
    let kind = PARK_INTENT_KIND.with(|k| k.replace(0));
    if kind != 0 {
        let ch = PARK_INTENT_CHAN.with(|c| c.replace(std::ptr::null_mut()));
        if !ch.is_null() {
            unsafe {
                if kind == 1 {
                    (*ch).recv_waiters.push_back(c);
                } else {
                    let v = PARK_INTENT_VAL.with(|x| x.get());
                    (*ch).send_waiters.push_back((c, v));
                }
            }
            PARKED_COUNT.with(|n| n.set(n.get() + 1));
            return c;
        }
    }
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
            // R5：记录当前协程 + 清 park 标志（通道 park 会置 true）。
            CURRENT.with(|x| x.set(c));
            PARKED.with(|p| p.set(false));
            let hdl = unsafe { (*c).hdl };
            (ops.resume)(hdl);
            CURRENT.with(|x| x.set(std::ptr::null_mut()));
            if (ops.done)(hdl) {
                let v = (ops.promise)(hdl);
                unsafe {
                    (*c).value = v;
                    (*c).done = true;
                }
                (ops.destroy)(hdl);
                // 不回收 QiCoro：留给 `等待` 读值。QiCoro 非 RC，不影响 QI_RC_REPORT。
            } else if PARKED.with(|p| p.get()) {
                // R5：协程 park 在某通道等待者列表上 —— 不回 PENDING（由 send/recv 唤醒）。
                PARKED_COUNT.with(|n| n.set(n.get() + 1));
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
/// R5：结束时若仍有 parked 协程（挂通道上无人唤醒）→ 死锁，告警不 hang。
#[no_mangle]
pub extern "C" fn qi_coro_run_all() {
    while step() {}
    let parked = PARKED_COUNT.with(|n| n.get());
    if parked > 0 {
        eprintln!(
            "[qi-coro] 死锁：{} 个协程挂在通道上等待，但执行器已无可运行协程（无人发送/接收）。",
            parked
        );
    }
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

/// R2：同步 `等待` 一个返回 RC 指针（字符串/结构体）的 coroutine future ——
/// **take 语义**：promise 里的 +1 随返回值移交调用方，值槽置 0；二次 take
/// 得 null（与 eager `qi_future_await_ptr` 的 take 一致，杜绝双释放）。
///
/// # Safety
/// `c` 必须是 `qi_coro_spawn` 返回的有效 QiCoro 指针（或 null）。
#[no_mangle]
pub extern "C" fn qi_coro_take_ptr(c: *mut QiCoro) -> *mut u8 {
    if c.is_null() {
        return std::ptr::null_mut();
    }
    qi_coro_run_until_done(c);
    unsafe {
        let v = (*c).value;
        (*c).value = 0;
        v as *mut u8
    }
}

/// R2：单步驱动执行器（resume 一个就绪协程一次 / 或睡到最早唤醒时刻）。
/// 返回 1 = 队列还有协程；0 = 队列已空。
#[no_mangle]
pub extern "C" fn qi_coro_step_once() -> i64 {
    step() as i64
}

/// R4：协作式 await 的**非阻塞轮询**。1 = 已就绪（可取值）；0 = 未就绪（应让出）。
/// 统一处理两类 future：
/// - 协程 future（QiCoro，magic 命中）：看 `done` 标志；
/// - eager `Future`（含 异步询问/异步对话 的**异步 IO** pending future）：看 state
///   —— Completed/Failed 即就绪，Pending 未就绪。
///
/// 协程体内 `等待 <future>` 编译成「poll；未就绪则 让出+挂起；resume 再 poll」的
/// 协作式循环（见 codegen 异步.rs 协程等待轮询）。这样：① 协程内 await 另一协程时
/// 让出执行权、由外层执行器驱动被等协程到完成（不再 re-entrant 驱动崩溃）；② await
/// 异步 IO 时让出，HTTP 后台线程在飞、同执行器上别的协程得以运行 —— 真 IO 上车。
/// 取值仍走既有 qi_future_await_*（此刻已就绪，立即返回不阻塞）。
///
/// # Safety
/// `fut` 为 null 或指向 QiCoro / eager `Future` 的有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_await_poll(fut: *const c_void) -> i32 {
    if fut.is_null() {
        return 1; // null → 视为已就绪（await 取默认值，不死等）
    }
    if unsafe { is_coro(fut) } {
        return unsafe { (*(fut as *const QiCoro)).done } as i32;
    }
    // eager Future（异步 IO pending future）：Completed/Failed 即就绪。
    use crate::async_runtime::future::{Future, FutureState};
    let f = unsafe { &*(fut as *const Future) };
    let st = f.state.lock().unwrap().clone();
    match st {
        FutureState::Pending => 0,
        _ => 1,
    }
}

// ───────────────────────── Round 3：协程原生通道 ─────────────────────────
//
// 与 tokio 版 `qi_runtime_channel_*`（跨线程、阻塞、boxed-i64 ABI）不同：本通道
// 活在**同一个单线程 executor 世界**里，非阻塞 try_send/try_recv，值按裸 i64 位模式
// 直传（不 box）。QI_CORO=1 且在协程上下文时，codegen 把 `<- ch` 编译成
// 「try_recv；空则 让出+挂起；resume 再试」的**协作式挂起**循环 —— 协程收空通道时
// 让出执行权而非占死线程，另一协程得以运行并发送，从而唤醒等待者。
//
// cap 语义（R3 简化）：cap<=0 视为无界（协作式下发送恒成功）；cap>0 为软上限，满则
// try_send 返回 1（当前 codegen 发送不挂起，仅无界/软上限，真·背压挂起留 R4）。
// 通道本体是裸 Box（非 RC，不带引用计数），不进入 QI_RC_REPORT 统计；通过通道传的
// RC 值（字符串/结构体指针）由 codegen 在发送端 retain、接收端按 OWNED 接管，净额平衡。

/// 协程原生通道对象。单线程 executor 内使用，无需锁。
/// R5 park-wake：收空通道的协程 park 进 recv_waiters；发满通道的协程连同值 park 进
/// send_waiters。对端操作时把等待者移回 PENDING（唤醒），免去 R3/R4 的协作式空转。
#[repr(C)]
pub struct QiCoroChan {
    buf: VecDeque<i64>,
    cap: usize, // 0 = 无界
    closed: bool,
    recv_waiters: VecDeque<*mut QiCoro>,        // 等接收的协程
    send_waiters: VecDeque<(*mut QiCoro, i64)>, // 等发送的协程（连同待发值）
}

/// R5：唤醒一个 parked 协程 —— 移回 PENDING（立即就绪）、parked 计数减一。
fn wake(c: *mut QiCoro) {
    if c.is_null() {
        return;
    }
    unsafe {
        (*c).wake_at = 0;
    }
    PENDING.with(|p| p.borrow_mut().push_back(c));
    PARKED_COUNT.with(|n| n.set((n.get() - 1).max(0)));
}

/// `通道<T>(cap)`（协程模式）→ 协程原生通道句柄。
#[no_mangle]
pub extern "C" fn qi_coro_chan_new(cap: i64) -> *mut QiCoroChan {
    Box::into_raw(Box::new(QiCoroChan {
        buf: VecDeque::new(),
        cap: if cap <= 0 { 0 } else { cap as usize },
        closed: false,
        recv_waiters: VecDeque::new(),
        send_waiters: VecDeque::new(),
    }))
}

/// 非阻塞发送：0 = 已入队（并唤醒一个等待接收者，若有）；1 = 有界且满（调用方应 park_send）。
///
/// # Safety
/// `ch` 为 null 或 `qi_coro_chan_new` 返回的有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_try_send(ch: *mut QiCoroChan, v: i64) -> i32 {
    if ch.is_null() {
        return 1;
    }
    let c = unsafe { &mut *ch };
    if c.cap != 0 && c.buf.len() >= c.cap {
        return 1; // 满 → 调用方 park_send
    }
    c.buf.push_back(v);
    if let Some(r) = c.recv_waiters.pop_front() {
        wake(r); // 唤醒一个等接收者（resume 后 try_recv 取到）
    }
    0
}

/// 非阻塞接收：0 = 取到（写入 `*slot`）；1 = 空（调用方应 park_recv）。i64 单层直传。
/// 取走一个后若有协程 park 在 send_waiters（因满而等）→ 把其值补进缓冲并唤醒它。
///
/// # Safety
/// `ch`/`slot` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_try_recv(ch: *mut QiCoroChan, slot: *mut i64) -> i32 {
    if ch.is_null() || slot.is_null() {
        return 1;
    }
    let c = unsafe { &mut *ch };
    match c.buf.pop_front() {
        Some(v) => {
            unsafe {
                *slot = v;
            }
            if let Some((s, sv)) = c.send_waiters.pop_front() {
                c.buf.push_back(sv);
                wake(s);
            }
            0
        }
        None => 1,
    }
}

/// R5：收空通道 → 把当前协程 park 进 recv_waiters（CURRENT 有值），或记延迟 park 意图
/// （ramp 期间 CURRENT=null，交 qi_coro_spawn 落实）。置 PARKED，供 step 不回队。
///
/// # Safety
/// `ch` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_park_recv(ch: *mut QiCoroChan) {
    if ch.is_null() {
        return;
    }
    let cur = CURRENT.with(|x| x.get());
    if cur.is_null() {
        // ramp 期间：记意图，qi_coro_spawn 建出 QiCoro 后挂到 recv_waiters。
        PARK_INTENT_KIND.with(|k| k.set(1));
        PARK_INTENT_CHAN.with(|c| c.set(ch));
        return;
    }
    unsafe { (*ch).recv_waiters.push_back(cur) };
    PARKED.with(|p| p.set(true));
}

/// R5：发满通道 → 把当前协程连同待发值 park 进 send_waiters（或记延迟意图）。
/// 被唤醒时值已由接收方补进缓冲（视作已发送），resume 后无需重试。
///
/// # Safety
/// `ch` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_park_send(ch: *mut QiCoroChan, v: i64) {
    if ch.is_null() {
        return;
    }
    let cur = CURRENT.with(|x| x.get());
    if cur.is_null() {
        PARK_INTENT_KIND.with(|k| k.set(2));
        PARK_INTENT_CHAN.with(|c| c.set(ch));
        PARK_INTENT_VAL.with(|x| x.set(v));
        return;
    }
    unsafe { (*ch).send_waiters.push_back((cur, v)) };
    PARKED.with(|p| p.set(true));
}

/// 关闭通道（当前 codegen 未用；预留给 R4 的收端「关闭即结束」语义）。
///
/// # Safety
/// `ch` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_close(ch: *mut QiCoroChan) {
    if !ch.is_null() {
        unsafe { (*ch).closed = true };
    }
}

/// 释放协程通道。
///
/// # Safety
/// `ch` 为 null 或 `qi_coro_chan_new` 返回且未释放过的指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_free(ch: *mut QiCoroChan) {
    if !ch.is_null() {
        unsafe { drop(Box::from_raw(ch)) };
    }
}

/// R2：提前销毁一个未完成的协程（`取消未来`）。
/// - 未完成：从待驱动队列摘除 + destroy handle —— destroy 克隆走 coroutine 的
///   cleanup 路径，frame 内 RC 槽在 coro.free 前逐个释放（R2 的灵魂）。
/// - 已完成/已取消/null：no-op（幂等，绝不双 destroy）。
/// 取消后 done=true、value=0：再 `等待` 得 0/null（勿使用）。
///
/// # Safety
/// `c` 必须是 `qi_coro_spawn` 返回的有效 QiCoro 指针（或 null）。
#[no_mangle]
pub extern "C" fn qi_coro_cancel(c: *mut QiCoro) {
    if c.is_null() || unsafe { (*c).done } {
        return;
    }
    // 从待驱动队列摘除（防 executor 之后 resume 已 destroy 的 handle）
    PENDING.with(|p| p.borrow_mut().retain(|&x| x != c));
    let ops = ops();
    let hdl = unsafe { (*c).hdl };
    (ops.destroy)(hdl);
    unsafe {
        (*c).done = true;
        (*c).value = 0;
    }
}
