//! 列表数据结构模块 (List Data Structure Module)
//!
//! 提供动态数组（列表）功能，支持整数、浮点数和字符串类型
//! Provides dynamic array (list) functionality for integers, floats, and strings

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

// 列表类型枚举
enum ListValue {
    Integer(Vec<i64>),
    Float(Vec<f64>),
    String(Vec<String>),
    Pointer(Vec<usize>),
}

// 全局列表存储
static LISTS: Mutex<Option<HashMap<u64, ListValue>>> = Mutex::new(None);
// 原子句柄计数器（真并发下 static mut 竞态会发重复句柄 → 堆损坏）
static NEXT_LIST_ID: AtomicU64 = AtomicU64::new(1);

/// 初始化列表存储
fn init_lists() {
    let mut lists = LISTS.lock().unwrap();
    if lists.is_none() {
        *lists = Some(HashMap::new());
    }
}

/// 获取下一个列表 ID（原子，真并发安全）
fn next_list_id() -> u64 {
    NEXT_LIST_ID.fetch_add(1, Ordering::Relaxed)
}

// ============================================================================
// 整数列表 (Integer List)
// ============================================================================

/// 创建整数列表
#[no_mangle]
pub extern "C" fn qi_list_int_create() -> i64 {
    init_lists();
    let id = next_list_id();

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        map.insert(id, ListValue::Integer(Vec::new()));
    }

    id as i64
}

/// 向整数列表添加元素
#[no_mangle]
pub extern "C" fn qi_list_int_push(list_id: i64, value: i64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::Integer(ref mut list)) = map.get_mut(&(list_id as u64)) {
            list.push(value);
            return 1;
        }
    }
    0
}

/// 从整数列表获取元素
#[no_mangle]
pub extern "C" fn qi_list_int_get(list_id: i64, index: i64) -> i64 {
    if list_id <= 0 || index < 0 {
        return 0;
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::Integer(ref list)) = map.get(&(list_id as u64)) {
            if (index as usize) < list.len() {
                return list[index as usize];
            }
        }
    }
    0
}

/// 设置整数列表元素
#[no_mangle]
pub extern "C" fn qi_list_int_set(list_id: i64, index: i64, value: i64) -> i64 {
    if list_id <= 0 || index < 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::Integer(ref mut list)) = map.get_mut(&(list_id as u64)) {
            if (index as usize) < list.len() {
                list[index as usize] = value;
                return 1;
            }
        }
    }
    0
}

/// 获取整数列表大小
#[no_mangle]
pub extern "C" fn qi_list_int_size(list_id: i64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::Integer(ref list)) = map.get(&(list_id as u64)) {
            return list.len() as i64;
        }
    }
    0
}

/// 弹出整数列表最后一个元素
#[no_mangle]
pub extern "C" fn qi_list_int_pop(list_id: i64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::Integer(ref mut list)) = map.get_mut(&(list_id as u64)) {
            return list.pop().unwrap_or(0);
        }
    }
    0
}

/// 清空整数列表
#[no_mangle]
pub extern "C" fn qi_list_int_clear(list_id: i64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::Integer(ref mut list)) = map.get_mut(&(list_id as u64)) {
            list.clear();
            return 1;
        }
    }
    0
}

/// 删除整数列表指定位置元素
#[no_mangle]
pub extern "C" fn qi_list_int_remove(list_id: i64, index: i64) -> i64 {
    if list_id <= 0 || index < 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::Integer(ref mut list)) = map.get_mut(&(list_id as u64)) {
            if (index as usize) < list.len() {
                list.remove(index as usize);
                return 1;
            }
        }
    }
    0
}

/// 在整数列表指定位置插入元素
#[no_mangle]
pub extern "C" fn qi_list_int_insert(list_id: i64, index: i64, value: i64) -> i64 {
    if list_id <= 0 || index < 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::Integer(ref mut list)) = map.get_mut(&(list_id as u64)) {
            if (index as usize) <= list.len() {
                list.insert(index as usize, value);
                return 1;
            }
        }
    }
    0
}

/// 检查整数列表是否包含某元素
#[no_mangle]
pub extern "C" fn qi_list_int_contains(list_id: i64, value: i64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::Integer(ref list)) = map.get(&(list_id as u64)) {
            return if list.contains(&value) { 1 } else { 0 };
        }
    }
    0
}

/// 查找整数在列表中的索引
#[no_mangle]
pub extern "C" fn qi_list_int_index_of(list_id: i64, value: i64) -> i64 {
    if list_id <= 0 {
        return -1;
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::Integer(ref list)) = map.get(&(list_id as u64)) {
            for (i, &item) in list.iter().enumerate() {
                if item == value {
                    return i as i64;
                }
            }
        }
    }
    -1
}

// ============================================================================
// 浮点数列表 (Float List)
// ============================================================================

/// 创建浮点数列表
#[no_mangle]
pub extern "C" fn qi_list_float_create() -> i64 {
    init_lists();
    let id = next_list_id();

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        map.insert(id, ListValue::Float(Vec::new()));
    }

    id as i64
}

/// 向浮点数列表添加元素
#[no_mangle]
pub extern "C" fn qi_list_float_push(list_id: i64, value: f64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::Float(ref mut list)) = map.get_mut(&(list_id as u64)) {
            list.push(value);
            return 1;
        }
    }
    0
}

/// 从浮点数列表获取元素
#[no_mangle]
pub extern "C" fn qi_list_float_get(list_id: i64, index: i64) -> f64 {
    if list_id <= 0 || index < 0 {
        return 0.0;
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::Float(ref list)) = map.get(&(list_id as u64)) {
            if (index as usize) < list.len() {
                return list[index as usize];
            }
        }
    }
    0.0
}

/// 获取浮点数列表大小
#[no_mangle]
pub extern "C" fn qi_list_float_size(list_id: i64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::Float(ref list)) = map.get(&(list_id as u64)) {
            return list.len() as i64;
        }
    }
    0
}

// ============================================================================
// 字符串列表 (String List)
// ============================================================================

/// 创建字符串列表
#[no_mangle]
pub extern "C" fn qi_list_string_create() -> i64 {
    init_lists();
    let id = next_list_id();

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        map.insert(id, ListValue::String(Vec::new()));
    }

    id as i64
}

/// 向字符串列表添加元素
#[no_mangle]
pub extern "C" fn qi_list_string_push(list_id: i64, value: *const c_char) -> i64 {
    if list_id <= 0 || value.is_null() {
        return 0;
    }

    let c_str = unsafe { CStr::from_ptr(value) };
    let rust_str = match c_str.to_str() {
        Ok(s) => s.to_string(),
        Err(_) => return 0,
    };

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::String(ref mut list)) = map.get_mut(&(list_id as u64)) {
            list.push(rust_str);
            return 1;
        }
    }
    0
}

/// 从字符串列表获取元素
#[no_mangle]
pub extern "C" fn qi_list_string_get(list_id: i64, index: i64) -> *mut c_char {
    if list_id <= 0 || index < 0 {
        return std::ptr::null_mut();
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::String(ref list)) = map.get(&(list_id as u64)) {
            if (index as usize) < list.len() {
                return crate::stdlib::qi_str::rc_cstr_from_str(&list[index as usize]);
            }
        }
    }
    std::ptr::null_mut()
}

/// 获取字符串列表大小
#[no_mangle]
pub extern "C" fn qi_list_string_size(list_id: i64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::String(ref list)) = map.get(&(list_id as u64)) {
            return list.len() as i64;
        }
    }
    0
}

// ============================================================================
// 指针列表 (Pointer List)
// ============================================================================

/// 创建指针列表
#[no_mangle]
pub extern "C" fn qi_list_ptr_create() -> i64 {
    init_lists();
    let id = next_list_id();

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        map.insert(id, ListValue::Pointer(Vec::new()));
    }

    id as i64
}

/// 向指针列表添加元素
#[no_mangle]
pub extern "C" fn qi_list_ptr_push(list_id: i64, value: *mut c_void) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::Pointer(ref mut list)) = map.get_mut(&(list_id as u64)) {
            list.push(value as usize);
            return 1;
        }
    }
    0
}

/// 从指针列表获取元素
#[no_mangle]
pub extern "C" fn qi_list_ptr_get(list_id: i64, index: i64) -> *mut c_void {
    if list_id <= 0 || index < 0 {
        return std::ptr::null_mut();
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::Pointer(ref list)) = map.get(&(list_id as u64)) {
            if (index as usize) < list.len() {
                return list[index as usize] as *mut c_void;
            }
        }
    }
    std::ptr::null_mut()
}

/// 设置指针列表元素
#[no_mangle]
pub extern "C" fn qi_list_ptr_set(list_id: i64, index: i64, value: *mut c_void) -> i64 {
    if list_id <= 0 || index < 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        if let Some(ListValue::Pointer(ref mut list)) = map.get_mut(&(list_id as u64)) {
            if (index as usize) < list.len() {
                list[index as usize] = value as usize;
                return 1;
            }
        }
    }
    0
}

/// 获取指针列表大小
#[no_mangle]
pub extern "C" fn qi_list_ptr_size(list_id: i64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let lists = LISTS.lock().unwrap();
    if let Some(ref map) = *lists {
        if let Some(ListValue::Pointer(ref list)) = map.get(&(list_id as u64)) {
            return list.len() as i64;
        }
    }
    0
}

// ============================================================================
// 通用操作 (Generic Operations)
// ============================================================================

/// 释放列表
#[no_mangle]
pub extern "C" fn qi_list_free(list_id: i64) -> i64 {
    if list_id <= 0 {
        return 0;
    }

    let mut lists = LISTS.lock().unwrap();
    if let Some(ref mut map) = *lists {
        map.remove(&(list_id as u64));
        return 1;
    }
    0
}

/// 释放字符串（header-aware：qi_list_string_get 返回的是 rc_cstr）
#[no_mangle]
pub extern "C" fn qi_list_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_integer_list() {
        let list_id = qi_list_int_create();
        assert!(list_id > 0);

        assert_eq!(qi_list_int_push(list_id, 10), 1);
        assert_eq!(qi_list_int_push(list_id, 20), 1);
        assert_eq!(qi_list_int_push(list_id, 30), 1);

        assert_eq!(qi_list_int_size(list_id), 3);
        assert_eq!(qi_list_int_get(list_id, 0), 10);
        assert_eq!(qi_list_int_get(list_id, 1), 20);
        assert_eq!(qi_list_int_get(list_id, 2), 30);

        assert_eq!(qi_list_int_contains(list_id, 20), 1);
        assert_eq!(qi_list_int_contains(list_id, 99), 0);

        assert_eq!(qi_list_free(list_id), 1);
    }
}
