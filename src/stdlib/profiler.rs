//! 插桩式性能剖析器 —— 函数级 CPU 归因（QI_PROF 编译期门控注入）。
//!
//! 类似简化版 pprof：编译期（QI_PROF=1）在每个用户函数入口/出口注入计时调用，
//! 运行时累计每函数的「总墙钟耗时 + 调用次数」，进程退出（atexit）打排序报告。
//!
//! ## 时间语义：wall-inclusive（墙钟-含子调用）
//!
//! `qi_prof_enter` 记进入时刻，`qi_prof_exit` 算 delta。某函数的「总耗时」是它
//! 每次**顶层**调用 enter→exit 的墙钟时间之和，**包含**其调用的所有子函数耗时
//! （亦包含 profiler 自身插桩开销）。故各函数占比之和 > 100%（父含子重复计入）；
//! 报告以最大值（通常是 入口）为 100% 基准。这是插桩式剖析器的固有语义。
//!
//! ## 递归正确性
//!
//! 同名函数递归时，只在**最外层**出口累计一次墙钟，内层重入不重复计总时间
//! （否则 斐波那契(35) 会把同一段时间计 N 次）。用**线程本地**的每函数递归深度
//! 计数实现：深度归零那次 exit 才累加 total。调用次数则每次 enter 都 +1。
//! 线程本地深度让「同名函数在不同 OS 线程并发」也各自正确（不会互相污染深度）。
//!
//! ## 线程
//!
//! goroutine 在别的 OS 线程跑；累计表是全局 `Mutex<HashMap>`，函数名指针做 key
//! 天然聚合跨线程同名函数。深度是线程本地，互不干扰。
//!
//! ## key = 函数名指针
//!
//! codegen 传入的函数名来自 immortal 全局常量（同名按内容去重 ⇒ 同一指针），
//! 地址进程内稳定且跨线程一致，故直接用指针地址（usize）当 key，热路径零字符串
//! 分配；仅报告时把指针转成 &str 显示。
//!
//! ## 已知限制
//!
//! 异常路径（setjmp/longjmp）跳过正常出口时 `qi_prof_exit` 不执行 —— 被 longjmp
//! 提前退出的函数计时/深度不准（其深度不归零，此后该函数总时间不再累计）。
//! 属可接受的诊断工具限制，不为它加复杂清理。

#![allow(non_snake_case)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 单个函数的累计统计。
struct Stat {
    /// 函数名指针（immortal 全局常量地址，报告时转 &str）。
    name_ptr: usize,
    /// 累计墙钟纳秒（wall-inclusive，仅顶层调用累加）。
    total_ns: u128,
    /// 调用次数（每次 enter 都 +1，含递归重入）。
    calls: u64,
}

/// 全局累计表：函数名指针 → 统计。跨线程共享。
fn table() -> &'static Mutex<HashMap<usize, Stat>> {
    static TABLE: OnceLock<Mutex<HashMap<usize, Stat>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 进程单调基准时刻（首次取样时钉住）。
fn start() -> &'static Instant {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now)
}

#[inline]
fn now_ns() -> i64 {
    start().elapsed().as_nanos() as i64
}

thread_local! {
    /// 每函数在**当前线程**的递归深度（0 表示不在栈上）。
    static DEPTH: RefCell<HashMap<usize, u32>> = RefCell::new(HashMap::new());
}

/// 收到中断信号的标志（信号处理器里只做这一个 async-signal-safe 的原子写）。
static SHUTDOWN: AtomicBool = AtomicBool::new(false);
/// 报告已打印保护（atexit 与信号看门狗竞争时只打一次）。
static REPORTED: AtomicBool = AtomicBool::new(false);

/// SIGINT/SIGTERM 处理器：**仅**置原子标志（async-signal-safe），不在信号上下文里
/// 加锁/打印/退出（那些非 async-signal-safe，易死锁）。由看门狗线程在普通上下文里收尾。
extern "C" fn prof_signal_handler(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

/// 首次 enter 时注册：atexit 报告 + 信号优雅退出看门狗（幂等）。
///
/// 长驻服务（Web 服务器等）正常不 return main，靠 Ctrl-C / kill 结束；信号默认动作
/// 直接终止、**不跑 atexit**，报告就丢了。剖析开启时装 SIGINT/SIGTERM → 置标志，
/// 看门狗线程在普通上下文里 打报告 + process::exit（安全，无 async-signal 风险）。
/// 仅 QI_PROF 编译进的程序会走到；不影响非剖析运行。用户若经 标准库.信号 覆盖了
/// 信号处理器，则看门狗超时兜底不会触发，但正常 return / exit 路径仍由 atexit 打印。
fn maybe_register_report() {
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        // 单元测试里只验累计逻辑，不该给测试进程装 atexit/信号/看门狗（会打出
        // 悬垂名的假报告、抢信号处理器）。shipped staticlib（非 test）照常注册。
        if cfg!(test) {
            return;
        }
        unsafe {
            libc::atexit(qi_prof_report);
            libc::signal(libc::SIGINT, prof_signal_handler as libc::sighandler_t);
            libc::signal(libc::SIGTERM, prof_signal_handler as libc::sighandler_t);
        }
        // 看门狗：普通线程里轮询关停标志，收到即 打报告 + 退出。
        // 同时看两处：① 本模块 SHUTDOWN（非服务型长驻程序，我的 handler 置）；
        // ② signal_ffi 的 qi_signal_should_shutdown（Web 框架 serve 时装的关停 handler
        // 会覆盖我的 handler 并置此标志）—— 这样无论谁抢到信号处理器，都能收尾报告，
        // 不与框架的信号机制争抢。
        std::thread::Builder::new()
            .name("qi-prof-watchdog".into())
            .spawn(|| loop {
                let 关停 = SHUTDOWN.load(Ordering::SeqCst)
                    || crate::stdlib::signal_ffi::qi_signal_should_shutdown() != 0;
                if 关停 {
                    qi_prof_report();
                    std::process::exit(130);
                }
                std::thread::sleep(Duration::from_millis(50));
            })
            .ok();
    });
}

/// 函数入口：记调用次数 + 递归深度 +1，返回进入时刻（纳秒）。
///
/// # Safety
/// `name` 必须是有效的、进程存活期不失效的 C 字符串指针
/// （codegen 传入的 immortal 全局常量）。
#[no_mangle]
pub extern "C" fn qi_prof_enter(name: *const c_char) -> i64 {
    if name.is_null() {
        return now_ns();
    }
    maybe_register_report();
    let key = name as usize;
    DEPTH.with(|d| {
        *d.borrow_mut().entry(key).or_insert(0) += 1;
    });
    if let Ok(mut t) = table().lock() {
        let e = t.entry(key).or_insert(Stat {
            name_ptr: key,
            total_ns: 0,
            calls: 0,
        });
        e.calls += 1;
    }
    now_ns()
}

/// 函数出口：递归深度 -1；归零时把本次顶层调用的 delta 累加进总耗时。
///
/// # Safety
/// `name` 同 [`qi_prof_enter`]；`enter_ts` 必须是配对 enter 的返回值。
#[no_mangle]
pub extern "C" fn qi_prof_exit(name: *const c_char, enter_ts: i64) {
    let now = now_ns();
    if name.is_null() {
        return;
    }
    let key = name as usize;
    let 归零 = DEPTH.with(|d| {
        let mut m = d.borrow_mut();
        let e = m.entry(key).or_insert(0);
        if *e > 0 {
            *e -= 1;
        }
        *e == 0
    });
    if 归零 {
        let delta = (now - enter_ts).max(0) as u128;
        if let Ok(mut t) = table().lock() {
            if let Some(e) = t.get_mut(&key) {
                e.total_ns += delta;
            }
        }
    }
}

/// atexit 报告：按总耗时降序打印每函数的 调用次数 / 总耗时 / 均耗时 / 占比。
///
/// 也可被 codegen 显式调用；重复调用只是重复打印，无副作用。
#[no_mangle]
pub extern "C" fn qi_prof_report() {
    // 只打印一次（atexit 与信号看门狗可能都调到）。
    if REPORTED.swap(true, Ordering::SeqCst) {
        return;
    }
    // 运行时双保险：显式 QI_PROF=0 时静默（正常场景 QI_PROF=1 一同设）。
    if std::env::var("QI_PROF")
        .map(|v| v == "0" || v.eq_ignore_ascii_case("false"))
        .unwrap_or(false)
    {
        return;
    }
    let 表 = match table().lock() {
        Ok(t) => t,
        Err(_) => return,
    };
    if 表.is_empty() {
        return;
    }
    let mut rows: Vec<&Stat> = 表.values().collect();
    rows.sort_by(|a, b| b.total_ns.cmp(&a.total_ns));
    let 基准 = rows.first().map(|s| s.total_ns).unwrap_or(0).max(1);

    eprintln!("=== Qi Profiler (wall-inclusive) ===");
    eprintln!(
        "{:<28} {:>12} {:>12} {:>12} {:>8}",
        "函数", "调用次数", "总耗时(ms)", "每次(µs)", "占比%"
    );
    for s in rows {
        let 名 = unsafe { CStr::from_ptr(s.name_ptr as *const c_char) }
            .to_string_lossy()
            .into_owned();
        let 总ms = s.total_ns as f64 / 1_000_000.0;
        let 均us = if s.calls > 0 {
            s.total_ns as f64 / 1_000.0 / s.calls as f64
        } else {
            0.0
        };
        let 占比 = s.total_ns as f64 / 基准 as f64 * 100.0;
        // 中文名按显示宽度粗对齐（每中文字≈2 列，Rust 的 {:<28} 按字符数不准，
        // 但足够可读）。
        eprintln!(
            "{:<28} {:>12} {:>12.3} {:>12.3} {:>7.1}%",
            名, s.calls, 总ms, 均us, 占比
        );
    }
    eprintln!("=== (占比以最大值为 100% 基准；wall-inclusive 含子调用与插桩开销) ===");
}

/// 测试用：读某函数名指针的 (调用次数, 总纳秒)。
#[cfg(test)]
pub fn prof_stat(name: *const c_char) -> Option<(u64, u128)> {
    let t = table().lock().ok()?;
    t.get(&(name as usize)).map(|s| (s.calls, s.total_ns))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn 基础_累计与调用次数() {
        let n = CString::new("测试函数A").unwrap();
        let p = n.as_ptr();
        for _ in 0..5 {
            let t = qi_prof_enter(p);
            // 制造一点耗时
            std::thread::sleep(std::time::Duration::from_micros(50));
            qi_prof_exit(p, t);
        }
        let (calls, total) = prof_stat(p).expect("应有统计");
        assert_eq!(calls, 5);
        assert!(total > 0, "总耗时应 > 0");
    }

    #[test]
    fn 递归_只在最外层累计() {
        let n = CString::new("测试递归B").unwrap();
        let p = n.as_ptr();
        // 手工模拟深度 3 的递归：enter enter enter exit exit exit
        let t1 = qi_prof_enter(p);
        std::thread::sleep(std::time::Duration::from_micros(20));
        let t2 = qi_prof_enter(p);
        std::thread::sleep(std::time::Duration::from_micros(20));
        let t3 = qi_prof_enter(p);
        std::thread::sleep(std::time::Duration::from_micros(20));
        qi_prof_exit(p, t3);
        qi_prof_exit(p, t2);
        qi_prof_exit(p, t1);
        let (calls, total) = prof_stat(p).expect("应有统计");
        assert_eq!(calls, 3, "3 次调用");
        // 只累计最外层一次墙钟 ≈ 60µs，不是三层相加的 120µs。
        // 宽松上界：远小于「每层都计」的和。
        assert!(total >= 40_000, "至少最外层墙钟");
        assert!(total < 200_000_000, "只计最外层一次（不因递归重复膨胀）");
    }
}
