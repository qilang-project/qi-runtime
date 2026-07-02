//! 正则表达式模块 FFI
//!
//! 提供正则表达式匹配、查找、替换等功能

use regex::Regex;
use std::ffi::CStr;
use std::os::raw::c_char;

/// 匹配正则表达式
#[no_mangle]
pub extern "C" fn qi_regex_is_match(pattern: *const c_char, text: *const c_char) -> i32 {
    if pattern.is_null() || text.is_null() {
        return 0;
    }

    unsafe {
        let pattern_str = CStr::from_ptr(pattern).to_string_lossy();
        let text_str = CStr::from_ptr(text).to_string_lossy();

        match Regex::new(&pattern_str) {
            Ok(re) => {
                if re.is_match(&text_str) {
                    1
                } else {
                    0
                }
            }
            Err(_) => 0,
        }
    }
}

/// 查找第一个匹配
#[no_mangle]
pub extern "C" fn qi_regex_find(pattern: *const c_char, text: *const c_char) -> *mut c_char {
    if pattern.is_null() || text.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let pattern_str = CStr::from_ptr(pattern).to_string_lossy();
        let text_str = CStr::from_ptr(text).to_string_lossy();

        match Regex::new(&pattern_str) {
            Ok(re) => {
                if let Some(mat) = re.find(&text_str) {
                    crate::stdlib::qi_str::rc_cstr_from_str(mat.as_str())
                } else {
                    crate::stdlib::qi_str::rc_cstr_from_str("")
                }
            }
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 查找所有匹配（返回 JSON 数组字符串）
#[no_mangle]
pub extern "C" fn qi_regex_find_all(pattern: *const c_char, text: *const c_char) -> *mut c_char {
    if pattern.is_null() || text.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let pattern_str = CStr::from_ptr(pattern).to_string_lossy();
        let text_str = CStr::from_ptr(text).to_string_lossy();

        match Regex::new(&pattern_str) {
            Ok(re) => {
                let matches: Vec<String> = re
                    .find_iter(&text_str)
                    .map(|m| m.as_str().to_string())
                    .collect();

                let json = serde_json::to_string(&matches).unwrap_or_else(|_| "[]".to_string());
                crate::stdlib::qi_str::rc_cstr_from_string(json)
            }
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 替换所有匹配
#[no_mangle]
pub extern "C" fn qi_regex_replace_all(
    pattern: *const c_char,
    text: *const c_char,
    replacement: *const c_char,
) -> *mut c_char {
    if pattern.is_null() || text.is_null() || replacement.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let pattern_str = CStr::from_ptr(pattern).to_string_lossy();
        let text_str = CStr::from_ptr(text).to_string_lossy();
        let replacement_str = CStr::from_ptr(replacement).to_string_lossy();

        match Regex::new(&pattern_str) {
            Ok(re) => {
                let result = re
                    .replace_all(&text_str, replacement_str.as_ref())
                    .to_string();
                crate::stdlib::qi_str::rc_cstr_from_string(result)
            }
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 分割字符串
#[no_mangle]
pub extern "C" fn qi_regex_split(pattern: *const c_char, text: *const c_char) -> *mut c_char {
    if pattern.is_null() || text.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let pattern_str = CStr::from_ptr(pattern).to_string_lossy();
        let text_str = CStr::from_ptr(text).to_string_lossy();

        match Regex::new(&pattern_str) {
            Ok(re) => {
                let parts: Vec<String> = re.split(&text_str).map(|s| s.to_string()).collect();

                let json = serde_json::to_string(&parts).unwrap_or_else(|_| "[]".to_string());
                crate::stdlib::qi_str::rc_cstr_from_string(json)
            }
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 释放字符串（header-aware：本模块返回的都是 rc_cstr）
#[no_mangle]
pub extern "C" fn qi_regex_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_is_match() {
        let pattern = CString::new(r"\d+").unwrap();
        let text1 = CString::new("abc123").unwrap();
        let text2 = CString::new("abc").unwrap();

        assert_eq!(qi_regex_is_match(pattern.as_ptr(), text1.as_ptr()), 1);
        assert_eq!(qi_regex_is_match(pattern.as_ptr(), text2.as_ptr()), 0);
    }

    #[test]
    fn test_find() {
        let pattern = CString::new(r"\d+").unwrap();
        let text = CString::new("abc123def456").unwrap();

        let result = qi_regex_find(pattern.as_ptr(), text.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert_eq!(result_str, "123");
            qi_regex_free_string(result);
        }
    }

    #[test]
    fn test_replace_all() {
        let pattern = CString::new(r"\d+").unwrap();
        let text = CString::new("abc123def456").unwrap();
        let replacement = CString::new("X").unwrap();

        let result = qi_regex_replace_all(pattern.as_ptr(), text.as_ptr(), replacement.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert_eq!(result_str, "abcXdefX");
            qi_regex_free_string(result);
        }
    }
}
