//! 操作系统模块 FFI 接口
//!
//! 为 Qi 语言提供跨平台的操作系统功能

use crate::stdlib::qi_str::{rc_cstr_from_str, rc_cstr_from_string};
use std::env;
use std::ffi::CStr;
use std::os::raw::c_char;

/// 获取环境变量
///
/// 参数:
/// - name: 环境变量名称
///
/// 返回: 环境变量值（需要调用 qi_os_free_string 释放），如果不存在返回空字符串
#[no_mangle]
pub extern "C" fn qi_os_getenv(name: *const c_char) -> *mut c_char {
    if name.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let name_str = CStr::from_ptr(name).to_string_lossy().to_string();

        match env::var(&name_str) {
            Ok(value) => rc_cstr_from_string(value),
            Err(_) => {
                // 返回空字符串而不是 null
                rc_cstr_from_str("")
            }
        }
    }
}

/// 设置环境变量
///
/// 参数:
/// - name: 环境变量名称
/// - value: 环境变量值
///
/// 返回: 1 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_os_setenv(name: *const c_char, value: *const c_char) -> i64 {
    if name.is_null() || value.is_null() {
        return -1;
    }

    unsafe {
        let name_str = CStr::from_ptr(name).to_string_lossy().to_string();
        let value_str = CStr::from_ptr(value).to_string_lossy().to_string();

        env::set_var(&name_str, &value_str);
        1 // Return success (set_var panics on error)
    }
}

/// 删除环境变量
///
/// 参数:
/// - name: 环境变量名称
///
/// 返回: 1 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_os_unsetenv(name: *const c_char) -> i64 {
    if name.is_null() {
        return -1;
    }

    unsafe {
        let name_str = CStr::from_ptr(name).to_string_lossy().to_string();
        env::remove_var(&name_str);
        1
    }
}

/// 获取当前工作目录
///
/// 返回: 当前工作目录路径（需要调用 qi_os_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_os_getcwd() -> *mut c_char {
    match env::current_dir() {
        Ok(path) => rc_cstr_from_str(path.to_string_lossy().as_ref()),
        Err(_) => std::ptr::null_mut(),
    }
}

/// 改变当前工作目录
///
/// 参数:
/// - path: 目标目录路径
///
/// 返回: 1 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_os_chdir(path: *const c_char) -> i64 {
    if path.is_null() {
        return -1;
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy().to_string();

        match env::set_current_dir(&path_str) {
            Ok(_) => 1,
            Err(_) => -1,
        }
    }
}

/// 获取操作系统类型
///
/// 返回: "windows", "linux", "macos", 或 "unknown"（需要调用 qi_os_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_os_type() -> *mut c_char {
    let os_type = if cfg!(target_os = "windows") {
        "windows"
    } else if cfg!(target_os = "linux") {
        "linux"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "freebsd") {
        "freebsd"
    } else if cfg!(target_os = "openbsd") {
        "openbsd"
    } else {
        "unknown"
    };

    rc_cstr_from_str(os_type)
}

/// 获取操作系统架构
///
/// 返回: "x86_64", "aarch64", "x86", 等（需要调用 qi_os_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_os_arch() -> *mut c_char {
    let arch = if cfg!(target_arch = "x86_64") {
        "x86_64"
    } else if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        env::consts::ARCH
    };

    rc_cstr_from_str(arch)
}

/// 获取操作系统家族
///
/// 返回: "unix", "windows", 或 "unknown"（需要调用 qi_os_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_os_family() -> *mut c_char {
    let family = env::consts::FAMILY;
    rc_cstr_from_str(family)
}

/// 获取主机名
///
/// 返回: 主机名（需要调用 qi_os_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_os_hostname() -> *mut c_char {
    use std::process::Command;

    let hostname = if cfg!(target_os = "windows") {
        // Windows: 使用 hostname 命令
        Command::new("hostname")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|s| s.trim().to_string())
    } else {
        // Unix: 使用 hostname 命令
        Command::new("hostname")
            .output()
            .ok()
            .and_then(|output| String::from_utf8(output.stdout).ok())
            .map(|s| s.trim().to_string())
    };

    match hostname {
        Some(name) => rc_cstr_from_string(name),
        None => rc_cstr_from_str("unknown"),
    }
}

/// 获取用户名
///
/// 返回: 当前用户名（需要调用 qi_os_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_os_username() -> *mut c_char {
    let username = env::var("USER")
        .or_else(|_| env::var("USERNAME"))
        .unwrap_or_else(|_| "unknown".to_string());

    rc_cstr_from_string(username)
}

/// 获取用户主目录
///
/// 返回: 用户主目录路径（需要调用 qi_os_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_os_homedir() -> *mut c_char {
    let home = env::var("HOME")
        .or_else(|_| env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());

    rc_cstr_from_string(home)
}

/// 获取临时目录
///
/// 返回: 临时目录路径（需要调用 qi_os_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_os_tempdir() -> *mut c_char {
    let temp = env::temp_dir();
    let temp_str = temp.to_string_lossy().to_string();

    rc_cstr_from_string(temp_str)
}

/// 获取CPU核心数
///
/// 返回: CPU核心数
#[no_mangle]
pub extern "C" fn qi_os_cpu_count() -> i64 {
    num_cpus::get() as i64
}

/// 获取进程ID
///
/// 返回: 当前进程ID
#[no_mangle]
pub extern "C" fn qi_os_getpid() -> i64 {
    std::process::id() as i64
}

/// 退出程序
///
/// 参数:
/// - code: 退出码
#[no_mangle]
pub extern "C" fn qi_os_exit(code: i32) {
    std::process::exit(code);
}

/// 获取所有环境变量
///
/// 返回: 环境变量列表，格式为 "KEY1=VALUE1\nKEY2=VALUE2\n..."（需要调用 qi_os_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_os_environ() -> *mut c_char {
    let mut result = String::new();

    for (key, value) in env::vars() {
        result.push_str(&format!("{}={}\n", key, value));
    }

    rc_cstr_from_string(result)
}

/// 从 .env 文件加载环境变量
///
/// 参数:
/// - path: .env 文件路径
///
/// 返回: 成功加载的环境变量数量，失败返回 -1
#[no_mangle]
pub extern "C" fn qi_os_load_env(path: *const c_char) -> i64 {
    if path.is_null() {
        return -1;
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy().to_string();

        // 读取文件内容
        let content = match std::fs::read_to_string(&path_str) {
            Ok(c) => c,
            Err(_) => return -1,
        };

        let mut count = 0;

        // 逐行解析
        for line in content.lines() {
            let line = line.trim();

            // 跳过空行和注释
            if line.is_empty() || line.starts_with('#') {
                continue;
            }

            // 解析 KEY=VALUE 格式
            if let Some(pos) = line.find('=') {
                let key = line[..pos].trim();
                let value = line[pos + 1..].trim();

                // 移除值两边的引号（如果有）
                let value = if (value.starts_with('"') && value.ends_with('"'))
                    || (value.starts_with('\'') && value.ends_with('\''))
                {
                    &value[1..value.len() - 1]
                } else {
                    value
                };

                // 设置环境变量
                env::set_var(key, value);
                count += 1;
            }
        }

        count
    }
}

/// 列出目录内容
///
/// 参数:
/// - path: 目录路径
///
/// 返回: 目录内容，每行一个文件/目录名（需要调用 qi_os_free_string 释放）
///        如果失败返回空字符串
#[no_mangle]
pub extern "C" fn qi_os_list_dir(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return rc_cstr_from_str("");
    }

    unsafe {
        let path_str = match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => return rc_cstr_from_str(""),
        };

        let dir_path = std::path::Path::new(path_str);

        let entries = match std::fs::read_dir(dir_path) {
            Ok(entries) => entries,
            Err(_) => return rc_cstr_from_str(""),
        };

        let mut result = String::new();
        for entry in entries {
            if let Ok(entry) = entry {
                if let Some(name) = entry.file_name().to_str() {
                    result.push_str(name);
                    result.push('\n');
                }
            }
        }

        rc_cstr_from_string(result)
    }
}

/// 检查路径是否为目录
///
/// 参数:
/// - path: 路径
///
/// 返回: 1表示是目录，0表示不是
#[no_mangle]
pub extern "C" fn qi_os_is_dir(path: *const c_char) -> i64 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let path_str = match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        let path_obj = std::path::Path::new(path_str);
        if path_obj.is_dir() {
            1
        } else {
            0
        }
    }
}

/// 检查路径是否为文件
///
/// 参数:
/// - path: 路径
///
/// 返回: 1表示是文件，0表示不是
#[no_mangle]
pub extern "C" fn qi_os_is_file(path: *const c_char) -> i64 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let path_str = match CStr::from_ptr(path).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        let path_obj = std::path::Path::new(path_str);
        if path_obj.is_file() {
            1
        } else {
            0
        }
    }
}

/// 释放操作系统模块返回的字符串
///
/// 参数:
/// - s: 字符串指针
#[no_mangle]
pub extern "C" fn qi_os_free_string(s: *mut c_char) {
    // 委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃
    crate::stdlib::qi_str::rc_cstr_release(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_os_type() {
        let os_type = qi_os_type();
        assert!(!os_type.is_null());

        unsafe {
            let c_str = CStr::from_ptr(os_type);
            let rust_str = c_str.to_string_lossy();

            // 应该返回一个有效的操作系统类型
            assert!(
                ["windows", "linux", "macos", "freebsd", "openbsd", "unknown"]
                    .contains(&rust_str.as_ref())
            );

            qi_os_free_string(os_type);
        }
    }

    #[test]
    fn test_os_arch() {
        let arch = qi_os_arch();
        assert!(!arch.is_null());

        unsafe {
            let c_str = CStr::from_ptr(arch);
            let rust_str = c_str.to_string_lossy();

            // 应该返回一个有效的架构
            assert!(!rust_str.is_empty());

            qi_os_free_string(arch);
        }
    }

    #[test]
    fn test_getenv_setenv() {
        let key = CString::new("QI_TEST_VAR").unwrap();
        let value = CString::new("test_value").unwrap();

        // 设置环境变量
        let result = qi_os_setenv(key.as_ptr(), value.as_ptr());
        assert_eq!(result, 1);

        // 获取环境变量
        let retrieved = qi_os_getenv(key.as_ptr());
        assert!(!retrieved.is_null());

        unsafe {
            let c_str = CStr::from_ptr(retrieved);
            let rust_str = c_str.to_string_lossy();
            assert_eq!(rust_str, "test_value");

            qi_os_free_string(retrieved);
        }

        // 删除环境变量
        let result = qi_os_unsetenv(key.as_ptr());
        assert_eq!(result, 1);
    }

    #[test]
    fn test_getcwd() {
        let cwd = qi_os_getcwd();
        assert!(!cwd.is_null());

        unsafe {
            let c_str = CStr::from_ptr(cwd);
            let rust_str = c_str.to_string_lossy();
            assert!(!rust_str.is_empty());

            qi_os_free_string(cwd);
        }
    }

    #[test]
    fn test_cpu_count() {
        let count = qi_os_cpu_count();
        assert!(count > 0);
    }

    #[test]
    fn test_getpid() {
        let pid = qi_os_getpid();
        assert!(pid > 0);
    }

    #[test]
    fn test_username() {
        let username = qi_os_username();
        assert!(!username.is_null());

        unsafe {
            let c_str = CStr::from_ptr(username);
            let rust_str = c_str.to_string_lossy();
            assert!(!rust_str.is_empty());

            qi_os_free_string(username);
        }
    }

    #[test]
    fn test_homedir() {
        let home = qi_os_homedir();
        assert!(!home.is_null());

        unsafe {
            let c_str = CStr::from_ptr(home);
            let rust_str = c_str.to_string_lossy();
            assert!(!rust_str.is_empty());

            qi_os_free_string(home);
        }
    }
}
