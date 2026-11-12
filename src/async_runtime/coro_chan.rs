//! 协程原生通道（R3-R6 + 线程阻塞收发）。
//!
//! 单线程/多 worker 共用；内部状态 Mutex 保护（R6）。值按裸 i64 位模式直传（不 box）。
//! 收空/发满 → 协程 park 进等待者列表；对端操作 wake 一个（移回就绪队列）。
//! **park/wake race（R6 多线程）**：try_recv 空 → park_recv 之间可能有并发 send 溜进，
//! 故 park_recv/park_send 在锁内**重新检查**：已可用则不 park，worker 重排重试。
//!
//! ## 非协程上下文（`启动` 出来的 goroutine 线程、入口、普通函数）
//! 这些地方没有协程可挂起，只能阻塞线程。以前 codegen 在这里调 `try_send` 并**丢弃**
//! 「满」的返回值 —— 带缓冲通道一写满，后续值直接丢失，接收方永远等不到（整个程序挂住）。
//! 现在走 `qi_coro_chan_send_blocking` / `qi_coro_chan_recv_blocking`：
//! - 条件不满足时先帮着驱动一个就绪协程（对端可能是还没人跑的协程；只在本线程不处于
//!   协程 resume 中时才驱动，避免嵌套 resume 打乱 CURRENT）；
//! - 没协程可驱动就在通道自己的 Condvar 上等，对端每次改动缓冲都会 notify
//!   （`thread_waiters > 0` 才 notify，纯协程路径零额外开销）；超时只是兜底。

use super::{pick, run_one, PARK_INTENT_VAL};
use super::{wake, Cptr, CURRENT, LIVE, PARKED, PARK_INTENT_CHAN, PARK_INTENT_KIND};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Condvar, Mutex, MutexGuard};
use std::time::Duration;

pub(super) struct ChanInner {
    pub(super) buf: VecDeque<i64>,
    pub(super) cap: usize, // 0 = 无界
    closed: bool,
    pub(super) recv_waiters: VecDeque<Cptr>,
    pub(super) send_waiters: VecDeque<(Cptr, i64)>,
    /// 阻塞在 `cv` 上的线程数（goroutine 线程 / 入口）。>0 才需要 notify。
    thread_waiters: usize,
}

impl ChanInner {
    fn full(&self) -> bool {
        self.cap != 0 && self.buf.len() >= self.cap
    }
}

/// 协程原生通道对象。`#[repr(C)]` 供 codegen 当句柄传递（内容只经 FFI 访问）。
#[repr(C)]
pub struct QiCoroChan {
    pub(super) inner: Mutex<ChanInner>,
    /// 线程侧（非协程）阻塞收发在这里等。
    cv: Condvar,
}

/// 放锁后统一处理：唤醒协程 + 通知阻塞线程。先放锁再 wake（wake 要锁就绪队列）。
fn finish(chan: &QiCoroChan, inner: MutexGuard<'_, ChanInner>, wakes: &[Cptr]) {
    let notify = inner.thread_waiters > 0;
    drop(inner);
    if notify {
        chan.cv.notify_all();
    }
    for w in wakes {
        wake(*w);
    }
}

/// 有空位就入队（并唤醒一个等接收者）；满返回 false，锁还给调用方。
fn send_locked<'a>(
    chan: &'a QiCoroChan,
    mut inner: MutexGuard<'a, ChanInner>,
    v: i64,
) -> Result<(), MutexGuard<'a, ChanInner>> {
    if inner.full() {
        return Err(inner);
    }
    inner.buf.push_back(v);
    let waiter = inner.recv_waiters.pop_front();
    match waiter {
        Some(r) => finish(chan, inner, &[r]),
        None => finish(chan, inner, &[]),
    }
    Ok(())
}

/// 有值就取（写 `*slot`）；空返回 false，锁还给调用方。
/// 取走后若有协程 park 在 send_waiters（因满而等）→ 补值进缓冲并唤醒它。
fn recv_locked<'a>(
    chan: &'a QiCoroChan,
    mut inner: MutexGuard<'a, ChanInner>,
    slot: *mut i64,
) -> Result<(), MutexGuard<'a, ChanInner>> {
    let Some(v) = inner.buf.pop_front() else {
        return Err(inner);
    };
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
    finish(chan, inner, &wakes);
    Ok(())
}

/// ramp 期间记下的 park 意图落地（qi_coro_spawn 建出 QiCoro 后调）。锁内重检：
/// 已可收/可发就不挂等待者 —— 发的那种直接把值放进缓冲 —— 返回 false，调用方
/// 把协程推进就绪队列；真挂上了返回 true。
pub(super) fn attach_parked_intent(ch: *mut QiCoroChan, kind: i32, c: Cptr, v: i64) -> bool {
    let chan = unsafe { &*ch };
    let mut inner = chan.inner.lock().unwrap();
    if kind == 1 {
        if !inner.buf.is_empty() {
            return false;
        }
        inner.recv_waiters.push_back(c);
        true
    } else {
        match send_locked(chan, inner, v) {
            Ok(()) => false,
            Err(mut inner) => {
                inner.send_waiters.push_back((c, v));
                true
            }
        }
    }
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
            thread_waiters: 0,
        }),
        cv: Condvar::new(),
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
    match send_locked(chan, chan.inner.lock().unwrap(), v) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// 非阻塞接收：0 = 取到（写入 `*slot`）；1 = 空（调用方应 park_recv）。i64 单层直传。
///
/// # Safety
/// `ch`/`slot` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_try_recv(ch: *mut QiCoroChan, slot: *mut i64) -> i32 {
    if ch.is_null() || slot.is_null() {
        return 1;
    }
    let chan = unsafe { &*ch };
    match recv_locked(chan, chan.inner.lock().unwrap(), slot) {
        Ok(()) => 0,
        Err(_) => 1,
    }
}

/// 本线程不在协程 resume 里时，帮着跑一个到点的就绪协程。跑了返回 true。
fn drive_one() -> bool {
    if !CURRENT.with(|x| x.get()).is_null() {
        return false;
    }
    match pick() {
        Some(Ok(c)) => {
            run_one(c);
            true
        }
        _ => false,
    }
}

/// 条件不满足时的一次等待：有活协程时短等（它们可能要靠本线程驱动），否则等对端 notify。
fn wait_turn<'a>(
    chan: &'a QiCoroChan,
    mut inner: MutexGuard<'a, ChanInner>,
) -> MutexGuard<'a, ChanInner> {
    let ms = if LIVE.load(Ordering::Acquire) > 0 {
        1
    } else {
        20
    };
    inner.thread_waiters += 1;
    let (mut inner, _) = chan
        .cv
        .wait_timeout(inner, Duration::from_millis(ms))
        .unwrap();
    inner.thread_waiters -= 1;
    inner
}

/// 非协程上下文的阻塞发送（Go 语义）：有界通道满 → 阻塞到接收方腾出空位。恒返回 0。
///
/// # Safety
/// `ch` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_send_blocking(ch: *mut QiCoroChan, v: i64) -> i32 {
    if ch.is_null() {
        return 1;
    }
    let chan = unsafe { &*ch };
    loop {
        let inner = match send_locked(chan, chan.inner.lock().unwrap(), v) {
            Ok(()) => return 0,
            Err(inner) => inner,
        };
        drop(inner);
        if drive_one() {
            continue;
        }
        let inner = chan.inner.lock().unwrap();
        if !inner.full() {
            continue;
        }
        drop(wait_turn(chan, inner));
    }
}

/// 非协程上下文的阻塞接收：空 → 阻塞到有值。恒返回 0（值写入 `*slot`）。
///
/// # Safety
/// `ch`/`slot` 为 null 或有效指针。
#[no_mangle]
pub extern "C" fn qi_coro_chan_recv_blocking(ch: *mut QiCoroChan, slot: *mut i64) -> i32 {
    if ch.is_null() || slot.is_null() {
        return 1;
    }
    let chan = unsafe { &*ch };
    loop {
        let inner = match recv_locked(chan, chan.inner.lock().unwrap(), slot) {
            Ok(()) => return 0,
            Err(inner) => inner,
        };
        drop(inner);
        if drive_one() {
            continue;
        }
        let inner = chan.inner.lock().unwrap();
        if !inner.buf.is_empty() {
            continue;
        }
        drop(wait_turn(chan, inner));
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
    // R6 race 关闭：锁内重检——有空位就直接发，别 park。
    let mut inner = match send_locked(chan, chan.inner.lock().unwrap(), v) {
        Ok(()) => return, // 已发送，PARKED 保持 false → worker requeue 继续
        Err(inner) => inner,
    };
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 线程侧阻塞收发：容量 2 的通道，生产线程发 1000 个，本线程收 —— 写满时
    /// 发送方必须等，不能丢值（以前非协程上下文满了就丢，接收方永远等不到）。
    #[test]
    fn blocking_send_waits_for_space() {
        let ch = qi_coro_chan_new(2);
        let addr = ch as usize;
        let producer = std::thread::spawn(move || {
            for i in 0..1000 {
                assert_eq!(qi_coro_chan_send_blocking(addr as *mut QiCoroChan, i), 0);
            }
        });
        let mut sum = 0i64;
        let mut slot = 0i64;
        for _ in 0..1000 {
            assert_eq!(qi_coro_chan_recv_blocking(ch, &mut slot), 0);
            sum += slot;
            assert!(unsafe { &*ch }.inner.lock().unwrap().buf.len() <= 2);
        }
        producer.join().unwrap();
        assert_eq!(sum, 999 * 1000 / 2);
        qi_coro_chan_free(ch);
    }

    /// 满通道上 try_send 返回 1 且不入队。
    #[test]
    fn try_send_reports_full() {
        let ch = qi_coro_chan_new(1);
        assert_eq!(qi_coro_chan_try_send(ch, 7), 0);
        assert_eq!(qi_coro_chan_try_send(ch, 8), 1);
        let mut slot = 0i64;
        assert_eq!(qi_coro_chan_try_recv(ch, &mut slot), 0);
        assert_eq!(slot, 7);
        assert_eq!(qi_coro_chan_try_recv(ch, &mut slot), 1);
        qi_coro_chan_free(ch);
    }
}
