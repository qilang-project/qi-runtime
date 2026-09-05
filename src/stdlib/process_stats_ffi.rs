//! 进程自身运行统计 FFI —— 常驻内存 / 峰值内存 / CPU 时间 / 线程数 / 打开文件数 /
//! 运行时长，外加运行时内部的几个存活计数（协程、JSON 句柄、邮箱）。
//!
//! 给 `标准库.进程统计` 模块用；qi-web 的 运行统计.qi 拿它喂 Prometheus 和 /stats。
//!
//! ── 平台覆盖 ────────────────────────────────────────────────────
//!
//! | 指标        | macOS                          | Linux                     | Windows                     |
//! |-------------|--------------------------------|---------------------------|-----------------------------|
//! | rss         | task_info(MACH_TASK_BASIC_INFO) | /proc/self/statm × 页大小 | GetProcessMemoryInfo        |
//! | peak rss    | getrusage ru_maxrss（字节）     | getrusage ru_maxrss（KB） | PeakWorkingSetSize          |
//! | cpu user/sys| getrusage                       | getrusage                 | GetProcessTimes             |
//! | 线程数      | proc_pidinfo(PROC_PIDTASKINFO)  | /proc/self/status Threads | Toolhelp32 线程快照         |
//! | 打开 fd     | proc_pidinfo(PROC_PIDLISTFDS)   | 数 /proc/self/fd          | GetProcessHandleCount（句柄）|
//! | 运行时长    | proc_pidinfo(PROC_PIDTBSDINFO) 启动时刻 | /proc/self/stat starttime | GetProcessTimes 创建时刻 |
//!
//! 运行时长优先问操作系统「这个进程是什么时候起的」，而不是靠运行时初始化钩子 ——
//! 生成的可执行文件的 main 并不会调 qi_runtime_initialize（只有 `qi 导出` 的动态库
//! 走 global_ctors 那条路），首次调用时刻才起表的话，第一次抓 /metrics 之前的
//! 时间就全丢了。操作系统那条路失败时才退化成「首次调用时刻」（FIRST_SEEN）。
//!
//! 每个函数都不会 panic：拿不到就返回 0，调用方按「没有」处理。

use std::os::raw::c_char;
use std::sync::OnceLock;
use std::time::Instant;

use crate::stdlib::qi_str::rc_cstr_from_string;

/// 退化用的起点：操作系统查不到进程启动时刻时，按第一次问到这个模块的时刻算。
static FIRST_SEEN: OnceLock<Instant> = OnceLock::new();

fn note_first_seen() -> Instant {
    *FIRST_SEEN.get_or_init(Instant::now)
}

// ─────────────────────────── 通用（unix）───────────────────────────

#[cfg(unix)]
fn rusage_self() -> Option<libc::rusage> {
    // SAFETY: rusage 是纯 POD，零初始化合法；getrusage 只写入我们传的那块。
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    let rc = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) };
    if rc == 0 {
        Some(usage)
    } else {
        None
    }
}

// tv_sec / tv_usec 的宽度随 libc 目标变（32 位上是 i32），显式 as i64 不是多余的
#[cfg(unix)]
#[allow(clippy::unnecessary_cast)]
fn timeval_ms(tv: libc::timeval) -> i64 {
    tv.tv_sec as i64 * 1000 + tv.tv_usec as i64 / 1000
}

// ─────────────────────────── macOS ───────────────────────────

// libc 把 mach_task_self() 标成 deprecated（建议改用 mach2 crate）；这儿就这一处
// mach 调用，不为它多拉一个依赖。
#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn task_basic_info() -> Option<libc::mach_task_basic_info> {
    // SAFETY: 结构体是 POD；task_info 按 count 写入，count 用官方常量。
    unsafe {
        let mut info: libc::mach_task_basic_info = std::mem::zeroed();
        let mut count: libc::mach_msg_type_number_t = libc::MACH_TASK_BASIC_INFO_COUNT;
        let kr = libc::task_info(
            libc::mach_task_self(),
            libc::MACH_TASK_BASIC_INFO,
            &mut info as *mut _ as libc::task_info_t,
            &mut count,
        );
        if kr == libc::KERN_SUCCESS {
            Some(info)
        } else {
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn proc_task_info() -> Option<libc::proc_taskinfo> {
    // SAFETY: proc_taskinfo 是 POD；proc_pidinfo 返回写入的字节数，不足即失败。
    unsafe {
        let mut info: libc::proc_taskinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
        let n = libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        if n == size {
            Some(info)
        } else {
            None
        }
    }
}

#[cfg(target_os = "macos")]
fn platform_rss_bytes() -> i64 {
    task_basic_info()
        .map(|i| i.resident_size as i64)
        .or_else(|| proc_task_info().map(|i| i.pti_resident_size as i64))
        .unwrap_or(0)
}

// ru_maxrss 是 c_long，32 位目标上是 i32；显式 as i64 不是多余的
#[cfg(target_os = "macos")]
#[allow(clippy::unnecessary_cast)]
fn platform_peak_rss_bytes() -> i64 {
    // macOS 的 ru_maxrss 已经是字节
    rusage_self().map(|u| u.ru_maxrss as i64).unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn platform_thread_count() -> i64 {
    proc_task_info()
        .map(|i| i.pti_threadnum as i64)
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn platform_open_fd_count() -> i64 {
    // 两步：先问需要多大的缓冲，再真取一次按结构体大小数个数。
    // 只用第一步的字节数除大小也行，但那是「上限估计」，中间有 fd 关掉就多数了。
    unsafe {
        let pid = libc::getpid();
        let needed = libc::proc_pidinfo(pid, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0);
        if needed <= 0 {
            return 0;
        }
        let one = std::mem::size_of::<libc::proc_fdinfo>();
        // 留一点余量：两次调用之间可能又开了几个
        let cap = needed as usize + 16 * one;
        let mut buf: Vec<libc::proc_fdinfo> = Vec::with_capacity(cap / one + 1);
        let got = libc::proc_pidinfo(
            pid,
            libc::PROC_PIDLISTFDS,
            0,
            buf.as_mut_ptr() as *mut libc::c_void,
            (buf.capacity() * one) as libc::c_int,
        );
        if got <= 0 {
            return 0;
        }
        (got as usize / one) as i64
    }
}

#[cfg(target_os = "macos")]
fn platform_start_epoch_ms() -> Option<i64> {
    // SAFETY: proc_bsdinfo 是 POD；返回值等于结构体大小才算成功。
    unsafe {
        let mut info: libc::proc_bsdinfo = std::mem::zeroed();
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let n = libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTBSDINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        );
        if n != size {
            return None;
        }
        Some(info.pbi_start_tvsec as i64 * 1000 + info.pbi_start_tvusec as i64 / 1000)
    }
}

#[cfg(target_os = "macos")]
fn platform_uptime_ms() -> Option<i64> {
    let start = platform_start_epoch_ms()?;
    let now = epoch_ms_now()?;
    Some((now - start).max(0))
}

#[cfg(target_os = "macos")]
fn platform_cpu_ms() -> (i64, i64) {
    rusage_self()
        .map(|u| (timeval_ms(u.ru_utime), timeval_ms(u.ru_stime)))
        .unwrap_or((0, 0))
}

// ─────────────────────────── Linux ───────────────────────────

#[cfg(target_os = "linux")]
fn page_size() -> i64 {
    let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    if n > 0 {
        n as i64
    } else {
        4096
    }
}

#[cfg(target_os = "linux")]
fn platform_rss_bytes() -> i64 {
    // /proc/self/statm：size resident shared text lib data dt（单位都是页）
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|s| {
            s.split_whitespace()
                .nth(1)
                .and_then(|v| v.parse::<i64>().ok())
        })
        .map(|pages| pages * page_size())
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
#[allow(clippy::unnecessary_cast)]
fn platform_peak_rss_bytes() -> i64 {
    // Linux 的 ru_maxrss 是 KB
    rusage_self()
        .map(|u| u.ru_maxrss as i64 * 1024)
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn platform_thread_count() -> i64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Threads:"))
                .and_then(|l| l["Threads:".len()..].trim().parse::<i64>().ok())
        })
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn platform_open_fd_count() -> i64 {
    // read_dir 自己也占一个 fd，会把自己数进去；这个 ±1 对「fd 泄漏了没有」
    // 这种趋势判断没有影响，不去扣它 —— 扣了反而在 fd 被别的线程同时关掉时出负数。
    std::fs::read_dir("/proc/self/fd")
        .map(|d| d.count() as i64)
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn platform_uptime_ms() -> Option<i64> {
    // /proc/self/stat 的第 22 个字段是 starttime（开机以来的 clock tick）。
    // 第 2 个字段 comm 带括号且可能含空格，所以先按最后一个 ')' 切，再数字段。
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    let after = &stat[stat.rfind(')')? + 1..];
    // after 里第一个字段是 state（原第 3 个），所以 starttime 是 after 里的第 20 个（下标 19）
    let start_ticks: i64 = after.split_whitespace().nth(19)?.parse().ok()?;
    let clk_tck = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if clk_tck <= 0 {
        return None;
    }
    let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
    let boot_secs: f64 = uptime.split_whitespace().next()?.parse().ok()?;
    let start_ms = start_ticks * 1000 / clk_tck as i64;
    Some(((boot_secs * 1000.0) as i64 - start_ms).max(0))
}

#[cfg(target_os = "linux")]
fn platform_cpu_ms() -> (i64, i64) {
    rusage_self()
        .map(|u| (timeval_ms(u.ru_utime), timeval_ms(u.ru_stime)))
        .unwrap_or((0, 0))
}

// ─────────────────────────── Windows ───────────────────────────

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Thread32First, Thread32Next, TH32CS_SNAPTHREAD, THREADENTRY32,
    };
    use windows_sys::Win32::System::ProcessStatus::{
        GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentProcessId, GetProcessHandleCount, GetProcessTimes,
    };

    fn memory_counters() -> Option<PROCESS_MEMORY_COUNTERS> {
        // SAFETY: POD 结构体，cb 填好后由系统按大小写入。
        unsafe {
            let mut c: PROCESS_MEMORY_COUNTERS = std::mem::zeroed();
            c.cb = std::mem::size_of::<PROCESS_MEMORY_COUNTERS>() as u32;
            if GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) != 0 {
                Some(c)
            } else {
                None
            }
        }
    }

    pub fn rss_bytes() -> i64 {
        memory_counters()
            .map(|c| c.WorkingSetSize as i64)
            .unwrap_or(0)
    }

    pub fn peak_rss_bytes() -> i64 {
        memory_counters()
            .map(|c| c.PeakWorkingSetSize as i64)
            .unwrap_or(0)
    }

    fn filetime_100ns(ft: FILETIME) -> i64 {
        ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
    }

    /// (创建时刻 100ns, 内核时间 100ns, 用户时间 100ns)
    fn process_times() -> Option<(i64, i64, i64)> {
        unsafe {
            let mut creation: FILETIME = std::mem::zeroed();
            let mut exit: FILETIME = std::mem::zeroed();
            let mut kernel: FILETIME = std::mem::zeroed();
            let mut user: FILETIME = std::mem::zeroed();
            if GetProcessTimes(
                GetCurrentProcess(),
                &mut creation,
                &mut exit,
                &mut kernel,
                &mut user,
            ) == 0
            {
                return None;
            }
            Some((
                filetime_100ns(creation),
                filetime_100ns(kernel),
                filetime_100ns(user),
            ))
        }
    }

    pub fn cpu_ms() -> (i64, i64) {
        process_times()
            .map(|(_, k, u)| (u / 10_000, k / 10_000))
            .unwrap_or((0, 0))
    }

    /// FILETIME 纪元是 1601-01-01，与 Unix 纪元差 11644473600 秒。
    pub fn start_epoch_ms() -> Option<i64> {
        process_times().map(|(c, _, _)| c / 10_000 - 11_644_473_600_000)
    }

    pub fn thread_count() -> i64 {
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return 0;
            }
            let me = GetCurrentProcessId();
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut n: i64 = 0;
            if Thread32First(snap, &mut entry) != 0 {
                loop {
                    if entry.th32OwnerProcessID == me {
                        n += 1;
                    }
                    if Thread32Next(snap, &mut entry) == 0 {
                        break;
                    }
                }
            }
            CloseHandle(snap);
            n
        }
    }

    /// Windows 没有 fd 的概念，给的是**内核对象句柄数**（文件/套接字/事件/线程…都算）。
    /// 趋势含义一样：只涨不跌就是在漏。
    pub fn handle_count() -> i64 {
        unsafe {
            let mut n: u32 = 0;
            if GetProcessHandleCount(GetCurrentProcess(), &mut n) != 0 {
                n as i64
            } else {
                0
            }
        }
    }
}

#[cfg(windows)]
fn platform_rss_bytes() -> i64 {
    win::rss_bytes()
}
#[cfg(windows)]
fn platform_peak_rss_bytes() -> i64 {
    win::peak_rss_bytes()
}
#[cfg(windows)]
fn platform_thread_count() -> i64 {
    win::thread_count()
}
#[cfg(windows)]
fn platform_open_fd_count() -> i64 {
    win::handle_count()
}
#[cfg(windows)]
fn platform_cpu_ms() -> (i64, i64) {
    win::cpu_ms()
}
#[cfg(windows)]
fn platform_uptime_ms() -> Option<i64> {
    let start = win::start_epoch_ms()?;
    let now = epoch_ms_now()?;
    Some((now - start).max(0))
}

// ─────────────────────────── 其他 unix（FreeBSD 等）：只有 getrusage ───────────────────────────

#[cfg(all(unix, not(target_os = "macos"), not(target_os = "linux")))]
fn platform_rss_bytes() -> i64 {
    0
}
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "linux")))]
#[allow(clippy::unnecessary_cast)]
fn platform_peak_rss_bytes() -> i64 {
    rusage_self()
        .map(|u| u.ru_maxrss as i64 * 1024)
        .unwrap_or(0)
}
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "linux")))]
fn platform_thread_count() -> i64 {
    0
}
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "linux")))]
fn platform_open_fd_count() -> i64 {
    0
}
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "linux")))]
fn platform_cpu_ms() -> (i64, i64) {
    rusage_self()
        .map(|u| (timeval_ms(u.ru_utime), timeval_ms(u.ru_stime)))
        .unwrap_or((0, 0))
}
#[cfg(all(unix, not(target_os = "macos"), not(target_os = "linux")))]
fn platform_uptime_ms() -> Option<i64> {
    None
}

// ─────────────────────────── 既不是 unix 也不是 windows（wasm 等）───────────────────────────

#[cfg(not(any(unix, windows)))]
fn platform_rss_bytes() -> i64 {
    0
}
#[cfg(not(any(unix, windows)))]
fn platform_peak_rss_bytes() -> i64 {
    0
}
#[cfg(not(any(unix, windows)))]
fn platform_thread_count() -> i64 {
    0
}
#[cfg(not(any(unix, windows)))]
fn platform_open_fd_count() -> i64 {
    0
}
#[cfg(not(any(unix, windows)))]
fn platform_cpu_ms() -> (i64, i64) {
    (0, 0)
}
#[cfg(not(any(unix, windows)))]
fn platform_uptime_ms() -> Option<i64> {
    None
}

// ─────────────────────────── 公共小工具 ───────────────────────────

#[allow(dead_code)]
fn epoch_ms_now() -> Option<i64> {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64)
}

fn uptime_ms() -> i64 {
    let first = note_first_seen();
    match platform_uptime_ms() {
        Some(ms) if ms > 0 => ms,
        _ => first.elapsed().as_millis() as i64,
    }
}

// ─────────────────────────── FFI ───────────────────────────

/// 常驻内存（RSS）字节数；拿不到返回 0
#[no_mangle]
pub extern "C" fn qi_proc_rss_bytes() -> i64 {
    note_first_seen();
    platform_rss_bytes()
}

/// 进程生命周期内的峰值常驻内存字节数（各平台已统一成字节）
#[no_mangle]
pub extern "C" fn qi_proc_peak_rss_bytes() -> i64 {
    note_first_seen();
    platform_peak_rss_bytes()
}

/// 用户态 CPU 累计毫秒
#[no_mangle]
pub extern "C" fn qi_proc_cpu_user_ms() -> i64 {
    note_first_seen();
    platform_cpu_ms().0
}

/// 内核态 CPU 累计毫秒
#[no_mangle]
pub extern "C" fn qi_proc_cpu_sys_ms() -> i64 {
    note_first_seen();
    platform_cpu_ms().1
}

/// 进程当前线程数（含 tokio worker / blocking pool）；拿不到返回 0
#[no_mangle]
pub extern "C" fn qi_proc_thread_count() -> i64 {
    note_first_seen();
    platform_thread_count()
}

/// 打开的文件描述符数（Windows 上是内核对象句柄数）；拿不到返回 0
#[no_mangle]
pub extern "C" fn qi_proc_open_fd_count() -> i64 {
    note_first_seen();
    platform_open_fd_count()
}

/// 进程已运行毫秒数。优先按操作系统记录的进程启动时刻算；查不到则按
/// 本模块首次被调用的时刻算。
#[no_mangle]
pub extern "C" fn qi_proc_uptime_ms() -> i64 {
    uptime_ms()
}

/// 一次性把所有统计拼成 JSON 对象文本（键全英文，见下）。
/// 返回的字符串走 RC 约定，用 释放字符串 / qi_os_free_string 释放。
///
/// 除了上面几个 OS 级指标，还带运行时内部的存活计数：
/// - goroutines：`启动` 出去、还没跑完的协程（tokio blocking pool 那条路）
/// - coroutines：QI_CORO 真协程调度器里的活跃协程（不开 QI_CORO 恒为 0）
/// - json_handles：标准库.JSON 句柄池里还没 删除 的对象数（涨不停 = 忘了释放）
/// - mailboxes：标准库.邮箱 池里存活的邮箱数
#[no_mangle]
pub extern "C" fn qi_proc_stats_json() -> *mut c_char {
    rc_cstr_from_string(stats_json_string())
}

fn stats_json_string() -> String {
    let (cpu_user, cpu_sys) = platform_cpu_ms();
    let v = serde_json::json!({
        "pid": std::process::id(),
        "os": std::env::consts::OS,
        "rss_bytes": platform_rss_bytes(),
        "peak_rss_bytes": platform_peak_rss_bytes(),
        "cpu_user_ms": cpu_user,
        "cpu_sys_ms": cpu_sys,
        "thread_count": platform_thread_count(),
        "open_fd_count": platform_open_fd_count(),
        "uptime_ms": uptime_ms(),
        "goroutines": crate::async_runtime::ffi::live_goroutine_count(),
        "coroutines": crate::async_runtime::coro::live_coroutine_count(),
        "json_handles": crate::stdlib::json_ffi::live_handle_count(),
        "mailboxes": crate::stdlib::mailbox_ffi::live_mailbox_count(),
    });
    v.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn rss_is_positive() {
        // 各平台都实现了 RSS；CI 的三个平台都该 > 0
        if cfg!(any(target_os = "macos", target_os = "linux", windows)) {
            assert!(qi_proc_rss_bytes() > 0, "rss should be > 0");
            assert!(qi_proc_peak_rss_bytes() >= qi_proc_rss_bytes() / 2);
        }
    }

    #[test]
    fn cpu_is_monotonic() {
        let a = qi_proc_cpu_user_ms() + qi_proc_cpu_sys_ms();
        // 烧一点 CPU，保证时间往前走了
        let mut x: u64 = 1;
        for i in 0..3_000_000u64 {
            x = x.wrapping_mul(6364136223846793005).wrapping_add(i);
        }
        assert_ne!(x, 0);
        let b = qi_proc_cpu_user_ms() + qi_proc_cpu_sys_ms();
        assert!(b >= a, "cpu time must not go backwards: {a} -> {b}");
    }

    #[test]
    fn uptime_increases() {
        let a = qi_proc_uptime_ms();
        std::thread::sleep(std::time::Duration::from_millis(30));
        let b = qi_proc_uptime_ms();
        assert!(b >= a + 20, "uptime should grow: {a} -> {b}");
        assert!(a >= 0);
    }

    #[test]
    fn thread_and_fd_counts() {
        if cfg!(any(target_os = "macos", target_os = "linux", windows)) {
            assert!(qi_proc_thread_count() >= 1);
            // 测试进程至少有 stdin/stdout/stderr
            assert!(qi_proc_open_fd_count() >= 1);
        }
    }

    #[test]
    fn json_parses_and_has_keys() {
        let p = qi_proc_stats_json();
        assert!(!p.is_null());
        let s = unsafe { CStr::from_ptr(p) }.to_string_lossy().to_string();
        crate::stdlib::qi_str::rc_cstr_release(p);
        let v: serde_json::Value = serde_json::from_str(&s).expect("stats json must parse");
        for key in [
            "rss_bytes",
            "peak_rss_bytes",
            "cpu_user_ms",
            "cpu_sys_ms",
            "thread_count",
            "open_fd_count",
            "uptime_ms",
            "goroutines",
            "coroutines",
            "json_handles",
            "mailboxes",
        ] {
            assert!(
                v.get(key).and_then(|x| x.as_i64()).is_some(),
                "missing {key}: {s}"
            );
        }
    }

    #[test]
    fn json_handle_count_tracks_pool() {
        let before = crate::stdlib::json_ffi::live_handle_count();
        let h = crate::stdlib::json_ffi::qi_json_create_object();
        assert_eq!(crate::stdlib::json_ffi::live_handle_count(), before + 1);
        crate::stdlib::json_ffi::qi_json_free(h);
        assert_eq!(crate::stdlib::json_ffi::live_handle_count(), before);
    }
}
