//! IO 模块 FFI 接口
//!
//! 为 Qi 语言提供 C 接口的文件操作函数

use super::file::{文件操作, 文件模块};
use crate::stdlib::StdlibValue;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::OnceLock;

// 全局文件模块实例
static 全局文件模块: OnceLock<文件模块> = OnceLock::new();

fn 获取文件模块() -> &'static 文件模块 {
    全局文件模块.get_or_init(|| 文件模块::创建())
}

/// 初始化文件模块
#[no_mangle]
pub extern "C" fn qi_io_init() {
    let _ = 获取文件模块();
}

/// 读取文件内容。
///
/// 文件不存在或读取失败时返回空 C 字符串（而不是 null 指针），这样下游
/// 字符串拼接 `读取(...) + "literal"` 不会被吞成空字符串。用户可以通过
/// `字符串::字节长度(...) == 0` 检测失败。
#[no_mangle]
pub extern "C" fn qi_io_read_file(path: *const c_char) -> *mut c_char {
    let empty = || crate::stdlib::qi_str::rc_cstr_from_str("");
    if path.is_null() {
        return empty();
    }

    unsafe {
        let 路径 = CStr::from_ptr(path).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(路径)];

        let 模块 = 获取文件模块();
        match 模块.执行操作(文件操作::读取, &参数) {
            Ok(StdlibValue::String(内容)) => crate::stdlib::qi_str::rc_cstr_from_string(内容),
            _ => empty(),
        }
    }
}

/// 写入文件内容
#[no_mangle]
pub extern "C" fn qi_io_write_file(path: *const c_char, content: *const c_char) -> i64 {
    if path.is_null() || content.is_null() {
        return 0;
    }

    unsafe {
        let 路径 = CStr::from_ptr(path).to_string_lossy().to_string();
        let 内容 = CStr::from_ptr(content).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(路径), StdlibValue::String(内容)];

        let 模块 = 获取文件模块();
        match 模块.执行操作(文件操作::写入, &参数) {
            Ok(_) => 1,
            _ => 0,
        }
    }
}

/// 追加文件内容
#[no_mangle]
pub extern "C" fn qi_io_append_file(path: *const c_char, content: *const c_char) -> i64 {
    if path.is_null() || content.is_null() {
        return 0;
    }

    unsafe {
        let 路径 = CStr::from_ptr(path).to_string_lossy().to_string();
        let 内容 = CStr::from_ptr(content).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(路径), StdlibValue::String(内容)];

        let 模块 = 获取文件模块();
        match 模块.执行操作(文件操作::追加, &参数) {
            Ok(_) => 1,
            _ => 0,
        }
    }
}

/// 删除文件
#[no_mangle]
pub extern "C" fn qi_io_delete_file(path: *const c_char) -> i64 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let 路径 = CStr::from_ptr(path).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(路径)];

        let 模块 = 获取文件模块();
        match 模块.执行操作(文件操作::删除, &参数) {
            Ok(_) => 1,
            _ => 0,
        }
    }
}

/// 创建文件
#[no_mangle]
pub extern "C" fn qi_io_create_file(path: *const c_char) -> i64 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let 路径 = CStr::from_ptr(path).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(路径)];

        let 模块 = 获取文件模块();
        match 模块.执行操作(文件操作::创建, &参数) {
            Ok(_) => 1,
            _ => 0,
        }
    }
}

/// 检查文件是否存在
#[no_mangle]
pub extern "C" fn qi_io_file_exists(path: *const c_char) -> i64 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let 路径 = CStr::from_ptr(path).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(路径)];

        let 模块 = 获取文件模块();
        match 模块.执行操作(文件操作::存在, &参数) {
            Ok(StdlibValue::Boolean(exists)) => {
                if exists {
                    1
                } else {
                    0
                }
            }
            _ => 0,
        }
    }
}

/// 获取文件大小
#[no_mangle]
pub extern "C" fn qi_io_file_size(path: *const c_char) -> i64 {
    if path.is_null() {
        return -1;
    }

    unsafe {
        let 路径 = CStr::from_ptr(path).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(路径)];

        let 模块 = 获取文件模块();
        match 模块.执行操作(文件操作::大小, &参数) {
            Ok(StdlibValue::Integer(size)) => size,
            _ => -1,
        }
    }
}

/// 创建目录
#[no_mangle]
pub extern "C" fn qi_io_create_dir(path: *const c_char) -> i64 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let 路径 = CStr::from_ptr(path).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(路径)];

        let 模块 = 获取文件模块();
        match 模块.执行操作(文件操作::创建目录, &参数) {
            Ok(_) => 1,
            _ => 0,
        }
    }
}

/// 创建符号链接：链接路径 → 指向 目标（幂等：若链接已存在先删）。
/// 成功返回 1，失败返回 0。仅 Unix 实现；其他平台返回 0。
#[no_mangle]
pub extern "C" fn qi_io_symlink(target: *const c_char, link_path: *const c_char) -> i64 {
    if target.is_null() || link_path.is_null() {
        return 0;
    }
    unsafe {
        let 目标 = CStr::from_ptr(target).to_string_lossy().to_string();
        let 链接 = CStr::from_ptr(link_path).to_string_lossy().to_string();
        let _ = std::fs::remove_file(&链接); // 幂等
        #[cfg(unix)]
        {
            match std::os::unix::fs::symlink(&目标, &链接) {
                Ok(_) => 1,
                Err(_) => 0,
            }
        }
        #[cfg(not(unix))]
        {
            let _ = (目标, 链接);
            0
        }
    }
}

/// 删除目录
#[no_mangle]
pub extern "C" fn qi_io_delete_dir(path: *const c_char) -> i64 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let 路径 = CStr::from_ptr(path).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(路径)];

        let 模块 = 获取文件模块();
        match 模块.执行操作(文件操作::删除目录, &参数) {
            Ok(_) => 1,
            _ => 0,
        }
    }
}

/// 释放字符串内存（委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃）
#[no_mangle]
pub extern "C" fn qi_io_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_file_write_read_ffi() {
        let path_buf = std::env::temp_dir().join("test_qi_ffi.txt");
        let path = CString::new(path_buf.to_str().unwrap()).unwrap();
        let content = CString::new("测试FFI").unwrap();

        // 写入
        let result = qi_io_write_file(path.as_ptr(), content.as_ptr());
        assert_eq!(result, 1);

        // 读取
        let read_result = qi_io_read_file(path.as_ptr());
        assert!(!read_result.is_null());

        let read_str = unsafe { CStr::from_ptr(read_result).to_string_lossy() };
        assert_eq!(read_str, "测试FFI");

        qi_io_free_string(read_result);

        // 清理
        let _ = std::fs::remove_file(&path_buf);
    }

    #[test]
    fn test_file_exists_ffi() {
        let path_buf = std::env::temp_dir().join("test_qi_exists_ffi.txt");
        let path = CString::new(path_buf.to_str().unwrap()).unwrap();

        // 文件不存在
        let exists = qi_io_file_exists(path.as_ptr());
        assert_eq!(exists, 0);

        // 创建文件
        let create_result = qi_io_create_file(path.as_ptr());
        assert_eq!(create_result, 1);

        // 文件存在
        let exists = qi_io_file_exists(path.as_ptr());
        assert_eq!(exists, 1);

        // 清理
        let _ = std::fs::remove_file(&path_buf);
    }
}
