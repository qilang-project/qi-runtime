//! 测试框架模块 FFI
//!
//! 提供断言和测试功能

use std::ffi::CStr;
use std::os::raw::c_char;

/// 断言相等（整数）
#[no_mangle]
pub extern "C" fn qi_test_assert_eq_int(actual: i64, expected: i64, message: *const c_char) -> i32 {
    if actual == expected {
        return 1; // 通过
    }

    unsafe {
        let msg = if !message.is_null() {
            CStr::from_ptr(message).to_string_lossy().to_string()
        } else {
            String::new()
        };

        eprintln!("❌ 断言失败: 期望 {}, 实际 {}", expected, actual);
        if !msg.is_empty() {
            eprintln!("   消息: {}", msg);
        }
    }

    0 // 失败
}

/// 断言相等（浮点数）
#[no_mangle]
pub extern "C" fn qi_test_assert_eq_float(
    actual: f64,
    expected: f64,
    message: *const c_char,
) -> i32 {
    const EPSILON: f64 = 1e-10;

    if (actual - expected).abs() < EPSILON {
        return 1;
    }

    unsafe {
        let msg = if !message.is_null() {
            CStr::from_ptr(message).to_string_lossy().to_string()
        } else {
            String::new()
        };

        eprintln!("❌ 断言失败: 期望 {}, 实际 {}", expected, actual);
        if !msg.is_empty() {
            eprintln!("   消息: {}", msg);
        }
    }

    0
}

/// 断言相等（字符串）
#[no_mangle]
pub extern "C" fn qi_test_assert_eq_string(
    actual: *const c_char,
    expected: *const c_char,
    message: *const c_char,
) -> i32 {
    if actual.is_null() || expected.is_null() {
        eprintln!("❌ 断言失败: 字符串为空指针");
        return 0;
    }

    unsafe {
        let actual_str = CStr::from_ptr(actual).to_string_lossy();
        let expected_str = CStr::from_ptr(expected).to_string_lossy();

        if actual_str == expected_str {
            return 1;
        }

        let msg = if !message.is_null() {
            CStr::from_ptr(message).to_string_lossy().to_string()
        } else {
            String::new()
        };

        eprintln!(
            "❌ 断言失败: 期望 '{}', 实际 '{}'",
            expected_str, actual_str
        );
        if !msg.is_empty() {
            eprintln!("   消息: {}", msg);
        }
    }

    0
}

/// 断言为真
#[no_mangle]
pub extern "C" fn qi_test_assert_true(value: i32, message: *const c_char) -> i32 {
    if value != 0 {
        return 1;
    }

    unsafe {
        let msg = if !message.is_null() {
            CStr::from_ptr(message).to_string_lossy().to_string()
        } else {
            String::new()
        };

        eprintln!("❌ 断言失败: 期望真值, 实际为假");
        if !msg.is_empty() {
            eprintln!("   消息: {}", msg);
        }
    }

    0
}

/// 断言为假
#[no_mangle]
pub extern "C" fn qi_test_assert_false(value: i32, message: *const c_char) -> i32 {
    if value == 0 {
        return 1;
    }

    unsafe {
        let msg = if !message.is_null() {
            CStr::from_ptr(message).to_string_lossy().to_string()
        } else {
            String::new()
        };

        eprintln!("❌ 断言失败: 期望假值, 实际为真");
        if !msg.is_empty() {
            eprintln!("   消息: {}", msg);
        }
    }

    0
}

/// 断言不等（整数）
#[no_mangle]
pub extern "C" fn qi_test_assert_ne_int(
    actual: i64,
    not_expected: i64,
    message: *const c_char,
) -> i32 {
    if actual != not_expected {
        return 1;
    }

    unsafe {
        let msg = if !message.is_null() {
            CStr::from_ptr(message).to_string_lossy().to_string()
        } else {
            String::new()
        };

        eprintln!("❌ 断言失败: 值不应该等于 {}", not_expected);
        if !msg.is_empty() {
            eprintln!("   消息: {}", msg);
        }
    }

    0
}

/// 打印测试通过消息
#[no_mangle]
pub extern "C" fn qi_test_pass(name: *const c_char) {
    unsafe {
        if !name.is_null() {
            let name_str = CStr::from_ptr(name).to_string_lossy();
            println!("✅ 测试通过: {}", name_str);
        } else {
            println!("✅ 测试通过");
        }
    }
}

/// 打印测试失败消息
#[no_mangle]
pub extern "C" fn qi_test_fail(name: *const c_char, reason: *const c_char) {
    unsafe {
        let name_str = if !name.is_null() {
            CStr::from_ptr(name).to_string_lossy().to_string()
        } else {
            "未命名测试".to_string()
        };

        let reason_str = if !reason.is_null() {
            CStr::from_ptr(reason).to_string_lossy().to_string()
        } else {
            "未知原因".to_string()
        };

        eprintln!("❌ 测试失败: {} - {}", name_str, reason_str);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_assert_eq_int() {
        let msg = CString::new("test message").unwrap();
        assert_eq!(qi_test_assert_eq_int(42, 42, msg.as_ptr()), 1);
        assert_eq!(qi_test_assert_eq_int(42, 43, msg.as_ptr()), 0);
    }

    #[test]
    fn test_assert_true_false() {
        let msg = CString::new("test").unwrap();
        assert_eq!(qi_test_assert_true(1, msg.as_ptr()), 1);
        assert_eq!(qi_test_assert_true(0, msg.as_ptr()), 0);
        assert_eq!(qi_test_assert_false(0, msg.as_ptr()), 1);
        assert_eq!(qi_test_assert_false(1, msg.as_ptr()), 0);
    }
}
