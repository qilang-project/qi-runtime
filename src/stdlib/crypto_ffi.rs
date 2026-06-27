//! 加密模块 FFI 接口
//!
//! 为 Qi 语言提供 C 接口的加密函数

use super::crypto::{加密操作, 加密模块};
use super::StdlibValue;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::OnceLock;

// 全局加密模块实例
static 全局加密模块: OnceLock<加密模块> = OnceLock::new();

fn 获取加密模块() -> &'static 加密模块 {
    全局加密模块.get_or_init(|| 加密模块::创建())
}

/// 初始化加密模块
#[no_mangle]
pub extern "C" fn qi_crypto_init() {
    let _ = 获取加密模块();
}

/// MD5 哈希
#[no_mangle]
pub extern "C" fn qi_crypto_md5(input: *const c_char) -> *mut c_char {
    if input.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 输入字符串 = CStr::from_ptr(input).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(输入字符串)];

        let 模块 = 获取加密模块();
        match 模块.执行操作(加密操作::MD5哈希, &参数) {
            Ok(StdlibValue::String(结果)) => CString::new(结果).unwrap().into_raw(),
            _ => std::ptr::null_mut(),
        }
    }
}

/// SHA256 哈希
#[no_mangle]
pub extern "C" fn qi_crypto_sha256(input: *const c_char) -> *mut c_char {
    if input.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 输入字符串 = CStr::from_ptr(input).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(输入字符串)];

        let 模块 = 获取加密模块();
        match 模块.执行操作(加密操作::SHA256哈希, &参数) {
            Ok(StdlibValue::String(结果)) => CString::new(结果).unwrap().into_raw(),
            _ => std::ptr::null_mut(),
        }
    }
}

/// SHA512 哈希
#[no_mangle]
pub extern "C" fn qi_crypto_sha512(input: *const c_char) -> *mut c_char {
    if input.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 输入字符串 = CStr::from_ptr(input).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(输入字符串)];

        let 模块 = 获取加密模块();
        match 模块.执行操作(加密操作::SHA512哈希, &参数) {
            Ok(StdlibValue::String(结果)) => CString::new(结果).unwrap().into_raw(),
            _ => std::ptr::null_mut(),
        }
    }
}

/// Base64 编码
#[no_mangle]
pub extern "C" fn qi_crypto_base64_encode(input: *const c_char) -> *mut c_char {
    if input.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 输入字符串 = CStr::from_ptr(input).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(输入字符串)];

        let 模块 = 获取加密模块();
        match 模块.执行操作(加密操作::Base64编码, &参数) {
            Ok(StdlibValue::String(结果)) => CString::new(结果).unwrap().into_raw(),
            _ => std::ptr::null_mut(),
        }
    }
}

/// Base64 解码
#[no_mangle]
pub extern "C" fn qi_crypto_base64_decode(input: *const c_char) -> *mut c_char {
    if input.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 输入字符串 = CStr::from_ptr(input).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(输入字符串)];

        let 模块 = 获取加密模块();
        match 模块.执行操作(加密操作::Base64解码, &参数) {
            Ok(StdlibValue::String(结果)) => CString::new(结果).unwrap().into_raw(),
            _ => std::ptr::null_mut(),
        }
    }
}

/// HMAC-SHA256
#[no_mangle]
pub extern "C" fn qi_crypto_hmac_sha256(message: *const c_char, key: *const c_char) -> *mut c_char {
    if message.is_null() || key.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 消息 = CStr::from_ptr(message).to_string_lossy().to_string();
        let 密钥 = CStr::from_ptr(key).to_string_lossy().to_string();
        let 参数 = vec![StdlibValue::String(消息), StdlibValue::String(密钥)];

        let 模块 = 获取加密模块();
        match 模块.执行操作(加密操作::HMAC_SHA256, &参数) {
            Ok(StdlibValue::String(结果)) => CString::new(结果).unwrap().into_raw(),
            _ => std::ptr::null_mut(),
        }
    }
}

/// 释放字符串内存
#[no_mangle]
pub extern "C" fn qi_crypto_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_md5_ffi() {
        let input = CString::new("hello").unwrap();
        let result = qi_crypto_md5(input.as_ptr());

        assert!(!result.is_null());

        let result_str = unsafe { CStr::from_ptr(result).to_string_lossy() };
        assert_eq!(result_str, "5d41402abc4b2a76b9719d911017c592");

        qi_crypto_free_string(result);
    }

    #[test]
    fn test_sha256_ffi() {
        let input = CString::new("hello").unwrap();
        let result = qi_crypto_sha256(input.as_ptr());

        assert!(!result.is_null());

        let result_str = unsafe { CStr::from_ptr(result).to_string_lossy() };
        assert_eq!(
            result_str,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );

        qi_crypto_free_string(result);
    }

    #[test]
    fn test_base64_ffi() {
        let input = CString::new("hello world").unwrap();
        let encoded = qi_crypto_base64_encode(input.as_ptr());

        assert!(!encoded.is_null());

        let encoded_str = unsafe { CStr::from_ptr(encoded).to_string_lossy().to_string() };
        assert_eq!(encoded_str, "aGVsbG8gd29ybGQ=");

        let decoded = qi_crypto_base64_decode(encoded);
        let decoded_str = unsafe { CStr::from_ptr(decoded).to_string_lossy() };
        assert_eq!(decoded_str, "hello world");

        qi_crypto_free_string(encoded);
        qi_crypto_free_string(decoded);
    }
}
