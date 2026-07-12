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
//!
//! ## R7：高扇入抗惊群（work-stealing 本地队列 + IDLE 门控 notify + HANDOFF 死锁修）
//! R6 里所有唤醒都推**全局** READY + 每次 `notify_one`。高扇入（单消费者连续唤醒 5 万
//! 发送者）下：每唤醒一个就 notify 一个睡着的 worker，worker 醒来只干一点点活（发送者
//! 仅剩 `返回`）又睡 —— 5 万次 park/notify 的 futex 往返≈整段耗时（实测 215ms，其中
//! `wait≈4.9 万`）。三处根治：
//! - **每 worker 本地就绪队列 `LOCALQ` + 批量偷取**：worker 内唤醒对端时，NEXT 单槽已占
//!   （高扇入的第 2 个起）就落**本地队列**（自 push 几乎无争用），空闲 worker 从别人队列
//!   一次**偷一半**（摊薄全局锁争用）。取代「全部挤全局 READY 由 12 worker 抢单个」。
//! - **IDLE 门控 notify**：只有真的有 worker park 在 Condvar（`IDLE>0`）才 `notify_one`；
//!   忙碌 worker 自己从队列/偷取抢活 → 免空发 notify 的 futex 风暴。
//! - **HANDOFF 计数修死锁误判**：NEXT 单槽里的在途协程不计入 SCHEDULABLE，R6 靠 2ms 采样
//!   概率躲过「对端刚进 NEXT、RUNNING 归 0」窗口；R7 用 `HANDOFF` 原子精确计数，死锁检测
//!   要求 `RUNNING==0 && HANDOFF==0`，pingpong 不再偶发假死锁。
//! 效果：高扇入 215ms→~30ms（反超 Go 49ms），pingpong/chan/纯创建 无回退。CPU 密集非
//!   调度器瓶颈（大负载实测扩展 5.2×≈Go 5.9×，小负载差距是单核 codegen O1 + turbo）。
//! `QI_CORO_SPIN`（默认 0）保留 park 前自旋旋钮（当前无收益，实验用）。

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

// ───────────────────────── R6 全局调度器状态 ─────────────────────────
/// 全局就绪队列（多 worker 共享，Mutex 提供跨 worker happens-before）。
static READY: Mutex<VecDeque<Cptr>> = Mutex::new(VecDeque::new());
/// 有新就绪协程 / 需要唤醒 worker 时通知。
static CV: Condvar = Condvar::new();
/// 活跃协程数（spawn++ / 完成或取消--）。到 0 → 全部完成。
static LIVE: AtomicI64 = AtomicI64::new(0);
/// 正在被某 worker resume 的协程数（就绪队列取出即 ++，跑完 --）。
static RUNNING: AtomicUsize = AtomicUsize::new(0);
/// 可调度协程总数镜像（全局 READY + 所有 worker 本地队列，不含 NEXT 单槽）。无锁快照：
/// 供 worker 自旋时免锁探测有无新活，也供 push 端判断是否需 notify。
static SCHEDULABLE: AtomicUsize = AtomicUsize::new(0);
/// 正 park 在 Condvar 上的 worker 数（>0 才需 notify，避免 handoff 时空发 notify）。
static IDLE: AtomicUsize = AtomicUsize::new(0);
/// 正停在某 worker NEXT 单槽里的协程数（NEXT 私有不可偷、不计入 SCHEDULABLE）。它们
/// 是「在途可跑」的活 —— 死锁检测必须把它算上，否则 pingpong 里对端刚进 NEXT、持有者
/// 恰好在 RUNNING 归 0 的瞬时窗口会被误判死锁（旧代码靠 2ms 采样概率侥幸躲过）。
static HANDOFF: AtomicUsize = AtomicUsize::new(0);
/// park 前自旋轮数上限（run_all 读 QI_CORO_SPIN 覆盖，默认 0=不自旋）。IDLE 门控 notify +
/// 批量偷取已消除 futex 风暴，自旋反而在过订阅（worker>物理核）时抢核、拖累 pingpong，
/// 故默认关；保留旋钮供实验。
static SPIN_LIMIT: AtomicUsize = AtomicUsize::new(0);

/// worker 本地就绪队列上限（同时也是 run_all 支持的最大 worker 数）。
const MAX_WORKERS: usize = 256;
/// 每 worker 一个本地就绪队列（可被其它 worker 偷取，散播 handoff 密集时的溢出唤醒）。
/// 自 push/pop 走本队列锁（几乎无争用）；偷取时批量搬走一半（摊薄争用）。R6 的单槽 NEXT
/// 仍保留（不可偷，保 pingpong 生产者↔消费者定位）；NEXT 满后的额外唤醒才落本地队列。
static LOCALQ: [Mutex<VecDeque<Cptr>>; MAX_WORKERS] =
    [const { Mutex::new(VecDeque::new()) }; MAX_WORKERS];
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
    /// R6 **直接交接**：运行中的协程唤醒对端时，把对端塞进本 worker 的「下一个」槽而非推
    /// 全局队列+跨线程唤醒 —— 让 pingpong（生产者↔消费者）待在同一线程上，免去每次交接
    /// 的跨线程 condvar 唤醒（通道密集从 1764ms→接近单线程）。pick() 先取此槽。
    static NEXT: Cell<*mut QiCoro> = const { Cell::new(std::ptr::null_mut()) };
    /// R6：死锁误判防护——(队列空+RUNNING=0+LIVE>0) 连续确认 2 次才判死锁，
    /// 避开「协程刚进本地 NEXT、RUNNING 短暂归 0」的瞬时窗口。
    static DEADLOCK_CONFIRM: Cell<i32> = const { Cell::new(0) };
    /// 本 worker 在 LOCALQ 中的下标（0..WORKERS）；usize::MAX = 非 worker 线程
    /// （主线程 spawn 阶段 / 同步 step 上下文），此时唤醒/入队一律走全局 READY。
    static WORKER_ID: Cell<usize> = const { Cell::new(usize::MAX) };
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

/// 入就绪队列 + 唤醒一个等待 worker。notify_one 避免惊群（notify_all 会让 N 个 worker
/// 全醒、N-1 个抢不到活白白再睡 → 通道密集场景 O(N) 浪费）。漏醒由 worker wait 的
/// 2ms 超时兜底（醒来重查队列）。
fn push_ready(c: Cptr) {
    READY.lock().unwrap().push_back(c);
    SCHEDULABLE.fetch_add(1, Ordering::Release);
    // 仅当有 worker 真的 park 在 Condvar 上才 notify。忙碌/自旋中的 worker 会自己
    // 探测 SCHEDULABLE 抢活 —— 免去 handoff 密集场景（高扇入）的 futex 唤醒风暴。
    if IDLE.load(Ordering::Acquire) > 0 {
        CV.notify_one();
    }
}

/// 入本 worker 的本地就绪队列（可被偷）。wake 溢出（NEXT 已占）时用。
fn push_local(wid: usize, c: Cptr) {
    LOCALQ[wid].lock().unwrap().push_back(c);
    SCHEDULABLE.fetch_add(1, Ordering::Release);
    if IDLE.load(Ordering::Acquire) > 0 {
        CV.notify_one();
    }
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
    // **关键修复**：协程可能在 ramp 期间就跑到 final suspend（body 无中途挂起点，如通道
    // 未满/未空、无 让出 —— 一路跑完）。此时 handle 已 done，**绝不能再入就绪队列让 worker
    // resume**（对 done 协程调 llvm.coro.resume 是 UB → 段错误）。直接收值+销毁+返回。
    {
        let o = ops();
        if (o.done)(hdl) {
            let v = (o.promise)(hdl);
            unsafe {
                (*c).value = v;
                (*c).done = true;
            }
            (o.destroy)(hdl);
            // 已完成的 fire-and-forget 协程：不 LIVE++（无需调度）。若被 await，值已就位。
            return c;
        }
    }
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
    // R6 直接交接：先取本 worker 的 NEXT 槽（同线程接力，不进全局队列/不跨线程唤醒）。
    let nx = NEXT.with(|n| n.replace(std::ptr::null_mut()));
    if !nx.is_null() {
        // 先 RUNNING++ 再 HANDOFF--：区间不存在两者同 0 的窗口（协程始终「有主」）。
        RUNNING.fetch_add(1, Ordering::AcqRel);
        HANDOFF.fetch_sub(1, Ordering::AcqRel);
        return Some(Ok(Cptr(nx)));
    }
    let wid = WORKER_ID.with(|w| w.get());
    // 本 worker 本地队列（LIFO 保 handoff 局部性；本队列锁几乎无争用）。
    if wid != usize::MAX {
        if let Some(c) = LOCALQ[wid].lock().unwrap().pop_back() {
            SCHEDULABLE.fetch_sub(1, Ordering::Release);
            RUNNING.fetch_add(1, Ordering::AcqRel);
            return Some(Ok(c));
        }
    }
    // 全局 READY（含定时器：只取到点的；否则记最早时刻，落到偷取后再决定）。
    let mut timer: Option<u64> = None;
    {
        let mut q = READY.lock().unwrap();
        if !q.is_empty() {
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
                    SCHEDULABLE.fetch_sub(1, Ordering::Release);
                    RUNNING.fetch_add(1, Ordering::AcqRel);
                    return Some(Ok(c));
                }
                None => timer = Some(earliest.saturating_sub(now)),
            }
        }
    }
    // 偷取：从其它 worker 的本地队列各偷取一半（批量，摊薄争用）。偷到即跑。
    if wid != usize::MAX {
        let n = WORKERS.load(Ordering::Acquire).min(MAX_WORKERS);
        for off in 1..n {
            let victim = (wid + off) % n;
            if let Ok(mut vq) = LOCALQ[victim].try_lock() {
                let len = vq.len();
                if len == 0 {
                    continue;
                }
                let take = len.div_ceil(2); // 偷一半（向上取整，至少 1）
                                            // 搬 take 个到本地队列，返回其中一个直接跑。
                let mut mine = LOCALQ[wid].lock().unwrap();
                let mut first = None;
                for _ in 0..take {
                    if let Some(c) = vq.pop_front() {
                        if first.is_none() {
                            first = Some(c);
                        } else {
                            mine.push_back(c);
                        }
                    }
                }
                drop(mine);
                drop(vq);
                if let Some(c) = first {
                    // take 个里 1 个直接跑、其余进本地：SCHEDULABLE 只减 1（跑的那个）。
                    SCHEDULABLE.fetch_sub(1, Ordering::Release);
                    RUNNING.fetch_add(1, Ordering::AcqRel);
                    return Some(Ok(c));
                }
            }
        }
    }
    match timer {
        Some(ms) => Some(Err(ms)),
        None => None,
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
            Some(Ok(c)) => {
                DEADLOCK_CONFIRM.with(|d| d.set(0)); // 有活 → 重置死锁确认
                run_one(c);
            }
            Some(Err(sleep_ms)) => {
                DEADLOCK_CONFIRM.with(|d| d.set(0));
                std::thread::sleep(Duration::from_millis(sleep_ms.min(5).max(1)));
            }
            None => {
                // 就绪队列空。先自旋无锁探测（桥接 handoff 空档，免 park/notify futex 风暴），
                // 自旋耗尽仍无活才 park。
                let spin = SPIN_LIMIT.load(Ordering::Relaxed);
                let mut got = false;
                for _ in 0..spin {
                    if SCHEDULABLE.load(Ordering::Acquire) > 0 || SHUTDOWN.load(Ordering::Acquire) {
                        got = true;
                        break;
                    }
                    std::hint::spin_loop();
                }
                if got {
                    continue; // 回去 pick 抢活 / 或退出
                }
                // 自旋耗尽仍无活 → 等待新 work，或判死锁/完成。
                let guard = READY.lock().unwrap();
                // SCHEDULABLE 含本地队列：全局 READY 空但某本地队列仍有活（可偷）→ 别 park。
                if SCHEDULABLE.load(Ordering::Acquire) > 0 || SHUTDOWN.load(Ordering::Acquire) {
                    continue;
                }
                if LIVE.load(Ordering::Acquire) <= 0 {
                    SHUTDOWN.store(true, Ordering::Release);
                    CV.notify_all();
                    return;
                }
                // 队列空 + 无人在跑 + 无在途 handoff + 仍有活协程 → 疑似死锁。HANDOFF 计入
                // NEXT 单槽里的在途协程（否则 pingpong 误判）；再叠加连续确认 2 次防瞬时窗口。
                if RUNNING.load(Ordering::Acquire) == 0 && HANDOFF.load(Ordering::Acquire) == 0 {
                    let n = DEADLOCK_CONFIRM.with(|d| {
                        let v = d.get() + 1;
                        d.set(v);
                        v
                    });
                    if n >= 2 {
                        DEADLOCK.store(true, Ordering::Release);
                        SHUTDOWN.store(true, Ordering::Release);
                        CV.notify_all();
                        return;
                    }
                } else {
                    DEADLOCK_CONFIRM.with(|d| d.set(0));
                }
                // 有 worker 在跑（可能即将 wake 出新 work）→ 真 park。IDLE++ 让 push_ready
                // 知道有人在睡、需要 notify（在锁内改 IDLE，与 push_ready 的 lock 顺序一致，
                // 无丢唤醒）。带超时兜底防错过。
                IDLE.fetch_add(1, Ordering::Release);
                let (guard, _) = CV.wait_timeout(guard, Duration::from_millis(2)).unwrap();
                IDLE.fetch_sub(1, Ordering::Release);
                drop(guard);
            }
        }
    }
}

/// 驱动所有排队 coroutine 直到全部完成。R6：起 N 个 worker 共同排空。
/// N = `QI_CORO_WORKERS`（显式覆盖），否则**默认 = CPU 逻辑核数**（对齐 Go 的
/// GOMAXPROCS=NumCPU，开箱即多核）；设 `QI_CORO_WORKERS=1` 回到单线程。
#[no_mangle]
pub extern "C" fn qi_coro_run_all() {
    let n = std::env::var("QI_CORO_WORKERS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .filter(|&x| x >= 1)
        .map(|x| x.min(MAX_WORKERS))
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|x| x.get())
                .unwrap_or(1)
                .min(MAX_WORKERS)
        });
    if let Some(s) = std::env::var("QI_CORO_SPIN")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
    {
        SPIN_LIMIT.store(s, Ordering::Relaxed);
    }
    SHUTDOWN.store(false, Ordering::Release);
    DEADLOCK.store(false, Ordering::Release);
    WORKERS.store(n, Ordering::Release);

    if n == 1 {
        // 单 worker：调用线程直接跑（与 R5 单线程行为一致，零线程开销）。
        WORKER_ID.with(|w| w.set(0));
        worker_loop();
        WORKER_ID.with(|w| w.set(usize::MAX));
    } else {
        let mut handles = Vec::new();
        for id in 1..n {
            handles.push(std::thread::spawn(move || {
                WORKER_ID.with(|w| w.set(id));
                worker_loop();
            }));
        }
        WORKER_ID.with(|w| w.set(0)); // 调用线程也当一个 worker（id 0）
        worker_loop();
        WORKER_ID.with(|w| w.set(usize::MAX));
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

/// 唤醒一个 parked 协程 —— 立即就绪。R6 直接交接：若本 worker 正在跑协程（CURRENT 非空）
/// 且本地 NEXT 槽空 → 塞进 NEXT，本 worker 接力跑（不跨线程、不推全局队列）；否则推全局队列。
fn wake(c: Cptr) {
    if c.0.is_null() {
        return;
    }
    unsafe {
        (*c.0).wake_at = 0;
    }
    let wid = WORKER_ID.with(|w| w.get());
    if wid != usize::MAX {
        // 本 worker 内唤醒对端：优先塞 NEXT 单槽（不可偷，保 pingpong 生产者↔消费者定位）。
        if NEXT.with(|n| n.get().is_null()) {
            // HANDOFF++ 在 waker 仍 RUNNING 时发生 → 全程 RUNNING>0||HANDOFF>0，死锁检测不误判。
            HANDOFF.fetch_add(1, Ordering::AcqRel);
            NEXT.with(|n| n.set(c.0));
            return;
        }
        // NEXT 已占（如高扇入：单消费者连续唤醒众发送者）→ 落本地队列，空闲 worker 可批量偷。
        push_local(wid, c);
        return;
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
            // 腾出空位 → 提升一个等待发送者的值进 buf。**关键修复（R5 潜藏 bug）**：被提升
            // 的值现在在 buf 里、需要一个消费者来取；若有等待接收者就唤醒一个，否则该值卡在
            // buf、消费者全 park 饿死 —— 此前被测试里 producer 的 让出() 掩盖。同时唤醒发送者继续。
            let mut wakes: Vec<Cptr> = Vec::new();
            if let Some((s, sv)) = inner.send_waiters.pop_front() {
                inner.buf.push_back(sv);
                wakes.push(s);
                if let Some(r) = inner.recv_waiters.pop_front() {
                    wakes.push(r);
                }
            }
            drop(inner);
            for w in wakes {
                wake(w);
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
    {
        let mut q = READY.lock().unwrap();
        let before = q.len();
        q.retain(|x| x.0 != c);
        let removed = before - q.len();
        if removed > 0 {
            SCHEDULABLE.fetch_sub(removed, Ordering::Release);
        }
    }
    let ops = ops();
    let hdl = unsafe { (*c).hdl };
    (ops.destroy)(hdl);
    unsafe {
        (*c).done = true;
        (*c).value = 0;
    }
    LIVE.fetch_sub(1, Ordering::AcqRel);
}
