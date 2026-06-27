//! JSON数据处理FFI模块 (JSON Data Processing FFI Module)
//!
//! 提供JSON对象和数组的创建、操作和序列化功能
//! Provides JSON object and array creation, manipulation, and serialization

use serde_json::{Map, Number, Value};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

// JSON值存储
static JSON_VALUES: Mutex<Option<HashMap<u64, Value>>> = Mutex::new(None);
static mut NEXT_JSON_ID: u64 = 1;

/// 初始化JSON存储
fn init_json_storage() {
    let mut storage = JSON_VALUES.lock().unwrap();
    if storage.is_none() {
        *storage = Some(HashMap::new());
    }
}

/// 获取下一个JSON ID
fn next_json_id() -> u64 {
    unsafe {
        let id = NEXT_JSON_ID;
        NEXT_JSON_ID += 1;
        id
    }
}

// ============================================================================
// JSON对象和数组创建 (JSON Object and Array Creation)
// ============================================================================

/// 创建JSON对象
#[no_mangle]
pub extern "C" fn qi_json_create_object() -> i64 {
    init_json_storage();
    let id = next_json_id();

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        map.insert(id, Value::Object(Map::new()));
    }

    id as i64
}

/// 创建JSON数组
#[no_mangle]
pub extern "C" fn qi_json_create_array() -> i64 {
    init_json_storage();
    let id = next_json_id();

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        map.insert(id, Value::Array(Vec::new()));
    }

    id as i64
}

// ============================================================================
// JSON对象字段设置 (JSON Object Field Setting)
// ============================================================================

/// 设置字符串字段
#[no_mangle]
pub extern "C" fn qi_json_set_string(obj_id: i64, key: *const c_char, value: *const c_char) -> i64 {
    if obj_id <= 0 || key.is_null() || value.is_null() {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return 0,
        }
    };

    let value_str = unsafe {
        match CStr::from_ptr(value).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return 0,
        }
    };

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if let Some(Value::Object(ref mut obj)) = map.get_mut(&(obj_id as u64)) {
            obj.insert(key_str, Value::String(value_str));
            return 1;
        }
    }
    0
}

/// 设置整数字段
#[no_mangle]
pub extern "C" fn qi_json_set_int(obj_id: i64, key: *const c_char, value: i64) -> i64 {
    if obj_id <= 0 || key.is_null() {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return 0,
        }
    };

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if let Some(Value::Object(ref mut obj)) = map.get_mut(&(obj_id as u64)) {
            obj.insert(key_str, Value::Number(Number::from(value)));
            return 1;
        }
    }
    0
}

/// 设置浮点数字段
#[no_mangle]
pub extern "C" fn qi_json_set_float(obj_id: i64, key: *const c_char, value: f64) -> i64 {
    if obj_id <= 0 || key.is_null() {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return 0,
        }
    };

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if let Some(Value::Object(ref mut obj)) = map.get_mut(&(obj_id as u64)) {
            if let Some(num) = Number::from_f64(value) {
                obj.insert(key_str, Value::Number(num));
                return 1;
            }
        }
    }
    0
}

/// 设置布尔字段
#[no_mangle]
pub extern "C" fn qi_json_set_bool(obj_id: i64, key: *const c_char, value: i64) -> i64 {
    if obj_id <= 0 || key.is_null() {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return 0,
        }
    };

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if let Some(Value::Object(ref mut obj)) = map.get_mut(&(obj_id as u64)) {
            obj.insert(key_str, Value::Bool(value != 0));
            return 1;
        }
    }
    0
}

/// 设置对象字段
#[no_mangle]
pub extern "C" fn qi_json_set_object(obj_id: i64, key: *const c_char, sub_obj_id: i64) -> i64 {
    if obj_id <= 0 || key.is_null() || sub_obj_id <= 0 {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return 0,
        }
    };

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        // 获取子对象的克隆
        if let Some(sub_obj) = map.get(&(sub_obj_id as u64)).cloned() {
            if let Some(Value::Object(ref mut obj)) = map.get_mut(&(obj_id as u64)) {
                obj.insert(key_str, sub_obj);
                return 1;
            }
        }
    }
    0
}

/// 设置数组字段
#[no_mangle]
pub extern "C" fn qi_json_set_array(obj_id: i64, key: *const c_char, array_id: i64) -> i64 {
    if obj_id <= 0 || key.is_null() || array_id <= 0 {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return 0,
        }
    };

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        // 获取数组的克隆
        if let Some(array) = map.get(&(array_id as u64)).cloned() {
            if let Some(Value::Object(ref mut obj)) = map.get_mut(&(obj_id as u64)) {
                obj.insert(key_str, array);
                return 1;
            }
        }
    }
    0
}

// ============================================================================
// JSON对象字段获取 (JSON Object Field Getting)
// ============================================================================

/// 获取字符串字段
#[no_mangle]
pub extern "C" fn qi_json_get_string(obj_id: i64, key: *const c_char) -> *mut c_char {
    if obj_id <= 0 || key.is_null() {
        return std::ptr::null_mut();
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        }
    };

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Object(ref obj)) = map.get(&(obj_id as u64)) {
            if let Some(Value::String(ref s)) = obj.get(key_str) {
                return CString::new(s.as_str()).unwrap().into_raw();
            }
        }
    }
    std::ptr::null_mut()
}

/// 获取整数字段
#[no_mangle]
pub extern "C" fn qi_json_get_int(obj_id: i64, key: *const c_char) -> i64 {
    if obj_id <= 0 || key.is_null() {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Object(ref obj)) = map.get(&(obj_id as u64)) {
            if let Some(Value::Number(ref n)) = obj.get(key_str) {
                return n.as_i64().unwrap_or(0);
            }
        }
    }
    0
}

/// 获取浮点数字段
#[no_mangle]
pub extern "C" fn qi_json_get_float(obj_id: i64, key: *const c_char) -> f64 {
    if obj_id <= 0 || key.is_null() {
        return 0.0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s,
            Err(_) => return 0.0,
        }
    };

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Object(ref obj)) = map.get(&(obj_id as u64)) {
            if let Some(Value::Number(ref n)) = obj.get(key_str) {
                return n.as_f64().unwrap_or(0.0);
            }
        }
    }
    0.0
}

/// 获取布尔字段
#[no_mangle]
pub extern "C" fn qi_json_get_bool(obj_id: i64, key: *const c_char) -> i64 {
    if obj_id <= 0 || key.is_null() {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Object(ref obj)) = map.get(&(obj_id as u64)) {
            if let Some(Value::Bool(b)) = obj.get(key_str) {
                return if *b { 1 } else { 0 };
            }
        }
    }
    0
}

/// 获取对象字段
#[no_mangle]
pub extern "C" fn qi_json_get_object(obj_id: i64, key: *const c_char) -> i64 {
    if obj_id <= 0 || key.is_null() {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    // 先获取对象的克隆
    let cloned_obj = {
        let storage = JSON_VALUES.lock().unwrap();
        if let Some(ref map) = *storage {
            if let Some(Value::Object(ref obj)) = map.get(&(obj_id as u64)) {
                if let Some(sub_obj) = obj.get(key_str) {
                    if sub_obj.is_object() {
                        Some(sub_obj.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    // 如果成功获取，创建新ID并存储
    if let Some(obj) = cloned_obj {
        let new_id = next_json_id();
        let mut storage = JSON_VALUES.lock().unwrap();
        if let Some(ref mut map) = *storage {
            map.insert(new_id, obj);
            return new_id as i64;
        }
    }
    0
}

/// 获取数组字段
#[no_mangle]
pub extern "C" fn qi_json_get_array(obj_id: i64, key: *const c_char) -> i64 {
    if obj_id <= 0 || key.is_null() {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    // 先获取数组的克隆
    let cloned_array = {
        let storage = JSON_VALUES.lock().unwrap();
        if let Some(ref map) = *storage {
            if let Some(Value::Object(ref obj)) = map.get(&(obj_id as u64)) {
                if let Some(array) = obj.get(key_str) {
                    if array.is_array() {
                        Some(array.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    // 如果成功获取，创建新ID并存储
    if let Some(array) = cloned_array {
        let new_id = next_json_id();
        let mut storage = JSON_VALUES.lock().unwrap();
        if let Some(ref mut map) = *storage {
            map.insert(new_id, array);
            return new_id as i64;
        }
    }
    0
}

// ============================================================================
// JSON数组操作 (JSON Array Operations)
// ============================================================================

/// 向数组添加字符串
#[no_mangle]
pub extern "C" fn qi_json_array_push_string(array_id: i64, value: *const c_char) -> i64 {
    if array_id <= 0 || value.is_null() {
        return 0;
    }

    let value_str = unsafe {
        match CStr::from_ptr(value).to_str() {
            Ok(s) => s.to_string(),
            Err(_) => return 0,
        }
    };

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if let Some(Value::Array(ref mut array)) = map.get_mut(&(array_id as u64)) {
            array.push(Value::String(value_str));
            return 1;
        }
    }
    0
}

/// 向数组添加整数
#[no_mangle]
pub extern "C" fn qi_json_array_push_int(array_id: i64, value: i64) -> i64 {
    if array_id <= 0 {
        return 0;
    }

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if let Some(Value::Array(ref mut array)) = map.get_mut(&(array_id as u64)) {
            array.push(Value::Number(Number::from(value)));
            return 1;
        }
    }
    0
}

/// 向数组添加浮点数
#[no_mangle]
pub extern "C" fn qi_json_array_push_float(array_id: i64, value: f64) -> i64 {
    if array_id <= 0 {
        return 0;
    }

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if let Some(Value::Array(ref mut array)) = map.get_mut(&(array_id as u64)) {
            if let Some(num) = Number::from_f64(value) {
                array.push(Value::Number(num));
                return 1;
            }
        }
    }
    0
}

/// 向数组添加布尔
#[no_mangle]
pub extern "C" fn qi_json_array_push_bool(array_id: i64, value: i64) -> i64 {
    if array_id <= 0 {
        return 0;
    }

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if let Some(Value::Array(ref mut array)) = map.get_mut(&(array_id as u64)) {
            array.push(Value::Bool(value != 0));
            return 1;
        }
    }
    0
}

/// 向数组添加对象
#[no_mangle]
pub extern "C" fn qi_json_array_push_object(array_id: i64, obj_id: i64) -> i64 {
    if array_id <= 0 || obj_id <= 0 {
        return 0;
    }

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        // 获取对象的克隆
        if let Some(obj) = map.get(&(obj_id as u64)).cloned() {
            if let Some(Value::Array(ref mut array)) = map.get_mut(&(array_id as u64)) {
                array.push(obj);
                return 1;
            }
        }
    }
    0
}

// ============================================================================
// JSON数组访问 (JSON Array Access)
// ============================================================================

/// 从数组获取字符串
#[no_mangle]
pub extern "C" fn qi_json_array_get_string(array_id: i64, index: i64) -> *mut c_char {
    if array_id <= 0 || index < 0 {
        return std::ptr::null_mut();
    }

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Array(ref array)) = map.get(&(array_id as u64)) {
            if let Some(Value::String(ref s)) = array.get(index as usize) {
                return CString::new(s.as_str()).unwrap().into_raw();
            }
        }
    }
    std::ptr::null_mut()
}

/// 从数组获取整数
#[no_mangle]
pub extern "C" fn qi_json_array_get_int(array_id: i64, index: i64) -> i64 {
    if array_id <= 0 || index < 0 {
        return 0;
    }

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Array(ref array)) = map.get(&(array_id as u64)) {
            if let Some(Value::Number(ref n)) = array.get(index as usize) {
                return n.as_i64().unwrap_or(0);
            }
        }
    }
    0
}

/// 从数组获取浮点数
#[no_mangle]
pub extern "C" fn qi_json_array_get_float(array_id: i64, index: i64) -> f64 {
    if array_id <= 0 || index < 0 {
        return 0.0;
    }

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Array(ref array)) = map.get(&(array_id as u64)) {
            if let Some(Value::Number(ref n)) = array.get(index as usize) {
                return n.as_f64().unwrap_or(0.0);
            }
        }
    }
    0.0
}

/// 从数组获取布尔
#[no_mangle]
pub extern "C" fn qi_json_array_get_bool(array_id: i64, index: i64) -> i64 {
    if array_id <= 0 || index < 0 {
        return 0;
    }

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Array(ref array)) = map.get(&(array_id as u64)) {
            if let Some(Value::Bool(b)) = array.get(index as usize) {
                return if *b { 1 } else { 0 };
            }
        }
    }
    0
}

/// 从数组获取对象
#[no_mangle]
pub extern "C" fn qi_json_array_get_object(array_id: i64, index: i64) -> i64 {
    if array_id <= 0 || index < 0 {
        return 0;
    }

    // 先获取对象的克隆
    let cloned_obj = {
        let storage = JSON_VALUES.lock().unwrap();
        if let Some(ref map) = *storage {
            if let Some(Value::Array(ref array)) = map.get(&(array_id as u64)) {
                if let Some(obj) = array.get(index as usize) {
                    if obj.is_object() {
                        Some(obj.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        }
    };

    // 如果成功获取，创建新ID并存储
    if let Some(obj) = cloned_obj {
        let new_id = next_json_id();
        let mut storage = JSON_VALUES.lock().unwrap();
        if let Some(ref mut map) = *storage {
            map.insert(new_id, obj);
            return new_id as i64;
        }
    }
    0
}

// ============================================================================
// 工具函数 (Utility Functions)
// ============================================================================

/// 获取数组长度
#[no_mangle]
pub extern "C" fn qi_json_array_length(array_id: i64) -> i64 {
    if array_id <= 0 {
        return 0;
    }

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Array(ref array)) = map.get(&(array_id as u64)) {
            return array.len() as i64;
        }
    }
    0
}

/// 检查对象是否包含键
#[no_mangle]
pub extern "C" fn qi_json_has_key(obj_id: i64, key: *const c_char) -> i64 {
    if obj_id <= 0 || key.is_null() {
        return 0;
    }

    let key_str = unsafe {
        match CStr::from_ptr(key).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(Value::Object(ref obj)) = map.get(&(obj_id as u64)) {
            return if obj.contains_key(key_str) { 1 } else { 0 };
        }
    }
    0
}

/// 转换为JSON字符串
#[no_mangle]
pub extern "C" fn qi_json_to_string(json_id: i64) -> *mut c_char {
    if json_id <= 0 {
        return std::ptr::null_mut();
    }

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(value) = map.get(&(json_id as u64)) {
            if let Ok(json_str) = serde_json::to_string(value) {
                return CString::new(json_str).unwrap().into_raw();
            }
        }
    }
    std::ptr::null_mut()
}

/// 转换为格式化JSON字符串
#[no_mangle]
pub extern "C" fn qi_json_to_string_pretty(json_id: i64) -> *mut c_char {
    if json_id <= 0 {
        return std::ptr::null_mut();
    }

    let storage = JSON_VALUES.lock().unwrap();
    if let Some(ref map) = *storage {
        if let Some(value) = map.get(&(json_id as u64)) {
            if let Ok(json_str) = serde_json::to_string_pretty(value) {
                return CString::new(json_str).unwrap().into_raw();
            }
        }
    }
    std::ptr::null_mut()
}

/// 从JSON字符串解析
#[no_mangle]
pub extern "C" fn qi_json_decode(json_str: *const c_char) -> i64 {
    if json_str.is_null() {
        return 0;
    }

    let json_string = unsafe {
        match CStr::from_ptr(json_str).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    if let Ok(value) = serde_json::from_str::<Value>(json_string) {
        init_json_storage();
        let id = next_json_id();

        let mut storage = JSON_VALUES.lock().unwrap();
        if let Some(ref mut map) = *storage {
            map.insert(id, value);
            return id as i64;
        }
    }
    0
}

/// 编码（暂时等同于to_string，为了API一致性）
#[no_mangle]
pub extern "C" fn qi_json_encode(json_str: *const c_char) -> *mut c_char {
    // 此函数实际上与decode+to_string组合使用
    // 为了简化，这里返回输入的副本
    if json_str.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        match CStr::from_ptr(json_str).to_str() {
            Ok(s) => CString::new(s).unwrap().into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 从 "键=值;键2=值2" 简写创建 JSON 字符串
#[no_mangle]
pub extern "C" fn qi_json_from_pairs(pairs: *const c_char) -> *mut c_char {
    if pairs.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 文本 = CStr::from_ptr(pairs).to_string_lossy().to_string();
        let mut 对象 = Map::new();

        for 项 in 文本.split(|c| c == ';' || c == '\n') {
            let 清理项 = 项.trim();
            if 清理项.is_empty() {
                continue;
            }

            if let Some((键, 值)) = 清理项.split_once('=') {
                let 键 = 键.trim();
                let 值 = 值.trim();
                if !键.is_empty() {
                    对象.insert(键.to_string(), Value::String(值.to_string()));
                }
            }
        }

        if 对象.is_empty() {
            对象.insert("结果".to_string(), Value::String(文本));
        }

        CString::new(Value::Object(对象).to_string())
            .unwrap()
            .into_raw()
    }
}

/// 从普通文本创建 {"结果":"..."} JSON 字符串
#[no_mangle]
pub extern "C" fn qi_json_from_text(text: *const c_char) -> *mut c_char {
    if text.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 文本 = CStr::from_ptr(text).to_string_lossy().to_string();
        let mut 对象 = Map::new();
        对象.insert("结果".to_string(), Value::String(文本));
        CString::new(Value::Object(对象).to_string())
            .unwrap()
            .into_raw()
    }
}

/// 释放JSON对象
#[no_mangle]
pub extern "C" fn qi_json_free(json_id: i64) -> i64 {
    if json_id <= 0 {
        return 0;
    }

    let mut storage = JSON_VALUES.lock().unwrap();
    if let Some(ref mut map) = *storage {
        if map.remove(&(json_id as u64)).is_some() {
            return 1;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_json_object_creation() {
        let obj_id = qi_json_create_object();
        assert!(obj_id > 0);
        assert_eq!(qi_json_free(obj_id), 1);
    }

    #[test]
    fn test_json_array_creation() {
        let array_id = qi_json_create_array();
        assert!(array_id > 0);
        assert_eq!(qi_json_free(array_id), 1);
    }

    #[test]
    fn test_json_set_get_string() {
        let obj_id = qi_json_create_object();
        let key = CString::new("name").unwrap();
        let value = CString::new("Alice").unwrap();

        assert_eq!(qi_json_set_string(obj_id, key.as_ptr(), value.as_ptr()), 1);

        let result = qi_json_get_string(obj_id, key.as_ptr());
        assert!(!result.is_null());

        let result_str = unsafe { CStr::from_ptr(result).to_str().unwrap() };
        assert_eq!(result_str, "Alice");

        qi_json_free(obj_id);
    }

    #[test]
    fn test_json_set_get_int() {
        let obj_id = qi_json_create_object();
        let key = CString::new("age").unwrap();

        assert_eq!(qi_json_set_int(obj_id, key.as_ptr(), 25), 1);
        assert_eq!(qi_json_get_int(obj_id, key.as_ptr()), 25);

        qi_json_free(obj_id);
    }

    #[test]
    fn test_json_array_operations() {
        let array_id = qi_json_create_array();
        let value1 = CString::new("item1").unwrap();
        let value2 = CString::new("item2").unwrap();

        assert_eq!(qi_json_array_push_string(array_id, value1.as_ptr()), 1);
        assert_eq!(qi_json_array_push_string(array_id, value2.as_ptr()), 1);
        assert_eq!(qi_json_array_length(array_id), 2);

        let result = qi_json_array_get_string(array_id, 0);
        assert!(!result.is_null());
        let result_str = unsafe { CStr::from_ptr(result).to_str().unwrap() };
        assert_eq!(result_str, "item1");

        qi_json_free(array_id);
    }

    #[test]
    fn test_json_to_string() {
        let obj_id = qi_json_create_object();
        let key = CString::new("test").unwrap();
        let value = CString::new("value").unwrap();

        qi_json_set_string(obj_id, key.as_ptr(), value.as_ptr());

        let json_str = qi_json_to_string(obj_id);
        assert!(!json_str.is_null());

        let json_string = unsafe { CStr::from_ptr(json_str).to_str().unwrap() };
        assert!(json_string.contains("test"));
        assert!(json_string.contains("value"));

        qi_json_free(obj_id);
    }
}
