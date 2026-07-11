//! 真协程 executor —— 配合 LLVM `llvm.coro.*` stackless 状态机。
//!
//! FFI 边界：与 future.rs 等既有 runtime FFI 同风格（裸指针入参）。
#![allow(clippy::not_unsafe_ptr_arg_deref)]
#![allow(clippy::while_immutable_condition)]
//!
//! QI_CORO=1 时 codegen 把「返回 `未来<T>` 且含挂起点（`等待`/通道收发）」的用户函数
//! 编译成 LLVM coroutine（switched-resume ABI）。挂起点把控制权交回本 executor 轮转恢复。
//!
//! ## R1-R5（单线程协作式，历史）
//! 状态机 / ARC 跨挂起 / goroutine+future 统一调度 / 协作式 await + 真 IO / 通道 park-wake。
//!
//! ## R6：多核 M:N 调度器（`QI_CORO_WORKERS`，默认 1）
//! 就绪队列 `thread_local PENDING` → 全局 `Mutex<VecDeque<Cptr>> + Condvar`；通道内部状态
//! `Mutex` 保护。`执行器运行全部` 起 N worker 线程共同排空（N=QI_CORO_WORKERS，默认 1=
//! 单线程行为，对 R1-R5 零风险）。CURRENT/PARKED/PARK_INTENT 仍 thread_local（每 worker 一份）。
//! - **park/wake race 关闭**：多线程下 `try_recv 空 → park_recv` 之间可能有并发 send 溜进，
//!   故 park_recv/park_send 在通道锁内**重新检查**：buf 已可用则不 park（清 PARKED），worker
//!   重排该协程重试 → 不丢唤醒。
//! - **终止**：`LIVE`（spawn++/完成--）到 0 → 广播退出。
//! - **死锁检测**：就绪队列空 + `RUNNING`==0 + LIVE>0（协程全 park 无人唤醒）→ 报错不 hang。
//! - QiCoro frame 任一时刻仅一个 worker 持有（就绪队列出队即独占）；ARC refcount 是
//!   AtomicI64（qi_str/rc_obj），跨 worker 共享 RC 对象安全。
//! - RC 对象/frame 的数据可见性由就绪队列的 Mutex（release/acquire）提供 happens-before。

use std::cell::Cell;
use std::collections::VecDeque;
use std::os::raw::c_void;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// QiCoro future 对象魔数（首 8 字节，供 `qi_future_await_*` 区分 coro future 与 eager Future）。
pub const CORO_MAGIC: u64 = 0xC0DE_F00D_C0A0_5151u64;

/// coro intrinsic 包装函数指针（codegen emit，主程序启动时注册）。
#[derive(Clone, Copy)]
struct CoroOps {
    resume: extern "C" fn(*mut c_void),
    done: extern "C" fn(*mut c_void) -> bool,
    destroy: extern "C" fn(*mut c_void),
    promise: extern "C" fn(*mut c_void) -> i64,
}
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

/// coro future 对象。`#[repr(C)]` + 首字段 `magic` 是与 eager Future 区分的契约。
#[repr(C)]
pub struct QiCoro {
    magic: u64,
    hdl: *mut c_void, // LLVM coroutine handle（frame 指针）
    wake_at: u64,     // 恢复时刻（绝对 ms）；0 = 立即就绪
    done: bool,
    value: i64, // 完成值（标量位模式）
}

/// R6：可跨 worker 传递的 QiCoro 指针（frame 任一时刻仅一个 worker 持有 → Send 安全）。
#[derive(Clone, Copy, PartialEq)]
struct Cptr(*mut QiCoro);
unsafe impl Send for Cptr {}

/// R6：可跨 worker 传递的通道指针。
#[derive(Clone, Copy)]
struct ChanPtr(*mut QiCoroChan);
unsafe impl Send for ChanPtr {}

// ───────────────────────── R6 全局调度器状态 ─────────────────────────
/// 全局就绪队列（多 worker 共享，Mutex 提供跨 worker happens-before）。
static READY: Mutex<VecDeque<Cptr>> = Mutex::new(VecDeque::new());
/// 有新就绪协程 / 需要唤醒 worker 时通知。
static CV: Condvar = Condvar::new();
/// 活跃协程数（spawn++ / 完成或取消--）。到 0 → 全部完成。
static LIVE: AtomicI64 = AtomicI64::new(0);
/// 正在被某 worker resume 的协程数（就绪队列取出即 ++，跑完 --）。
static RUNNING: AtomicUsize = AtomicUsize::new(0);
/// 本次 run_all 的 worker 总数。
static WORKERS: AtomicUsize = AtomicUsize::new(0);
/// 广播退出（LIVE==0 或死锁）。
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// 死锁标志（run_all 结束时告警）。
static DEADLOCK: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// R5 park-wake：本 worker resume 前设为当前协程；供通道 park 记录「谁在等」。
    static CURRENT: Cell<*mut QiCoro> = const { Cell::new(std::ptr::null_mut()) };
    /// R5：本次 resume 是否 park 了（挂到通道等待者列表）。true → 不回就绪队列。
    static PARKED: Cell<bool> = const { Cell::new(false) };
    /// R5 延迟 park：顶层 `启动` 跑 ramp 时 CURRENT=null，协程首次 park 无 QiCoro 对象，
    /// park_recv/send 记意图，qi_coro_spawn 建出对象后据此挂到通道等待者列表。
    static PARK_INTENT_KIND: Cell<i32> = const { Cell::new(0) };
    static PARK_INTENT_CHAN: Cell<*mut QiCoroChan> = const { Cell::new(std::ptr::null_mut()) };
    static PARK_INTENT_VAL: Cell<i64> = const { Cell::new(0) };
    /// 协程挂起前的下次恢复意愿：0 = 立即就绪；>0 = 睡到该绝对 ms。
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

/// 入就绪队列 + 唤醒等待 worker。notify_all：多 worker 下宁多醒不漏醒（醒来抢不到活会再睡）。
fn push_ready(c: Cptr) {
    READY.lock().unwrap().push_back(c);
    CV.notify_all();
}

/// coroutine ramp 初始挂起时由调用点调：把 handle 包成 future 入队（或据延迟 park 意图挂通道）。
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
    LIVE.fetch_add(1, Ordering::AcqRel);
    // R5 延迟 park：ramp 期间协程已表达 park 意图 → 挂到通道等待者列表（锁内），不入就绪队列。
    let kind = PARK_INTENT_KIND.with(|k| k.replace(0));
    if kind != 0 {
        let ch = PARK_INTENT_CHAN.with(|x| x.replace(std::ptr::null_mut()));
        if !ch.is_null() {
            let chan = unsafe { &*ch };
            let mut inner = chan.inner.lock().unwrap();
            if kind == 1 {
                inner.recv_waiters.push_back(Cptr(c));
            } else {
                let v = PARK_INTENT_VAL.with(|x| x.get());
                inner.send_waiters.push_back((Cptr(c), v));
            }
            return c;
        }
    }
    push_ready(Cptr(c));
    c
}

/// 从就绪队列取一个「到点可跑」的协程。返回：
/// - Some(Ok(c))：取到可跑协程；
/// - Some(Err(ms))：无到点协程但有定时器，最早还需等 ms；
/// - None：队列真空。
/// RUNNING 在取到时同锁内 ++（与死锁检测同步）。
fn pick() -> Option<Result<Cptr, u64>> {
    let mut q = READY.lock().unwrap();
    if q.is_empty() {
        return None;
    }
    let now = now_ms();
    let mut earliest = u64::MAX;
    let mut idx = None;
    for (i, c) in q.iter().enumerate() {
        let w = unsafe { (*c.0).wake_at };
        if w <= now {
            idx = Some(i);
            break;
        }
        if w < earliest {
            earliest = w;
        }
    }
    match idx {
        Some(i) => {
            let c = q.remove(i).unwrap();
            RUNNING.fetch_add(1, Ordering::AcqRel);
            Some(Ok(c))
        }
        None => Some(Err(earliest.saturating_sub(now))),
    }
}

/// resume 一个协程一次，处理 done/park/requeue + 计数。调用者已在 RUNNING 里 +1。
fn run_one(c: Cptr) {
    let ops = ops();
    LAST_WAKE.with(|w| w.set(0));
    CURRENT.with(|x| x.set(c.0));
    PARKED.with(|p| p.set(false));
    let hdl = unsafe { (*c.0).hdl };
    (ops.resume)(hdl);
    CURRENT.with(|x| x.set(std::ptr::null_mut()));
    if (ops.done)(hdl) {
        let v = (ops.promise)(hdl);
        unsafe {
            (*c.0).value = v;
            (*c.0).done = true;
        }
        (ops.destroy)(hdl);
        RUNNING.fetch_sub(1, Ordering::AcqRel);
        if LIVE.fetch_sub(1, Ordering::AcqRel) - 1 <= 0 {
            SHUTDOWN.store(true, Ordering::Release);
            CV.notify_all();
        }
    } else if PARKED.with(|p| p.get()) {
        // 协程 park 在某通道等待者列表 —— 不回就绪队列（由 send/recv 唤醒）。
        RUNNING.fetch_sub(1, Ordering::AcqRel);
    } else {
        let w = LAST_WAKE.with(|x| x.get());
        unsafe {
            (*c.0).wake_at = w;
        }
        // 先入队再 RUNNING--：否则存在「RUNNING=0 但可运行协程尚未入队」的窗口，
        // 会被 worker 误判死锁。入队后 READY 非空，RUNNING-- 无害。
        push_ready(c);
        RUNNING.fetch_sub(1, Ordering::AcqRel);
    }
}

/// worker 主循环（run_all 的每个 worker 线程 + 调用线程都跑它）。
fn worker_loop() {
    loop {
        if SHUTDOWN.load(Ordering::Acquire) {
            return;
        }
        match pick() {
            Some(Ok(c)) => run_one(c),
            Some(Err(sleep_ms)) => {
                // 有定时器但没到点：短睡等最早唤醒（并让出以便别的 worker 抢 wake 的活）。
                std::thread::sleep(Duration::from_millis(sleep_ms.min(5).max(1)));
            }
            None => {
                // 就绪队列空。等待新 work，或判死锁/完成。
                let guard = READY.lock().unwrap();
                if !guard.is_empty() || SHUTDOWN.load(Ordering::Acquire) {
                    continue;
                }
                if LIVE.load(Ordering::Acquire) <= 0 {
                    SHUTDOWN.store(true, Ordering::Release);
                    CV.notify_all();
                    return;
                }
                // 队列空 + 无人在跑 + 仍有活协程 → 全 park 无人唤醒 → 死锁。
                if RUNNING.load(Ordering::Acquire) == 0 {
                    DEADLOCK.store(true, Ordering::Release);
                    SHUTDOWN.store(true, Ordering::Release);
                    CV.notify_all();
                    return;
                }
                // 有 worker 在跑（可能即将 wake 出新 work）→ 等通知（带超时防错过）。
                let _ = CV
                    .wait_timeout(guard, Duration::from_millis(2))
                    .unwrap();
            }
        }
    }
}

/// 驱动所有排队 coroutine 直到全部完成。R6：起 QI_CORO_WORKERS 个 worker 共同排空。
#[no_mangle]
pub extern "C" fn qi_coro_run_all() {
    let n = std::env::var("QI_CORO_WORKERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&x| x >= 1)
        .unwrap_or(1);
    SHUTDOWN.store(false, Ordering::Release);
    DEADLOCK.store(false, Ordering::Release);
    WORKERS.store(n, Ordering::Release);

    if n == 1 {
        // 单 worker：调用线程直接跑（与 R5 单线程行为一致，零线程开销）。
        worker_loop();
    } else {
        let mut handles = Vec::new();
        for _ in 1..n {
            handles.push(std::thread::spawn(worker_loop));
        }
        worker_loop(); // 调用线程也当一个 worker
        for h in handles {
            let _ = h.join();
        }
    }

    if DEADLOCK.load(Ordering::Acquire) {
        eprintln!(
            "[qi-coro] 死锁：仍有 {} 个协程挂在通道上等待，但执行器已无可运行协程（无人发送/接收）。",
            LIVE.load(Ordering::Acquire).max(0)
        );
    }
}

/// 单线程驱动一步（供顶层同步 `等待` / `<- ch` 用；无 worker 上下文）。
/// 返回 1 = 就绪队列还有协程；0 = 空。
fn step() -> bool {
    match pick() {
        Some(Ok(c)) => {
            run_one(c);
            true
        }
        Some(Err(ms)) => {
            std::thread::sleep(Duration::from_millis(ms.min(5).max(1)));
            true
        }
        None => LIVE.load(Ordering::Acquire) > 0,
    }
}

/// 驱动直到指定 coroutine 完成（单线程驱动，供同步 `等待`）。
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
/// `p` 要么为 null，要么指向至少 8 字节可读内存。
#[inline]
pub unsafe fn is_coro(p: *const c_void) -> bool {
    !p.is_null() && *(p as *const u64) == CORO_MAGIC
}

/// 同步 `等待` 一个 coroutine future 的整数值（未完成先驱动）。
///
/// # Safety
/// `c` 必须是有效 QiCoro 指针（或 null）。
#[no_mangle]
pub extern "C" fn qi_coro_await_i64(c: *mut QiCoro) -> i64 {
    if c.is_null() {
        return 0;
    }
    qi_coro_run_until_done(c);
    unsafe { (*c).value }
}

/// R2：同步 `等待` 返回 RC 指针的 coroutine future —— take 语义（值槽置 0，二次得 null）。
///
/// # Safety
/// `c` 必须是有效 QiCoro 指针（或 null）。
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

/// R2：单步驱动执行器（单线程）。返回 1 = 还有协程；0 = 队列空。
#[no_mangle]
pub extern "C" fn qi_coro_step_once() -> i64 {
    step() as i64
}

/// R4：协作式 await 的非阻塞轮询。1 = 已就绪；0 = 未就绪（应让出）。
/// 协程 future 看 done；eager Future（异步 IO）看 state。
///
/// # Safety
/// `fut` 为 null 或指向 QiCoro / eager Future 的有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_await_poll(fut: *const c_void) -> i32 {
    if fut.is_null() {
        return 1;
    }
    if unsafe { is_coro(fut) } {
        return unsafe { (*(fut as *const QiCoro)).done } as i32;
    }
    use crate::async_runtime::future::{Future, FutureState};
    let f = unsafe { &*(fut as *const Future) };
    let st = f.state.lock().unwrap().clone();
    match st {
        FutureState::Pending => 0,
        _ => 1,
    }
}

// ───────────────────────── 协程原生通道（R3-R6） ─────────────────────────
//
// 单线程/多 worker 共用；内部状态 Mutex 保护（R6）。值按裸 i64 位模式直传（不 box）。
// 收空/发满 → 协程 park 进等待者列表；对端操作 wake 一个（移回就绪队列）。
// **park/wake race（R6 多线程）**：try_recv 空 → park_recv 之间可能有并发 send 溜进，
// 故 park_recv/park_send 在锁内**重新检查**：已可用则不 park，worker 重排重试。

struct ChanInner {
    buf: VecDeque<i64>,
    cap: usize, // 0 = 无界
    closed: bool,
    recv_waiters: VecDeque<Cptr>,
    send_waiters: VecDeque<(Cptr, i64)>,
}

/// 协程原生通道对象。`#[repr(C)]` 供 codegen 当句柄传递（内容只经 FFI 访问）。
#[repr(C)]
pub struct QiCoroChan {
    inner: Mutex<ChanInner>,
}

/// 唤醒一个 parked 协程 —— 移回就绪队列（立即就绪）+ 通知 worker。
fn wake(c: Cptr) {
    if c.0.is_null() {
        return;
    }
    unsafe {
        (*c.0).wake_at = 0;
    }
    push_ready(c);
}

/// `通道<T>(cap)`（协程模式）→ 协程原生通道句柄。
#[no_mangle]
pub extern "C" fn qi_coro_chan_new(cap: i64) -> *mut QiCoroChan {
    Box::into_raw(Box::new(QiCoroChan {
        inner: Mutex::new(ChanInner {
            buf: VecDeque::new(),
            cap: if cap <= 0 { 0 } else { cap as usize },
            closed: false,
            recv_waiters: VecDeque::new(),
            send_waiters: VecDeque::new(),
        }),
    }))
}

/// 非阻塞发送：0 = 已入队（并唤醒一个等接收者，若有）；1 = 有界且满（调用方应 park_send）。
///
/// # Safety
/// `ch` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_try_send(ch: *mut QiCoroChan, v: i64) -> i32 {
    if ch.is_null() {
        return 1;
    }
    let chan = unsafe { &*ch };
    let mut inner = chan.inner.lock().unwrap();
    if inner.cap != 0 && inner.buf.len() >= inner.cap {
        return 1; // 满 → 调用方 park_send
    }
    inner.buf.push_back(v);
    let waiter = inner.recv_waiters.pop_front();
    drop(inner); // 先放锁，再 wake（wake 要锁就绪队列，避免嵌套持锁顺序问题）
    if let Some(r) = waiter {
        wake(r);
    }
    0
}

/// 非阻塞接收：0 = 取到（写入 `*slot`）；1 = 空（调用方应 park_recv）。i64 单层直传。
/// 取走后若有协程 park 在 send_waiters（因满而等）→ 补值进缓冲并唤醒它。
///
/// # Safety
/// `ch`/`slot` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_try_recv(ch: *mut QiCoroChan, slot: *mut i64) -> i32 {
    if ch.is_null() || slot.is_null() {
        return 1;
    }
    let chan = unsafe { &*ch };
    let mut inner = chan.inner.lock().unwrap();
    match inner.buf.pop_front() {
        Some(v) => {
            unsafe {
                *slot = v;
            }
            let woken = if let Some((s, sv)) = inner.send_waiters.pop_front() {
                inner.buf.push_back(sv);
                Some(s)
            } else {
                None
            };
            drop(inner);
            if let Some(s) = woken {
                wake(s);
            }
            0
        }
        None => 1,
    }
}

/// R5/R6：收空通道 → 把当前协程 park 进 recv_waiters（锁内重检 race）。
/// 若锁内发现 buf 已可用（并发 send 溜进）→ 不 park（清 PARKED），worker 重排重试。
///
/// # Safety
/// `ch` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_park_recv(ch: *mut QiCoroChan) {
    if ch.is_null() {
        return;
    }
    let chan = unsafe { &*ch };
    let mut inner = chan.inner.lock().unwrap();
    // R6 race 关闭：锁内重检——已有值就别 park（让 worker 重排，下次 try_recv 命中）。
    if !inner.buf.is_empty() {
        return; // PARKED 保持 false → worker requeue → 重试
    }
    let cur = CURRENT.with(|x| x.get());
    if cur.is_null() {
        // ramp 期间：记意图，qi_coro_spawn 建出 QiCoro 后挂到 recv_waiters。
        PARK_INTENT_KIND.with(|k| k.set(1));
        PARK_INTENT_CHAN.with(|c| c.set(ch));
        return;
    }
    inner.recv_waiters.push_back(Cptr(cur));
    PARKED.with(|p| p.set(true));
}

/// R5/R6：发满通道 → 把当前协程连同待发值 park 进 send_waiters（锁内重检 race）。
/// 若锁内发现有空位（并发 recv 腾出）→ 直接入缓冲并唤醒等接收者，不 park。
///
/// # Safety
/// `ch` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_park_send(ch: *mut QiCoroChan, v: i64) {
    if ch.is_null() {
        return;
    }
    let chan = unsafe { &*ch };
    let mut inner = chan.inner.lock().unwrap();
    // R6 race 关闭：锁内重检——有空位就直接发，别 park。
    if inner.cap == 0 || inner.buf.len() < inner.cap {
        inner.buf.push_back(v);
        let waiter = inner.recv_waiters.pop_front();
        drop(inner);
        if let Some(r) = waiter {
            wake(r);
        }
        return; // 已发送，PARKED 保持 false → worker requeue 继续
    }
    let cur = CURRENT.with(|x| x.get());
    if cur.is_null() {
        PARK_INTENT_KIND.with(|k| k.set(2));
        PARK_INTENT_CHAN.with(|c| c.set(ch));
        PARK_INTENT_VAL.with(|x| x.set(v));
        return;
    }
    inner.send_waiters.push_back((Cptr(cur), v));
    PARKED.with(|p| p.set(true));
}

/// 关闭通道（预留）。
///
/// # Safety
/// `ch` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_close(ch: *mut QiCoroChan) {
    if !ch.is_null() {
        unsafe { &*ch }.inner.lock().unwrap().closed = true;
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

/// R2：提前销毁一个未完成的协程（`取消未来`）。从就绪队列摘除 + destroy handle
/// （destroy 走 cleanup 释放 frame 内 RC 槽）。幂等。LIVE--。
/// 注：若协程已 park 在通道等待者列表（非就绪队列），本函数摘不到 —— destroy 后
/// 通道仍持悬挂指针（此边缘留待后续；提前销毁测不涉通道）。
///
/// # Safety
/// `c` 必须是有效 QiCoro 指针（或 null）。
#[no_mangle]
pub extern "C" fn qi_coro_cancel(c: *mut QiCoro) {
    if c.is_null() || unsafe { (*c).done } {
        return;
    }
    READY.lock().unwrap().retain(|x| x.0 != c);
    let ops = ops();
    let hdl = unsafe { (*c).hdl };
    (ops.destroy)(hdl);
    unsafe {
        (*c).done = true;
        (*c).value = 0;
    }
    LIVE.fetch_sub(1, Ordering::AcqRel);
}
