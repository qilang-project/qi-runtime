//! 哈希表数据结构模块 (HashMap Data Structure Module)
//!
//! 提供键值对映射功能，支持字符串键和多种值类型
//! Provides key-value mapping with string keys and multiple value types

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

// 哈希表值类型
enum MapValue {
    IntegerMap(HashMap<String, i64>),
    FloatMap(HashMap<String, f64>),
    StringMap(HashMap<String, String>),
}

// 全局哈希表存储
static HASHMAPS: Mutex<Option<HashMap<u64, MapValue>>> = Mutex::new(None);
// 原子句柄计数器（真并发下 static mut 竞态会发重复句柄 → 堆损坏）
static NEXT_MAP_ID: AtomicU64 = AtomicU64::new(1);

/// 初始化哈希表存储
fn init_hashmaps() {
    let mut maps = HASHMAPS.lock().unwrap();
    if maps.is_none() {
        *maps = Some(HashMap::new());
    }
}

/// 获取下一个哈希表 ID（原子，真并发安全）
fn next_map_id() -> u64 {
    NEXT_MAP_ID.fetch_add(1, Ordering::Relaxed)
}

/// 辅助函数：将 C 字符串转换为 Rust 字符串
unsafe fn c_str_to_rust(s: *const c_char) -> Option<String> {
    if s.is_null() {
        return None;
    }
    CStr::from_ptr(s).to_str().ok().map(|s| s.to_string())
}

// ============================================================================
// 整数哈希表 (Integer HashMap)
// ============================================================================

/// 创建整数哈希表
#[no_mangle]
pub extern "C" fn qi_hashmap_int_create() -> i64 {
    init_hashmaps();
    let id = next_map_id();

    let mut maps = HASHMAPS.lock().unwrap();
    if let Some(ref mut map_collection) = *maps {
        map_collection.insert(id, MapValue::IntegerMap(HashMap::new()));
    }

    id as i64
}

/// 设置整数哈希表的键值
#[no_mangle]
pub extern "C" fn qi_hashmap_int_set(map_id: i64, key: *const c_char, value: i64) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let key_str = unsafe {
        match c_str_to_rust(key) {
            Some(s) => s,
            None => return 0,
        }
    };

    let mut maps = HASHMAPS.lock().unwrap();
    if let Some(ref mut map_collection) = *maps {
        if let Some(MapValue::IntegerMap(ref mut map)) = map_collection.get_mut(&(map_id as u64)) {
            map.insert(key_str, value);
            return 1;
        }
    }
    0
}

/// 获取整数哈希表的值
#[no_mangle]
pub extern "C" fn qi_hashmap_int_get(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let key_str = unsafe {
        match c_str_to_rust(key) {
            Some(s) => s,
            None => return 0,
        }
    };

    let maps = HASHMAPS.lock().unwrap();
    if let Some(ref map_collection) = *maps {
        if let Some(MapValue::IntegerMap(ref map)) = map_collection.get(&(map_id as u64)) {
            return *map.get(&key_str).unwrap_or(&0);
        }
    }
    0
}

/// 检查整数哈希表是否包含键
#[no_mangle]
pub extern "C" fn qi_hashmap_int_contains(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let key_str = unsafe {
        match c_str_to_rust(key) {
            Some(s) => s,
            None => return 0,
        }
    };

    let maps = HASHMAPS.lock().unwrap();
    if let Some(ref map_collection) = *maps {
        if let Some(MapValue::IntegerMap(ref map)) = map_collection.get(&(map_id as u64)) {
            return if map.contains_key(&key_str) { 1 } else { 0 };
        }
    }
    0
}

/// 删除整数哈希表的键
#[no_mangle]
pub extern "C" fn qi_hashmap_int_remove(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let key_str = unsafe {
        match c_str_to_rust(key) {
            Some(s) => s,
            None => return 0,
        }
    };

    let mut maps = HASHMAPS.lock().unwrap();
    if let Some(ref mut map_collection) = *maps {
        if let Some(MapValue::IntegerMap(ref mut map)) = map_collection.get_mut(&(map_id as u64)) {
            return if map.remove(&key_str).is_some() { 1 } else { 0 };
        }
    }
    0
}

/// 获取整数哈希表大小
#[no_mangle]
pub extern "C" fn qi_hashmap_int_size(map_id: i64) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let maps = HASHMAPS.lock().unwrap();
    if let Some(ref map_collection) = *maps {
        if let Some(MapValue::IntegerMap(ref map)) = map_collection.get(&(map_id as u64)) {
            return map.len() as i64;
        }
    }
    0
}

/// 清空整数哈希表
#[no_mangle]
pub extern "C" fn qi_hashmap_int_clear(map_id: i64) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let mut maps = HASHMAPS.lock().unwrap();
    if let Some(ref mut map_collection) = *maps {
        if let Some(MapValue::IntegerMap(ref mut map)) = map_collection.get_mut(&(map_id as u64)) {
            map.clear();
            return 1;
        }
    }
    0
}

// ============================================================================
// 浮点数哈希表 (Float HashMap)
// ============================================================================

/// 创建浮点数哈希表
#[no_mangle]
pub extern "C" fn qi_hashmap_float_create() -> i64 {
    init_hashmaps();
    let id = next_map_id();

    let mut maps = HASHMAPS.lock().unwrap();
    if let Some(ref mut map_collection) = *maps {
        map_collection.insert(id, MapValue::FloatMap(HashMap::new()));
    }

    id as i64
}

/// 设置浮点数哈希表的键值
#[no_mangle]
pub extern "C" fn qi_hashmap_float_set(map_id: i64, key: *const c_char, value: f64) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let key_str = unsafe {
        match c_str_to_rust(key) {
            Some(s) => s,
            None => return 0,
        }
    };

    let mut maps = HASHMAPS.lock().unwrap();
    if let Some(ref mut map_collection) = *maps {
        if let Some(MapValue::FloatMap(ref mut map)) = map_collection.get_mut(&(map_id as u64)) {
            map.insert(key_str, value);
            return 1;
        }
    }
    0
}

/// 获取浮点数哈希表的值
#[no_mangle]
pub extern "C" fn qi_hashmap_float_get(map_id: i64, key: *const c_char) -> f64 {
    if map_id <= 0 {
        return 0.0;
    }

    let key_str = unsafe {
        match c_str_to_rust(key) {
            Some(s) => s,
            None => return 0.0,
        }
    };

    let maps = HASHMAPS.lock().unwrap();
    if let Some(ref map_collection) = *maps {
        if let Some(MapValue::FloatMap(ref map)) = map_collection.get(&(map_id as u64)) {
            return *map.get(&key_str).unwrap_or(&0.0);
        }
    }
    0.0
}

/// 获取浮点数哈希表大小
#[no_mangle]
pub extern "C" fn qi_hashmap_float_size(map_id: i64) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let maps = HASHMAPS.lock().unwrap();
    if let Some(ref map_collection) = *maps {
        if let Some(MapValue::FloatMap(ref map)) = map_collection.get(&(map_id as u64)) {
            return map.len() as i64;
        }
    }
    0
}

// ============================================================================
// 字符串哈希表 (String HashMap)
// ============================================================================

/// 创建字符串哈希表
#[no_mangle]
pub extern "C" fn qi_hashmap_string_create() -> i64 {
    init_hashmaps();
    let id = next_map_id();

    let mut maps = HASHMAPS.lock().unwrap();
    if let Some(ref mut map_collection) = *maps {
        map_collection.insert(id, MapValue::StringMap(HashMap::new()));
    }

    id as i64
}

/// 设置字符串哈希表的键值
#[no_mangle]
pub extern "C" fn qi_hashmap_string_set(
    map_id: i64,
    key: *const c_char,
    value: *const c_char,
) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let key_str = unsafe {
        match c_str_to_rust(key) {
            Some(s) => s,
            None => return 0,
        }
    };

    let value_str = unsafe {
        match c_str_to_rust(value) {
            Some(s) => s,
            None => return 0,
        }
    };

    let mut maps = HASHMAPS.lock().unwrap();
    if let Some(ref mut map_collection) = *maps {
        if let Some(MapValue::StringMap(ref mut map)) = map_collection.get_mut(&(map_id as u64)) {
            map.insert(key_str, value_str);
            return 1;
        }
    }
    0
}

/// 获取字符串哈希表的值
#[no_mangle]
pub extern "C" fn qi_hashmap_string_get(map_id: i64, key: *const c_char) -> *mut c_char {
    if map_id <= 0 {
        return std::ptr::null_mut();
    }

    let key_str = unsafe {
        match c_str_to_rust(key) {
            Some(s) => s,
            None => return std::ptr::null_mut(),
        }
    };

    let maps = HASHMAPS.lock().unwrap();
    if let Some(ref map_collection) = *maps {
        if let Some(MapValue::StringMap(ref map)) = map_collection.get(&(map_id as u64)) {
            if let Some(value) = map.get(&key_str) {
                return CString::new(value.clone()).unwrap().into_raw();
            }
        }
    }
    std::ptr::null_mut()
}

/// 获取字符串哈希表大小
#[no_mangle]
pub extern "C" fn qi_hashmap_string_size(map_id: i64) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let maps = HASHMAPS.lock().unwrap();
    if let Some(ref map_collection) = *maps {
        if let Some(MapValue::StringMap(ref map)) = map_collection.get(&(map_id as u64)) {
            return map.len() as i64;
        }
    }
    0
}

// ============================================================================
// 通用操作 (Generic Operations)
// ============================================================================

/// 释放哈希表
#[no_mangle]
pub extern "C" fn qi_hashmap_free(map_id: i64) -> i64 {
    if map_id <= 0 {
        return 0;
    }

    let mut maps = HASHMAPS.lock().unwrap();
    if let Some(ref mut map_collection) = *maps {
        map_collection.remove(&(map_id as u64));
        return 1;
    }
    0
}

/// 释放字符串
#[no_mangle]
pub extern "C" fn qi_hashmap_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_integer_hashmap() {
        let map_id = qi_hashmap_int_create();
        assert!(map_id > 0);

        let key1 = CString::new("age").unwrap();
        let key2 = CString::new("score").unwrap();

        assert_eq!(qi_hashmap_int_set(map_id, key1.as_ptr(), 25), 1);
        assert_eq!(qi_hashmap_int_set(map_id, key2.as_ptr(), 100), 1);

        assert_eq!(qi_hashmap_int_get(map_id, key1.as_ptr()), 25);
        assert_eq!(qi_hashmap_int_get(map_id, key2.as_ptr()), 100);

        assert_eq!(qi_hashmap_int_size(map_id), 2);
        assert_eq!(qi_hashmap_int_contains(map_id, key1.as_ptr()), 1);

        assert_eq!(qi_hashmap_int_remove(map_id, key1.as_ptr()), 1);
        assert_eq!(qi_hashmap_int_size(map_id), 1);

        assert_eq!(qi_hashmap_free(map_id), 1);
    }
}
