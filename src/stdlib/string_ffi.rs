//! String FFI Module
//!
//! This module provides C FFI functions for string manipulation
//! with full Unicode and Chinese language support.

use std::ffi::{CStr, CString};
use std::os::raw::c_char;

/// 返回一个空 C 字符串而不是 null。
///
/// 历史教训：本模块的字符串函数以前在错误路径返回 `null_mut()`，导致
/// 下游 qi 代码 `字符串::替换(...) + "literal"` 拼接被吞成空串。统一
/// 改成返回空字符串后，用户能用 `字符串::字节长度(...) == 0` 检测失
/// 败，拼接结果也跟直觉一致。
#[inline]
fn empty_c_string() -> *mut c_char {
    crate::stdlib::qi_str::rc_cstr_from_str("")
}

/// Find the position of a substring in a string
/// Returns -1 if not found, otherwise returns the byte position
#[no_mangle]
pub extern "C" fn qi_string_find(text_ptr: *const c_char, search_ptr: *const c_char) -> i64 {
    if text_ptr.is_null() || search_ptr.is_null() {
        return -1;
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return -1,
        };

        let search = match CStr::from_ptr(search_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return -1,
        };

        match text.find(search) {
            Some(pos) => pos as i64,
            None => -1,
        }
    }
}

/// Find the position of a substring starting from a given position
/// Returns -1 if not found, otherwise returns the byte position
#[no_mangle]
pub extern "C" fn qi_string_find_from(
    text_ptr: *const c_char,
    search_ptr: *const c_char,
    start: i64,
) -> i64 {
    if text_ptr.is_null() || search_ptr.is_null() || start < 0 {
        return -1;
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return -1,
        };

        let search = match CStr::from_ptr(search_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return -1,
        };

        let start = start as usize;
        if start >= text.len() {
            return -1;
        }

        match text[start..].find(search) {
            Some(pos) => (start + pos) as i64,
            None => -1,
        }
    }
}

/// Extract substring from start position with given length (in bytes)
/// Returns a new string allocated with malloc
#[no_mangle]
pub extern "C" fn qi_string_substring(
    text_ptr: *const c_char,
    start: i64,
    length: i64,
) -> *mut c_char {
    if text_ptr.is_null() || start < 0 || length < 0 {
        return empty_c_string();
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        let start = start as usize;
        let length = length as usize;

        if start >= text.len() {
            return empty_c_string();
        }

        let mut end = std::cmp::min(start + length, text.len());
        // 防御：start/end 落在多字节字符中间时，直接 &text[start..end] 会 panic（
        // non-unwinding，直接 abort 掉用户程序）。把边界向内收缩到最近的 UTF-8 字符边界，
        // 保证切片安全（宁可少切一两个字节，也绝不 abort）。
        let mut start = start;
        while start < text.len() && !text.is_char_boundary(start) {
            start += 1;
        }
        while end > start && !text.is_char_boundary(end) {
            end -= 1;
        }
        if start >= end {
            return empty_c_string();
        }
        let substring = &text[start..end];

        crate::stdlib::qi_str::rc_cstr_from_str(substring)
    }
}

/// Extract substring from start position to end
/// Returns a new string allocated with malloc
#[no_mangle]
pub extern "C" fn qi_string_substring_from(text_ptr: *const c_char, start: i64) -> *mut c_char {
    if text_ptr.is_null() || start < 0 {
        return empty_c_string();
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        let start = start as usize;

        if start >= text.len() {
            return empty_c_string();
        }

        let substring = &text[start..];

        crate::stdlib::qi_str::rc_cstr_from_str(substring)
    }
}

/// Get the byte length of a string
#[no_mangle]
pub extern "C" fn qi_string_byte_length(text_ptr: *const c_char) -> i64 {
    if text_ptr.is_null() {
        return 0;
    }

    unsafe {
        match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s.len() as i64,
            Err(_) => 0,
        }
    }
}

/// Get the character count of a UTF-8 string
#[no_mangle]
pub extern "C" fn qi_string_char_count(text_ptr: *const c_char) -> i64 {
    if text_ptr.is_null() {
        return 0;
    }

    unsafe {
        match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s.chars().count() as i64,
            Err(_) => 0,
        }
    }
}

/// 按字符（Unicode 标量）提取子串：从第 `start` 个字符起，取 `count` 个字符。
///
/// 与按字节的 `qi_string_substring` 不同，这里的偏移和长度都以「字符」计，
/// 汉字/emoji 各算一个字符，绝不会把多字节字符切成半个产出非法 UTF-8。
/// 越界一律钳制（clamp）：起点超过字符数返回空串，长度超过剩余字符只取到末尾。
#[no_mangle]
pub extern "C" fn qi_string_char_substring(
    text_ptr: *const c_char,
    start: i64,
    count: i64,
) -> *mut c_char {
    if text_ptr.is_null() || start < 0 || count < 0 {
        return empty_c_string();
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        // skip/take 天然钳制越界，不会 panic
        let result: String = text
            .chars()
            .skip(start as usize)
            .take(count as usize)
            .collect();

        crate::stdlib::qi_str::rc_cstr_from_string(result)
    }
}

/// 按字符查找子串首次出现位置，返回「字符」索引，未找到返回 -1。
///
/// 与按字节的 `qi_string_find` 不同，返回值是字符下标（可直接喂给
/// `qi_string_char_substring` / `qi_string_char_at`），对中文优先语言更直觉。
#[no_mangle]
pub extern "C" fn qi_string_char_find(text_ptr: *const c_char, search_ptr: *const c_char) -> i64 {
    if text_ptr.is_null() || search_ptr.is_null() {
        return -1;
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return -1,
        };

        let search = match CStr::from_ptr(search_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return -1,
        };

        match text.find(search) {
            // 把字节偏移转成字符偏移：统计命中位置之前的字符数
            Some(byte_pos) => text[..byte_pos].chars().count() as i64,
            None => -1,
        }
    }
}

/// 按字符索引取单个字符（返回单字符串）。越界返回空串。
#[no_mangle]
pub extern "C" fn qi_string_char_at(text_ptr: *const c_char, index: i64) -> *mut c_char {
    if text_ptr.is_null() || index < 0 {
        return empty_c_string();
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        match text.chars().nth(index as usize) {
            Some(ch) => crate::stdlib::qi_str::rc_cstr_from_string(ch.to_string()),
            None => empty_c_string(),
        }
    }
}

/// 按字符从第 `start` 个字符起取到末尾。起点越界返回空串。
#[no_mangle]
pub extern "C" fn qi_string_char_from(text_ptr: *const c_char, start: i64) -> *mut c_char {
    if text_ptr.is_null() || start < 0 {
        return empty_c_string();
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        let result: String = text.chars().skip(start as usize).collect();

        crate::stdlib::qi_str::rc_cstr_from_string(result)
    }
}

/// Replace all occurrences of a substring with another
/// Returns a new string allocated with malloc
#[no_mangle]
pub extern "C" fn qi_string_replace(
    text_ptr: *const c_char,
    search_ptr: *const c_char,
    replace_ptr: *const c_char,
) -> *mut c_char {
    if text_ptr.is_null() || search_ptr.is_null() || replace_ptr.is_null() {
        return empty_c_string();
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        let search = match CStr::from_ptr(search_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        let replace = match CStr::from_ptr(replace_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        let result = text.replace(search, replace);

        crate::stdlib::qi_str::rc_cstr_from_string(result)
    }
}

/// Trim whitespace from both ends of a string
/// Returns a new string allocated with malloc
#[no_mangle]
pub extern "C" fn qi_string_trim(text_ptr: *const c_char) -> *mut c_char {
    if text_ptr.is_null() {
        return empty_c_string();
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        let trimmed = text.trim();

        crate::stdlib::qi_str::rc_cstr_from_str(trimmed)
    }
}

/// Convert string to uppercase
/// Returns a new string allocated with malloc
#[no_mangle]
pub extern "C" fn qi_string_to_upper(text_ptr: *const c_char) -> *mut c_char {
    if text_ptr.is_null() {
        return empty_c_string();
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        let upper = text.to_uppercase();

        crate::stdlib::qi_str::rc_cstr_from_string(upper)
    }
}

/// Convert string to lowercase
/// Returns a new string allocated with malloc
#[no_mangle]
pub extern "C" fn qi_string_to_lower(text_ptr: *const c_char) -> *mut c_char {
    if text_ptr.is_null() {
        return empty_c_string();
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return empty_c_string(),
        };

        let lower = text.to_lowercase();

        crate::stdlib::qi_str::rc_cstr_from_string(lower)
    }
}

/// Check if a string contains a substring
/// Returns 1 if contains, 0 if not
#[no_mangle]
pub extern "C" fn qi_string_contains(text_ptr: *const c_char, search_ptr: *const c_char) -> i64 {
    if text_ptr.is_null() || search_ptr.is_null() {
        return 0;
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        let search = match CStr::from_ptr(search_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        if text.contains(search) {
            1
        } else {
            0
        }
    }
}

/// Check if a string starts with a prefix
/// Returns 1 if starts with, 0 if not
#[no_mangle]
pub extern "C" fn qi_string_starts_with(text_ptr: *const c_char, prefix_ptr: *const c_char) -> i64 {
    if text_ptr.is_null() || prefix_ptr.is_null() {
        return 0;
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        let prefix = match CStr::from_ptr(prefix_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        if text.starts_with(prefix) {
            1
        } else {
            0
        }
    }
}

/// Check if a string ends with a suffix
/// Returns 1 if ends with, 0 if not
#[no_mangle]
pub extern "C" fn qi_string_ends_with(text_ptr: *const c_char, suffix_ptr: *const c_char) -> i64 {
    if text_ptr.is_null() || suffix_ptr.is_null() {
        return 0;
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        let suffix = match CStr::from_ptr(suffix_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        };

        if text.ends_with(suffix) {
            1
        } else {
            0
        }
    }
}

/// Split a string by a delimiter.
/// Returns a string-list handle (compatible with 列表::字符串列表大小 / 获取字符串).
/// Empty delimiter returns a list of one element (the original string).
#[no_mangle]
pub extern "C" fn qi_string_split(text_ptr: *const c_char, delimiter_ptr: *const c_char) -> i64 {
    let list_handle = crate::stdlib::list::qi_list_string_create();
    if text_ptr.is_null() {
        return list_handle;
    }

    unsafe {
        let text = match CStr::from_ptr(text_ptr).to_str() {
            Ok(s) => s,
            Err(_) => return list_handle,
        };

        let parts: Vec<&str> = if delimiter_ptr.is_null() {
            vec![text]
        } else {
            match CStr::from_ptr(delimiter_ptr).to_str() {
                Ok(d) if !d.is_empty() => text.split(d).collect(),
                _ => vec![text],
            }
        };

        for part in parts {
            let c = match CString::new(part) {
                Ok(c) => c,
                Err(_) => continue, // skip parts containing NUL
            };
            crate::stdlib::list::qi_list_string_push(list_handle, c.as_ptr());
        }
    }

    list_handle
}

/// Compare two strings for equality
/// Returns 1 if equal, 0 if not equal
#[no_mangle]
pub extern "C" fn qi_string_equals(a_ptr: *const c_char, b_ptr: *const c_char) -> i64 {
    if a_ptr.is_null() && b_ptr.is_null() {
        return 1;
    }
    if a_ptr.is_null() || b_ptr.is_null() {
        return 0;
    }
    unsafe {
        let a = CStr::from_ptr(a_ptr);
        let b = CStr::from_ptr(b_ptr);
        if a == b {
            1
        } else {
            0
        }
    }
}

/// Free a string allocated by string functions
/// Note: Uses qi_string_free from future.rs (already defined)

#[cfg(test)]
mod tests {
    use super::*;
    use crate::async_runtime::future::qi_string_free;
    use std::ffi::{CStr, CString};

    #[test]
    fn test_string_find() {
        let text = CString::new("Hello, 世界!").unwrap();
        let search = CString::new("世界").unwrap();

        let pos = qi_string_find(text.as_ptr(), search.as_ptr());
        assert!(pos >= 0);

        let not_found = CString::new("不存在").unwrap();
        let pos = qi_string_find(text.as_ptr(), not_found.as_ptr());
        assert_eq!(pos, -1);
    }

    #[test]
    fn test_string_find_from() {
        let text = CString::new("abcabc").unwrap();
        let search = CString::new("bc").unwrap();

        let pos1 = qi_string_find_from(text.as_ptr(), search.as_ptr(), 0);
        assert_eq!(pos1, 1);

        let pos2 = qi_string_find_from(text.as_ptr(), search.as_ptr(), 2);
        assert_eq!(pos2, 4);
    }

    #[test]
    fn test_string_substring() {
        let text = CString::new("Hello, World!").unwrap();

        let sub = qi_string_substring(text.as_ptr(), 0, 5);
        assert!(!sub.is_null());
        unsafe {
            let result = CStr::from_ptr(sub).to_str().unwrap();
            assert_eq!(result, "Hello");
            qi_string_free(sub);
        }
    }

    #[test]
    fn test_string_lengths() {
        let text = CString::new("你好世界").unwrap();

        let byte_len = qi_string_byte_length(text.as_ptr());
        assert_eq!(byte_len, 12); // 4 characters * 3 bytes each

        let char_count = qi_string_char_count(text.as_ptr());
        assert_eq!(char_count, 4);
    }

    #[test]
    fn test_string_replace() {
        let text = CString::new("Hello World").unwrap();
        let search = CString::new("World").unwrap();
        let replace = CString::new("Rust").unwrap();

        let result = qi_string_replace(text.as_ptr(), search.as_ptr(), replace.as_ptr());
        assert!(!result.is_null());
        unsafe {
            let result_str = CStr::from_ptr(result).to_str().unwrap();
            assert_eq!(result_str, "Hello Rust");
            qi_string_free(result);
        }
    }

    #[test]
    fn test_string_trim() {
        let text = CString::new("  Hello  ").unwrap();

        let result = qi_string_trim(text.as_ptr());
        assert!(!result.is_null());
        unsafe {
            let result_str = CStr::from_ptr(result).to_str().unwrap();
            assert_eq!(result_str, "Hello");
            qi_string_free(result);
        }
    }

    #[test]
    fn test_string_case() {
        let text = CString::new("Hello World").unwrap();

        let upper = qi_string_to_upper(text.as_ptr());
        assert!(!upper.is_null());
        unsafe {
            let upper_str = CStr::from_ptr(upper).to_str().unwrap();
            assert_eq!(upper_str, "HELLO WORLD");
            qi_string_free(upper);
        }

        let lower = qi_string_to_lower(text.as_ptr());
        assert!(!lower.is_null());
        unsafe {
            let lower_str = CStr::from_ptr(lower).to_str().unwrap();
            assert_eq!(lower_str, "hello world");
            qi_string_free(lower);
        }
    }

    #[test]
    fn test_string_checks() {
        let text = CString::new("Hello World").unwrap();
        let hello = CString::new("Hello").unwrap();
        let world = CString::new("World").unwrap();
        let test = CString::new("test").unwrap();

        assert_eq!(qi_string_contains(text.as_ptr(), world.as_ptr()), 1);
        assert_eq!(qi_string_contains(text.as_ptr(), test.as_ptr()), 0);

        assert_eq!(qi_string_starts_with(text.as_ptr(), hello.as_ptr()), 1);
        assert_eq!(qi_string_starts_with(text.as_ptr(), world.as_ptr()), 0);

        assert_eq!(qi_string_ends_with(text.as_ptr(), world.as_ptr()), 1);
        assert_eq!(qi_string_ends_with(text.as_ptr(), hello.as_ptr()), 0);
    }

    #[test]
    fn test_chinese_strings() {
        let text = CString::new("你好，世界！").unwrap();
        let search = CString::new("世界").unwrap();

        // Test find with Chinese
        let pos = qi_string_find(text.as_ptr(), search.as_ptr());
        assert!(pos >= 0);

        // Test substring with Chinese
        let sub = qi_string_substring_from(text.as_ptr(), pos);
        assert!(!sub.is_null());
        unsafe {
            let result = CStr::from_ptr(sub).to_str().unwrap();
            assert!(result.starts_with("世界"));
            qi_string_free(sub);
        }

        // Test character count
        let count = qi_string_char_count(text.as_ptr());
        assert_eq!(count, 6); // 你好，世界！
    }
}
