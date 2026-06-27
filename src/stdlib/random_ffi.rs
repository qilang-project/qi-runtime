//! 随机数模块 FFI
//!
//! 提供随机数生成功能

use rand::Rng;
use std::ffi::CString;
use std::os::raw::c_char;
use uuid::Uuid;

/// 生成随机整数 [min, max)
#[no_mangle]
pub extern "C" fn qi_random_int(min: i64, max: i64) -> i64 {
    if min >= max {
        return min;
    }

    let mut rng = rand::thread_rng();
    rng.gen_range(min..max)
}

/// 生成随机浮点数 [min, max)
#[no_mangle]
pub extern "C" fn qi_random_float(min: f64, max: f64) -> f64 {
    if min >= max {
        return min;
    }

    let mut rng = rand::thread_rng();
    rng.gen_range(min..max)
}

/// 生成随机布尔值
#[no_mangle]
pub extern "C" fn qi_random_bool() -> i32 {
    let mut rng = rand::thread_rng();
    if rng.gen_bool(0.5) {
        1
    } else {
        0
    }
}

/// 生成随机字符串
#[no_mangle]
pub extern "C" fn qi_random_string(length: i64) -> *mut c_char {
    if length <= 0 {
        return std::ptr::null_mut();
    }

    use rand::distributions::Alphanumeric;
    let mut rng = rand::thread_rng();

    let random_string: String = (0..length)
        .map(|_| rng.sample(Alphanumeric) as char)
        .collect();

    match CString::new(random_string) {
        Ok(c_str) => c_str.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// 生成 UUID
#[no_mangle]
pub extern "C" fn qi_random_uuid() -> *mut c_char {
    let uuid = Uuid::new_v4().to_string();

    match CString::new(uuid) {
        Ok(c_str) => c_str.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// 释放字符串
#[no_mangle]
pub extern "C" fn qi_random_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn test_random_int() {
        let result = qi_random_int(1, 10);
        assert!(result >= 1 && result < 10);
    }

    #[test]
    fn test_random_float() {
        let result = qi_random_float(0.0, 1.0);
        assert!(result >= 0.0 && result < 1.0);
    }

    #[test]
    fn test_random_bool() {
        let result = qi_random_bool();
        assert!(result == 0 || result == 1);
    }

    #[test]
    fn test_random_string() {
        let result = qi_random_string(10);
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert_eq!(result_str.len(), 10);
            qi_random_free_string(result);
        }
    }

    #[test]
    fn test_uuid() {
        let result = qi_random_uuid();
        assert!(!result.is_null());

        unsafe {
            let uuid_str = CStr::from_ptr(result).to_string_lossy();
            assert_eq!(uuid_str.len(), 36); // UUID格式长度
            qi_random_free_string(result);
        }
    }
}
