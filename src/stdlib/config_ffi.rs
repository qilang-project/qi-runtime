//! 配置文件模块 FFI
//!
//! 提供 TOML, YAML, INI 配置文件读写功能

use std::ffi::CStr;
use std::fs;
use std::os::raw::c_char;

/// 读取 TOML 文件（返回 JSON 字符串）
#[no_mangle]
pub extern "C" fn qi_config_read_toml(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();

        match fs::read_to_string(path_str.as_ref()) {
            Ok(content) => {
                match toml::from_str::<toml::Value>(&content) {
                    Ok(value) => {
                        // 转换为 JSON
                        match serde_json::to_string(&value) {
                            Ok(json) => crate::stdlib::qi_str::rc_cstr_from_string(json),
                            Err(_) => std::ptr::null_mut(),
                        }
                    }
                    Err(_) => std::ptr::null_mut(),
                }
            }
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 写入 TOML 文件（输入 JSON 字符串）
#[no_mangle]
pub extern "C" fn qi_config_write_toml(path: *const c_char, json: *const c_char) -> i32 {
    if path.is_null() || json.is_null() {
        return -1;
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();
        let json_str = CStr::from_ptr(json).to_string_lossy();

        // 从 JSON 解析
        match serde_json::from_str::<serde_json::Value>(&json_str) {
            Ok(value) => {
                // 转换为 TOML
                match toml::to_string_pretty(&value) {
                    Ok(toml_str) => match fs::write(path_str.as_ref(), toml_str) {
                        Ok(_) => 0,
                        Err(_) => -1,
                    },
                    Err(_) => -1,
                }
            }
            Err(_) => -1,
        }
    }
}

/// 读取 INI 文件（返回 JSON 字符串）
#[no_mangle]
pub extern "C" fn qi_config_read_ini(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();

        use configparser::ini::Ini;
        let mut conf = Ini::new();

        match conf.load(path_str.as_ref()) {
            Ok(_) => {
                let map = conf.get_map_ref();

                match serde_json::to_string(&map) {
                    Ok(json) => crate::stdlib::qi_str::rc_cstr_from_string(json),
                    Err(_) => std::ptr::null_mut(),
                }
            }
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 写入 INI 文件（输入 JSON 字符串）
#[no_mangle]
pub extern "C" fn qi_config_write_ini(path: *const c_char, json: *const c_char) -> i32 {
    if path.is_null() || json.is_null() {
        return -1;
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();
        let json_str = CStr::from_ptr(json).to_string_lossy();

        use configparser::ini::Ini;
        use std::collections::HashMap;

        match serde_json::from_str::<HashMap<String, HashMap<String, String>>>(&json_str) {
            Ok(map) => {
                let mut conf = Ini::new();

                for (section, props) in map.iter() {
                    for (key, value) in props.iter() {
                        conf.set(section, key, Some(value.clone()));
                    }
                }

                match conf.write(path_str.as_ref()) {
                    Ok(_) => 0,
                    Err(_) => -1,
                }
            }
            Err(_) => -1,
        }
    }
}

/// 释放字符串（委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃）
#[no_mangle]
pub extern "C" fn qi_config_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::fs;

    #[test]
    fn test_toml_roundtrip() {
        let path_buf = std::env::temp_dir().join("test_config.toml");
        let path = path_buf.to_str().unwrap();
        let json = CString::new(r#"{"key": "value", "number": 42}"#).unwrap();
        let path_c = CString::new(path).unwrap();

        // 写入
        assert_eq!(qi_config_write_toml(path_c.as_ptr(), json.as_ptr()), 0);

        // 读取
        let result = qi_config_read_toml(path_c.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert!(result_str.contains("value"));
            qi_config_free_string(result);
        }

        // 清理
        let _ = fs::remove_file(path);
    }
}
