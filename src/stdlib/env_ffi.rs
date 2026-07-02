//! 环境变量模块 FFI
//!
//! 提供环境变量操作功能

use crate::stdlib::qi_str::{rc_cstr_from_str, rc_cstr_from_string};
use std::env;
use std::ffi::CStr;
use std::os::raw::c_char;

/// 获取环境变量
#[no_mangle]
pub extern "C" fn qi_env_get(key: *const c_char) -> *mut c_char {
    if key.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let key_str = CStr::from_ptr(key).to_string_lossy();

        match env::var(key_str.as_ref()) {
            Ok(value) => rc_cstr_from_string(value),
            Err(_) => rc_cstr_from_str(""),
        }
    }
}

/// 设置环境变量
#[no_mangle]
pub extern "C" fn qi_env_set(key: *const c_char, value: *const c_char) -> i32 {
    if key.is_null() || value.is_null() {
        return -1;
    }

    unsafe {
        let key_str = CStr::from_ptr(key).to_string_lossy();
        let value_str = CStr::from_ptr(value).to_string_lossy();

        env::set_var(key_str.as_ref(), value_str.as_ref());
        0
    }
}

/// 删除环境变量
#[no_mangle]
pub extern "C" fn qi_env_remove(key: *const c_char) -> i32 {
    if key.is_null() {
        return -1;
    }

    unsafe {
        let key_str = CStr::from_ptr(key).to_string_lossy();
        env::remove_var(key_str.as_ref());
        0
    }
}

/// 获取当前目录
#[no_mangle]
pub extern "C" fn qi_env_current_dir() -> *mut c_char {
    match env::current_dir() {
        Ok(path) => rc_cstr_from_str(path.to_string_lossy().as_ref()),
        Err(_) => std::ptr::null_mut(),
    }
}

/// 改变当前目录
#[no_mangle]
pub extern "C" fn qi_env_set_current_dir(path: *const c_char) -> i32 {
    if path.is_null() {
        return -1;
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();

        match env::set_current_dir(path_str.as_ref()) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    }
}

/// 获取用户主目录
#[no_mangle]
pub extern "C" fn qi_env_home_dir() -> *mut c_char {
    if let Some(home) = dirs::home_dir() {
        rc_cstr_from_str(home.to_string_lossy().as_ref())
    } else {
        std::ptr::null_mut()
    }
}

/// 获取所有环境变量（返回 JSON 字符串）
#[no_mangle]
pub extern "C" fn qi_env_all() -> *mut c_char {
    use std::collections::HashMap;

    let vars: HashMap<String, String> = env::vars().collect();

    match serde_json::to_string(&vars) {
        Ok(json) => rc_cstr_from_string(json),
        Err(_) => std::ptr::null_mut(),
    }
}

/// 释放字符串（委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃）
#[no_mangle]
pub extern "C" fn qi_env_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_set_and_get() {
        let key = CString::new("QI_TEST_VAR").unwrap();
        let value = CString::new("test_value").unwrap();

        assert_eq!(qi_env_set(key.as_ptr(), value.as_ptr()), 0);

        let result = qi_env_get(key.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert_eq!(result_str, "test_value");
            qi_env_free_string(result);
        }
    }

    #[test]
    fn test_current_dir() {
        let result = qi_env_current_dir();
        assert!(!result.is_null());

        unsafe {
            qi_env_free_string(result);
        }
    }
}
