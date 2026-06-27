//! 信号 FFI — 监听 SIGINT/SIGTERM 让 qi 程序能做优雅关闭。
//!
//! 关键设计：把"挂在 listener 上的 socket fd"登记进一个 lock-free 数组。
//! 收到信号时 handler 对每个登记过的 fd 做 shutdown(2)，立刻把任何阻塞在
//! accept() 上的线程唤醒（accept 返回 -1）。这样 server 主循环就不再
//! 需要"非阻塞 accept + 50ms 睡眠"那种轮询模式。
//!
//! shutdown(2) 在 Linux 和 macOS 都是 async-signal-safe 的。
//! AtomicI32 的 load/store 也是 lock-free 的，handler 里调用安全。

#![allow(non_snake_case)]

use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static SHOULD_SHUTDOWN: AtomicBool = AtomicBool::new(false);

/// 登记 listener fd 用的小固定数组。一个进程同时挂的 listener 通常 ≤ 4 个
/// （HTTP / HTTPS / HTTP2 / WS），16 槽对所有现实场景都够用。
const MAX_LISTENER_FDS: usize = 16;
const NO_FD: i32 = -1;

// 用 AtomicI32 数组而非 Vec<Mutex<...>>，是因为信号 handler 里不能拿锁
// （会导致死锁）。每个槽 0 = 空，否则存活的 fd。
static LISTENER_FDS: [AtomicI32; MAX_LISTENER_FDS] = {
    // 数组初始化常量必须能 const-evaluate；AtomicI32::new 是 const fn
    const INIT: AtomicI32 = AtomicI32::new(NO_FD);
    [INIT; MAX_LISTENER_FDS]
};

extern "C" fn handler(_sig: c_int) {
    SHOULD_SHUTDOWN.store(true, Ordering::SeqCst);
    // 把所有挂着的 listener fd 都 shutdown 掉，让阻塞的 accept() 立即返回 -1。
    // 注意：不是 close —— close 在 handler 里也安全，但 close 后 fd 可能被
    // 别的线程 reuse，造成"我以为我关的是 listener，其实关的是别人新打开的
    // 文件"那种 race。shutdown(SHUT_RDWR) 只断双向 IO 流，fd 仍归 listener
    // 拥有，accept 主线程后续再正常 close 即可。
    for slot in LISTENER_FDS.iter() {
        let fd = slot.load(Ordering::SeqCst);
        if fd >= 0 {
            关停套接字(fd);
        }
    }
}

/// 对一个原始套接字做双向 shutdown，唤醒阻塞的 accept()。
#[cfg(unix)]
#[inline]
fn 关停套接字(fd: i32) {
    unsafe {
        libc::shutdown(fd, libc::SHUT_RDWR);
    }
}

/// Windows：libc 不导出 winsock 的 shutdown，直接链 ws2_32。
/// SD_BOTH = 2；SOCKET 句柄保证可用 32 位表示，i32 → u32 → usize 还原无损。
#[cfg(windows)]
#[inline]
fn 关停套接字(sock: i32) {
    #[link(name = "ws2_32")]
    extern "system" {
        fn shutdown(s: usize, how: i32) -> i32;
    }
    unsafe {
        shutdown(sock as u32 as usize, 2);
    }
}

/// 安装 SIGINT / SIGTERM handler。多次调用安全。
/// 返回 0
#[no_mangle]
pub extern "C" fn qi_signal_install_shutdown() -> i64 {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        libc::signal(libc::SIGINT, handler as libc::sighandler_t);
        libc::signal(libc::SIGTERM, handler as libc::sighandler_t);
    });
    0
}

/// 查询是否收到关闭信号
/// 返回 1 表示应该关闭，0 表示继续
#[no_mangle]
pub extern "C" fn qi_signal_should_shutdown() -> i64 {
    if SHOULD_SHUTDOWN.load(Ordering::SeqCst) {
        1
    } else {
        0
    }
}

/// 重置标志（测试用）
#[no_mangle]
pub extern "C" fn qi_signal_reset() -> i64 {
    SHOULD_SHUTDOWN.store(false, Ordering::SeqCst);
    0
}

/// 把一个 listener 的 fd 登记进信号关停名单。
/// 返回登记到的槽位索引（0..MAX_LISTENER_FDS），失败返回 -1（满了）。
#[no_mangle]
pub extern "C" fn qi_signal_register_listener_fd(fd: i32) -> i64 {
    if fd < 0 {
        return -1;
    }
    for (idx, slot) in LISTENER_FDS.iter().enumerate() {
        // 用 CAS 抢一个空槽
        if slot
            .compare_exchange(NO_FD, fd, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return idx as i64;
        }
    }
    -1
}

/// 取消登记 — listener 关闭后调用，腾出槽位。
#[no_mangle]
pub extern "C" fn qi_signal_unregister_listener_fd(fd: i32) -> i64 {
    if fd < 0 {
        return 0;
    }
    for slot in LISTENER_FDS.iter() {
        if slot
            .compare_exchange(fd, NO_FD, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            return 1;
        }
    }
    0
}
