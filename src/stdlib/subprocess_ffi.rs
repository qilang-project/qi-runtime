//! 子进程管理模块 FFI
//!
//! 长存的流式子进程：stdin/stdout 走管道，按行读写（MCP stdio = 换行分隔 JSON-RPC）。
//! 后台读线程持续读 stdout 放入队列；读取行/读取行超时从队列弹出。

use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString};
use std::io::{BufRead, BufReader, Write};
use std::os::raw::c_char;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

struct ChildState {
    child: Child,
    stdin: ChildStdin,
    lines: Arc<Mutex<VecDeque<String>>>,
    eof: Arc<AtomicBool>,
}

type Registry = Mutex<HashMap<i64, Arc<Mutex<ChildState>>>>;

fn registry() -> &'static Registry {
    static REG: OnceLock<Registry> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

static COUNTER: AtomicI64 = AtomicI64::new(1);

fn get_child(handle: i64) -> Option<Arc<Mutex<ChildState>>> {
    registry().lock().ok()?.get(&handle).cloned()
}

/// 启动子进程；命令 + 参数JSON(数组)。成功返回句柄(>0)，失败返回 -1。
#[no_mangle]
pub extern "C" fn qi_subprocess_spawn(command: *const c_char, args_json: *const c_char) -> i64 {
    if command.is_null() {
        return -1;
    }
    unsafe {
        let cmd = CStr::from_ptr(command).to_string_lossy().to_string();
        let args: Vec<String> = if args_json.is_null() {
            Vec::new()
        } else {
            let s = CStr::from_ptr(args_json).to_string_lossy();
            serde_json::from_str(&s).unwrap_or_default()
        };
        let mut child = match Command::new(&cmd)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            // stderr 继承到父进程：MCP server 日志可见，且不污染 stdout 的 JSON 流
            .spawn()
        {
            Ok(c) => c,
            Err(_) => return -1,
        };
        let stdin = match child.stdin.take() {
            Some(s) => s,
            None => return -1,
        };
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => return -1,
        };

        // 后台读线程：持续读 stdout，每行 push 进队列；EOF 设标志
        let lines: Arc<Mutex<VecDeque<String>>> = Arc::new(Mutex::new(VecDeque::new()));
        let eof: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
        let lines_clone = lines.clone();
        let eof_clone = eof.clone();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line_result in reader.lines() {
                match line_result {
                    Ok(line) => {
                        if let Ok(mut q) = lines_clone.lock() {
                            q.push_back(line);
                        }
                    }
                    Err(_) => break,
                }
            }
            eof_clone.store(true, Ordering::SeqCst);
        });

        let state = ChildState {
            child,
            stdin,
            lines,
            eof,
        };
        let handle = COUNTER.fetch_add(1, Ordering::SeqCst);
        registry()
            .lock()
            .unwrap()
            .insert(handle, Arc::new(Mutex::new(state)));
        handle
    }
}

/// 向子进程写一行（自动补 \n + flush）。成功 1，失败 0。
#[no_mangle]
pub extern "C" fn qi_subprocess_write_line(handle: i64, line: *const c_char) -> i32 {
    if line.is_null() {
        return 0;
    }
    let text = unsafe { CStr::from_ptr(line).to_string_lossy().to_string() };
    let cell = match get_child(handle) {
        Some(c) => c,
        None => return 0,
    };
    let mut st = match cell.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    if writeln!(st.stdin, "{}", text).is_ok() && st.stdin.flush().is_ok() {
        1
    } else {
        0
    }
}

/// 阻塞读一行 stdout（去掉行尾换行）。EOF/错误返回空串 ""。
/// 使用后台读线程队列：轮询直到有行或 EOF。
#[no_mangle]
pub extern "C" fn qi_subprocess_read_line(handle: i64) -> *mut c_char {
    let empty = || CString::new("").unwrap().into_raw();

    // 取出 lines/eof Arc 引用（短暂持锁，不在 poll 期间持锁）
    let (lines, eof) = {
        let cell = match get_child(handle) {
            Some(c) => c,
            None => return empty(),
        };
        let st = match cell.lock() {
            Ok(g) => g,
            Err(_) => return empty(),
        };
        (st.lines.clone(), st.eof.clone())
    };

    // 轮询队列，直到有行或 EOF
    loop {
        {
            let mut q = match lines.lock() {
                Ok(g) => g,
                Err(_) => return empty(),
            };
            if let Some(line) = q.pop_front() {
                return CString::new(line)
                    .unwrap_or_else(|_| CString::new("").unwrap())
                    .into_raw();
            }
        }
        if eof.load(Ordering::SeqCst) {
            // 再检查一次队列（EOF 设置时可能还有残余行）
            if let Ok(mut q) = lines.lock() {
                if let Some(line) = q.pop_front() {
                    return CString::new(line)
                        .unwrap_or_else(|_| CString::new("").unwrap())
                        .into_raw();
                }
            }
            return empty();
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// 带超时读一行 stdout。timeout_ms 内有行则返回，否则返回 ""。EOF 也返回 ""。
#[no_mangle]
pub extern "C" fn qi_subprocess_read_line_timeout(handle: i64, timeout_ms: i64) -> *mut c_char {
    let empty = || CString::new("").unwrap().into_raw();

    let (lines, eof) = {
        let cell = match get_child(handle) {
            Some(c) => c,
            None => return empty(),
        };
        let st = match cell.lock() {
            Ok(g) => g,
            Err(_) => return empty(),
        };
        (st.lines.clone(), st.eof.clone())
    };

    let deadline = Instant::now() + Duration::from_millis(timeout_ms.max(0) as u64);

    loop {
        {
            let mut q = match lines.lock() {
                Ok(g) => g,
                Err(_) => return empty(),
            };
            if let Some(line) = q.pop_front() {
                return CString::new(line)
                    .unwrap_or_else(|_| CString::new("").unwrap())
                    .into_raw();
            }
        }
        if eof.load(Ordering::SeqCst) {
            // drain any last lines
            if let Ok(mut q) = lines.lock() {
                if let Some(line) = q.pop_front() {
                    return CString::new(line)
                        .unwrap_or_else(|_| CString::new("").unwrap())
                        .into_raw();
                }
            }
            return empty();
        }
        if Instant::now() >= deadline {
            return empty();
        }
        thread::sleep(Duration::from_millis(5));
    }
}

/// 是否仍存活。1=活，0=已退出/不存在。
#[no_mangle]
pub extern "C" fn qi_subprocess_is_alive(handle: i64) -> i32 {
    let cell = match get_child(handle) {
        Some(c) => c,
        None => return 0,
    };
    let mut st = match cell.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    match st.child.try_wait() {
        Ok(Some(_)) => 0,
        Ok(None) => 1,
        Err(_) => 0,
    }
}

/// 结束子进程并从注册表移除。成功 1，失败 0。
#[no_mangle]
pub extern "C" fn qi_subprocess_terminate(handle: i64) -> i32 {
    let cell = match registry().lock().unwrap().remove(&handle) {
        Some(c) => c,
        None => return 0,
    };
    let mut st = match cell.lock() {
        Ok(g) => g,
        Err(_) => return 0,
    };
    let _ = st.child.kill();
    let _ = st.child.wait();
    1
}

/// 释放 read_line 返回的字符串。
#[no_mangle]
pub extern "C" fn qi_subprocess_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spawn_write_read_roundtrip() {
        // `cat` 回显：写一行应能原样读回一行
        let cmd = CString::new("cat").unwrap();
        let handle = qi_subprocess_spawn(cmd.as_ptr(), std::ptr::null());
        assert!(handle > 0);

        let line = CString::new("hello-mcp").unwrap();
        assert_eq!(qi_subprocess_write_line(handle, line.as_ptr()), 1);

        let got = qi_subprocess_read_line(handle);
        unsafe {
            let s = CStr::from_ptr(got).to_string_lossy().to_string();
            assert_eq!(s, "hello-mcp");
            qi_subprocess_free_string(got);
        }
        assert_eq!(qi_subprocess_is_alive(handle), 1);
        assert_eq!(qi_subprocess_terminate(handle), 1);
        assert_eq!(qi_subprocess_is_alive(handle), 0);
    }

    #[test]
    fn test_read_line_timeout_returns_empty_when_no_data() {
        // cat 不写入任何数据，读取行超时(300ms) 应快速返回空串
        let cmd = CString::new("cat").unwrap();
        let handle = qi_subprocess_spawn(cmd.as_ptr(), std::ptr::null());
        assert!(handle > 0);

        let start = std::time::Instant::now();
        let got = qi_subprocess_read_line_timeout(handle, 300);
        let elapsed = start.elapsed();

        unsafe {
            let s = CStr::from_ptr(got).to_string_lossy().to_string();
            assert_eq!(s, "", "超时应返回空串");
            qi_subprocess_free_string(got);
        }
        // 应在约 300ms 内完成（给 100ms 宽容量）
        assert!(
            elapsed < Duration::from_millis(500),
            "超时耗时过长: {:?}",
            elapsed
        );

        assert_eq!(qi_subprocess_terminate(handle), 1);
    }
}
