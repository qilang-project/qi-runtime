//! 进程管理模块 FFI
//!
//! 提供进程执行和管理功能

use std::ffi::CStr;
use std::os::raw::c_char;
use std::process::Command;

/// 执行命令并等待完成（返回 JSON: {status, stdout, stderr}）
#[no_mangle]
pub extern "C" fn qi_process_execute(
    command: *const c_char,
    args_json: *const c_char,
) -> *mut c_char {
    if command.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let cmd_str = CStr::from_ptr(command).to_string_lossy();

        // 解析参数（JSON数组）
        let args: Vec<String> = if !args_json.is_null() {
            let args_str = CStr::from_ptr(args_json).to_string_lossy();
            serde_json::from_str(&args_str).unwrap_or_else(|_| Vec::new())
        } else {
            Vec::new()
        };

        // 执行命令
        let output = Command::new(cmd_str.as_ref()).args(&args).output();

        match output {
            Ok(out) => {
                let result = serde_json::json!({
                    "status": out.status.code().unwrap_or(-1),
                    "stdout": String::from_utf8_lossy(&out.stdout).to_string(),
                    "stderr": String::from_utf8_lossy(&out.stderr).to_string(),
                });

                match serde_json::to_string(&result) {
                    Ok(json) => crate::stdlib::qi_str::rc_cstr_from_string(json),
                    Err(_) => std::ptr::null_mut(),
                }
            }
            Err(e) => {
                let error = serde_json::json!({
                    "status": -1,
                    "stdout": "",
                    "stderr": format!("执行失败: {}", e),
                });

                match serde_json::to_string(&error) {
                    Ok(json) => crate::stdlib::qi_str::rc_cstr_from_string(json),
                    Err(_) => std::ptr::null_mut(),
                }
            }
        }
    }
}

/// 获取当前进程ID
#[no_mangle]
pub extern "C" fn qi_process_current_pid() -> i64 {
    std::process::id() as i64
}

/// 原始命令行参数 → `数组<字符串>`（Qi 数组布局：槽 0 是长度，之后每槽一个 rc=1 串）。
///
/// **含程序名**：下标 0 是程序路径（跟 C 的 argv、Rust 的 env::args 一致），
/// 用户参数从下标 1 开始。非 UTF-8 的参数按有损转换（非法字节变成 U+FFFD），
/// 不会因此丢掉一个参数、让下标错位。
#[no_mangle]
pub extern "C" fn qi_process_args() -> *mut std::os::raw::c_void {
    let args: Vec<String> = std::env::args_os()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
    let n = args.len();
    let p = crate::stdlib::rc_obj::qi_obj_alloc(((n + 1) * 8) as i64);
    unsafe {
        *(p as *mut i64) = n as i64;
        for (i, a) in args.iter().enumerate() {
            let s = crate::stdlib::qi_str::rc_cstr_from_str(a);
            *(p as *mut i64).add(i + 1) = s as usize as i64;
        }
    }
    p as *mut std::os::raw::c_void
}

/// 退出进程
#[no_mangle]
pub extern "C" fn qi_process_exit(code: i32) {
    std::process::exit(code);
}

/// 释放字符串（委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃）
#[no_mangle]
pub extern "C" fn qi_process_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_execute_echo() {
        let cmd = CString::new("echo").unwrap();
        let args = CString::new(r#"["Hello"]"#).unwrap();

        let result = qi_process_execute(cmd.as_ptr(), args.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert!(result_str.contains("Hello"));
            qi_process_free_string(result);
        }
    }

    #[test]
    fn test_current_pid() {
        let pid = qi_process_current_pid();
        assert!(pid > 0);
    }
}
