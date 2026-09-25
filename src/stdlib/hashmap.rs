//! 哈希表数据结构模块 (HashMap Data Structure Module)
//!
//! 提供键值对映射功能，支持字符串键和多种值类型
//! Provides key-value mapping with string keys and multiple value types
//!
//! 热路径上的几个取舍（见 qi/基准/哈希表.qi）：
//! - 查 / 判 / 删 直接借 C 串做 `&str` 去查，不再每次拷一份 `String` 再 free；
//! - 写入走 `entry_ref`：只哈希一次，键已存在就原地改值，只有新键才分配；
//! - 键哈希用 ahash（运行时随机种子，抗 HashDoS —— qi-web 的表里装的是请求来的键），
//!   比 std 的 SipHash-1-3 短键快一截；
//! - 句柄登记表的键是自增 id，不是外部输入，用一个乘法混合的轻哈希代替 SipHash。
//! - 登记表用读写锁：只读操作走读锁，不再每次进 pthread_mutex。

use hashbrown::hash_map::EntryRef;
use std::collections::HashMap;
use std::ffi::CStr;
use std::hash::{BuildHasherDefault, Hasher};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;

/// 表内键的哈希器：ahash + 进程级随机种子（抗 HashDoS）
type KeyHasher = ahash::RandomState;
type KeyMap<V> = hashbrown::HashMap<String, V, KeyHasher>;

// 哈希表值类型
enum MapValue {
    IntegerMap(KeyMap<i64>),
    FloatMap(KeyMap<f64>),
    StringMap(KeyMap<String>),
}

/// 句柄 id 的哈希器。id 由 NEXT_MAP_ID 自增发放，外部控制不了，不需要抗碰撞；
/// 乘一个奇常数把低位的变化扩散到高位（hashbrown 用高 7 位做控制字节）。
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        // 只会走 write_u64；兜底按字节折叠，保证实现完整
        for &b in bytes {
            self.0 = (self.0.rotate_left(8) ^ b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_u64(&mut self, n: u64) {
        self.0 = n.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

type Registry = HashMap<u64, MapValue, BuildHasherDefault<IdHasher>>;

// 全局哈希表存储。读写锁：查 / 判 / 大小 走读锁（多个协程线程可并发读），
// 写入 / 删除 / 清空 / 建表 / 释放 走写锁。macOS 上 std 的 RwLock 是原子队列实现，
// 不经 pthread，无竞争时比 Mutex（pthread_mutex）每次调用省一截。
static HASHMAPS: RwLock<Option<Registry>> = RwLock::new(None);
// 原子句柄计数器（真并发下 static mut 竞态会发重复句柄 → 堆损坏）
static NEXT_MAP_ID: AtomicU64 = AtomicU64::new(1);

/// 获取下一个哈希表 ID（原子，真并发安全）
fn next_map_id() -> u64 {
    NEXT_MAP_ID.fetch_add(1, Ordering::Relaxed)
}

/// 登记一张新表，返回句柄
fn register(value: MapValue) -> i64 {
    let id = next_map_id();
    let mut maps = HASHMAPS.write().unwrap();
    maps.get_or_insert_with(Registry::default).insert(id, value);
    id as i64
}

/// 读锁内对句柄对应的表做一次只读操作；句柄无效（<=0 / 不存在）返回 None
fn with_map<R>(map_id: i64, f: impl FnOnce(&MapValue) -> R) -> Option<R> {
    if map_id <= 0 {
        return None;
    }
    let maps = HASHMAPS.read().unwrap();
    maps.as_ref()?.get(&(map_id as u64)).map(f)
}

/// 写锁内对句柄对应的表做一次修改；句柄无效（<=0 / 不存在）返回 None
fn with_map_mut<R>(map_id: i64, f: impl FnOnce(&mut MapValue) -> R) -> Option<R> {
    if map_id <= 0 {
        return None;
    }
    let mut maps = HASHMAPS.write().unwrap();
    maps.as_mut()?.get_mut(&(map_id as u64)).map(f)
}

/// 借用 C 字符串为 `&str`（不分配）。空指针或非法 UTF-8 返回 None ——
/// 与旧版 `c_str_to_rust` 的判定完全一致，只是不再拷贝。
///
/// # Safety
/// `s` 为空或指向以 NUL 结尾的有效 C 串，且在返回值使用期间有效。
unsafe fn borrow_key<'a>(s: *const c_char) -> Option<&'a str> {
    if s.is_null() {
        return None;
    }
    CStr::from_ptr(s).to_str().ok()
}

/// 写入：只哈希一次；键已存在原地改值，新键才分配 `String`
fn put<V>(map: &mut KeyMap<V>, key: &str, value: V) {
    match map.entry_ref(key) {
        EntryRef::Occupied(mut e) => {
            e.insert(value);
        }
        EntryRef::Vacant(e) => {
            e.insert(value);
        }
    }
}

fn int_of(v: &MapValue) -> Option<&KeyMap<i64>> {
    match v {
        MapValue::IntegerMap(m) => Some(m),
        _ => None,
    }
}

fn float_of(v: &MapValue) -> Option<&KeyMap<f64>> {
    match v {
        MapValue::FloatMap(m) => Some(m),
        _ => None,
    }
}

fn string_of(v: &MapValue) -> Option<&KeyMap<String>> {
    match v {
        MapValue::StringMap(m) => Some(m),
        _ => None,
    }
}

fn as_int(v: &mut MapValue) -> Option<&mut KeyMap<i64>> {
    match v {
        MapValue::IntegerMap(m) => Some(m),
        _ => None,
    }
}

fn as_float(v: &mut MapValue) -> Option<&mut KeyMap<f64>> {
    match v {
        MapValue::FloatMap(m) => Some(m),
        _ => None,
    }
}

fn as_string(v: &mut MapValue) -> Option<&mut KeyMap<String>> {
    match v {
        MapValue::StringMap(m) => Some(m),
        _ => None,
    }
}

/// 布尔 → FFI 约定的 1/0
fn flag(b: bool) -> i64 {
    if b {
        1
    } else {
        0
    }
}

// ============================================================================
// 整数哈希表 (Integer HashMap)
// ============================================================================

/// 创建整数哈希表
#[no_mangle]
pub extern "C" fn qi_hashmap_int_create() -> i64 {
    register(MapValue::IntegerMap(KeyMap::default()))
}

/// 设置整数哈希表的键值
#[no_mangle]
pub extern "C" fn qi_hashmap_int_set(map_id: i64, key: *const c_char, value: i64) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    with_map_mut(map_id, |v| as_int(v).map(|m| put(m, k, value)).is_some()).map_or(0, flag)
}

/// 获取整数哈希表的值
#[no_mangle]
pub extern "C" fn qi_hashmap_int_get(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    with_map(map_id, |v| int_of(v).and_then(|m| m.get(k).copied()))
        .flatten()
        .unwrap_or(0)
}

/// 检查整数哈希表是否包含键
#[no_mangle]
pub extern "C" fn qi_hashmap_int_contains(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    with_map(map_id, |v| int_of(v).is_some_and(|m| m.contains_key(k))).map_or(0, flag)
}

/// 删除整数哈希表的键
#[no_mangle]
pub extern "C" fn qi_hashmap_int_remove(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    with_map_mut(map_id, |v| as_int(v).is_some_and(|m| m.remove(k).is_some())).map_or(0, flag)
}

/// 获取整数哈希表大小
#[no_mangle]
pub extern "C" fn qi_hashmap_int_size(map_id: i64) -> i64 {
    with_map(map_id, |v| int_of(v).map(|m| m.len() as i64))
        .flatten()
        .unwrap_or(0)
}

/// 清空整数哈希表
#[no_mangle]
pub extern "C" fn qi_hashmap_int_clear(map_id: i64) -> i64 {
    with_map_mut(map_id, |v| as_int(v).map(|m| m.clear()).is_some()).map_or(0, flag)
}

// ============================================================================
// 浮点数哈希表 (Float HashMap)
// ============================================================================

/// 创建浮点数哈希表
#[no_mangle]
pub extern "C" fn qi_hashmap_float_create() -> i64 {
    register(MapValue::FloatMap(KeyMap::default()))
}

/// 设置浮点数哈希表的键值
#[no_mangle]
pub extern "C" fn qi_hashmap_float_set(map_id: i64, key: *const c_char, value: f64) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    with_map_mut(map_id, |v| as_float(v).map(|m| put(m, k, value)).is_some()).map_or(0, flag)
}

/// 获取浮点数哈希表的值
#[no_mangle]
pub extern "C" fn qi_hashmap_float_get(map_id: i64, key: *const c_char) -> f64 {
    if map_id <= 0 {
        return 0.0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0.0;
    };
    with_map(map_id, |v| float_of(v).and_then(|m| m.get(k).copied()))
        .flatten()
        .unwrap_or(0.0)
}

/// 获取浮点数哈希表大小
#[no_mangle]
pub extern "C" fn qi_hashmap_float_size(map_id: i64) -> i64 {
    with_map(map_id, |v| float_of(v).map(|m| m.len() as i64))
        .flatten()
        .unwrap_or(0)
}

// ============================================================================
// 字符串哈希表 (String HashMap)
// ============================================================================

/// 创建字符串哈希表
#[no_mangle]
pub extern "C" fn qi_hashmap_string_create() -> i64 {
    register(MapValue::StringMap(KeyMap::default()))
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
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    let Some(val) = (unsafe { borrow_key(value) }) else {
        return 0;
    };
    let done = with_map_mut(map_id, |v| {
        let Some(m) = as_string(v) else {
            return false;
        };
        match m.entry_ref(k) {
            // 键已存在：复用旧值的缓冲区，容量够就不分配
            EntryRef::Occupied(mut e) => {
                let slot = e.get_mut();
                slot.clear();
                slot.push_str(val);
            }
            EntryRef::Vacant(e) => {
                e.insert(val.to_owned());
            }
        }
        true
    });
    done.map_or(0, flag)
}

/// 获取字符串哈希表的值
#[no_mangle]
pub extern "C" fn qi_hashmap_string_get(map_id: i64, key: *const c_char) -> *mut c_char {
    if map_id <= 0 {
        return std::ptr::null_mut();
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return std::ptr::null_mut();
    };
    with_map(map_id, |v| {
        string_of(v)
            .and_then(|m| m.get(k))
            .map(|s| crate::stdlib::qi_str::rc_cstr_from_str(s))
    })
    .flatten()
    .unwrap_or(std::ptr::null_mut())
}

/// 获取字符串哈希表大小
#[no_mangle]
pub extern "C" fn qi_hashmap_string_size(map_id: i64) -> i64 {
    with_map(map_id, |v| string_of(v).map(|m| m.len() as i64))
        .flatten()
        .unwrap_or(0)
}

/// 检查字符串哈希表是否包含键（与整数表 contains 对称）
#[no_mangle]
pub extern "C" fn qi_hashmap_str_contains(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    with_map(map_id, |v| string_of(v).is_some_and(|m| m.contains_key(k))).map_or(0, flag)
}

/// 删除字符串哈希表的键（与整数表 remove 对称）
#[no_mangle]
pub extern "C" fn qi_hashmap_str_remove(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    with_map_mut(map_id, |v| {
        as_string(v).is_some_and(|m| m.remove(k).is_some())
    })
    .map_or(0, flag)
}

/// 通用：检查任意类型哈希表是否包含键（整数/浮点/字符串表统一分派）。
/// `哈希表.包含键` 绑定此函数，故对三种表都正确（此前只认整数表）。
#[no_mangle]
pub extern "C" fn qi_hashmap_contains(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    let has = with_map(map_id, |v| match v {
        MapValue::IntegerMap(m) => m.contains_key(k),
        MapValue::FloatMap(m) => m.contains_key(k),
        MapValue::StringMap(m) => m.contains_key(k),
    });
    // 旧版语义：登记表已初始化时，句柄不存在也按「不含」返回 0 —— 结果相同
    has.map_or(0, flag)
}

/// 通用：删除任意类型哈希表的键（整数/浮点/字符串表统一分派）。
/// `哈希表.删除键` 绑定此函数。返回 1=删了 / 0=键不存在或表无效。
#[no_mangle]
pub extern "C" fn qi_hashmap_remove(map_id: i64, key: *const c_char) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    let Some(k) = (unsafe { borrow_key(key) }) else {
        return 0;
    };
    let removed = with_map_mut(map_id, |v| match v {
        MapValue::IntegerMap(m) => m.remove(k).is_some(),
        MapValue::FloatMap(m) => m.remove(k).is_some(),
        MapValue::StringMap(m) => m.remove(k).is_some(),
    });
    removed.map_or(0, flag)
}

// ============================================================================
// 通用操作 (Generic Operations)
// ============================================================================

/// 释放哈希表
/// 返回值沿用旧版：登记表初始化过（创建过任意一张表）就返回 1，不论句柄是否存在。
#[no_mangle]
pub extern "C" fn qi_hashmap_free(map_id: i64) -> i64 {
    if map_id <= 0 {
        return 0;
    }
    // 表本体移出锁外再析构：大表释放不占着全局锁
    let removed = {
        let mut maps = HASHMAPS.write().unwrap();
        match maps.as_mut() {
            Some(map_collection) => Some(map_collection.remove(&(map_id as u64))),
            None => None,
        }
    };
    match removed {
        Some(_table) => 1,
        None => 0,
    }
}

/// 释放字符串（header-aware：qi_hashmap_string_get 返回的是 rc_cstr）
#[no_mangle]
pub extern "C" fn qi_hashmap_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    /// 非法 UTF-8 的 C 串（0xFF 不可能出现在 UTF-8 里）
    fn bad_utf8() -> CString {
        CString::new(vec![b'k', 0xFF, b'x']).unwrap()
    }

    unsafe fn read_and_free(p: *mut c_char) -> Option<String> {
        if p.is_null() {
            return None;
        }
        let s = CStr::from_ptr(p).to_str().unwrap().to_string();
        qi_hashmap_free_string(p);
        Some(s)
    }

    #[test]
    fn test_integer_hashmap() {
        let map_id = qi_hashmap_int_create();
        assert!(map_id > 0);

        let key1 = c("age");
        let key2 = c("score");

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

    #[test]
    fn int_update_existing_key_in_place() {
        let t = qi_hashmap_int_create();
        let k = c("k1");
        assert_eq!(qi_hashmap_int_set(t, k.as_ptr(), 1), 1);
        assert_eq!(qi_hashmap_int_set(t, k.as_ptr(), 2), 1);
        assert_eq!(qi_hashmap_int_set(t, k.as_ptr(), -7), 1);
        assert_eq!(qi_hashmap_int_get(t, k.as_ptr()), -7);
        assert_eq!(qi_hashmap_int_size(t), 1);
        assert_eq!(qi_hashmap_int_clear(t), 1);
        assert_eq!(qi_hashmap_int_size(t), 0);
        assert_eq!(qi_hashmap_int_get(t, k.as_ptr()), 0);
        qi_hashmap_free(t);
    }

    #[test]
    fn missing_key_and_bad_handle_defaults() {
        let t = qi_hashmap_int_create();
        let f = qi_hashmap_float_create();
        let s = qi_hashmap_string_create();
        let k = c("nope");
        assert_eq!(qi_hashmap_int_get(t, k.as_ptr()), 0);
        assert_eq!(qi_hashmap_int_contains(t, k.as_ptr()), 0);
        assert_eq!(qi_hashmap_int_remove(t, k.as_ptr()), 0);
        assert_eq!(qi_hashmap_float_get(f, k.as_ptr()), 0.0);
        assert!(qi_hashmap_string_get(s, k.as_ptr()).is_null());
        assert_eq!(qi_hashmap_contains(s, k.as_ptr()), 0);
        assert_eq!(qi_hashmap_remove(f, k.as_ptr()), 0);

        // 句柄 <=0 / 不存在 / 类型不符
        for bad in [0, -1, i64::MAX] {
            assert_eq!(qi_hashmap_int_set(bad, k.as_ptr(), 1), 0);
            assert_eq!(qi_hashmap_int_get(bad, k.as_ptr()), 0);
            assert_eq!(qi_hashmap_int_size(bad), 0);
            assert_eq!(qi_hashmap_contains(bad, k.as_ptr()), 0);
            assert!(qi_hashmap_string_get(bad, k.as_ptr()).is_null());
        }
        assert_eq!(qi_hashmap_int_set(f, k.as_ptr(), 1), 0);
        assert_eq!(qi_hashmap_float_set(t, k.as_ptr(), 1.0), 0);
        assert_eq!(qi_hashmap_string_set(t, k.as_ptr(), k.as_ptr()), 0);
        assert_eq!(qi_hashmap_int_size(f), 0);
        assert_eq!(qi_hashmap_int_clear(s), 0);

        // 空指针键
        assert_eq!(qi_hashmap_int_set(t, std::ptr::null(), 1), 0);
        assert_eq!(qi_hashmap_int_get(t, std::ptr::null()), 0);
        assert_eq!(qi_hashmap_int_size(t), 0);

        assert_eq!(qi_hashmap_free(t), 1);
        assert_eq!(qi_hashmap_free(f), 1);
        assert_eq!(qi_hashmap_free(s), 1);
        // 旧版语义：登记表初始化过就返回 1，不论句柄在不在
        assert_eq!(qi_hashmap_free(t), 1);
        assert_eq!(qi_hashmap_free(0), 0);
        assert_eq!(qi_hashmap_int_get(t, k.as_ptr()), 0);
    }

    #[test]
    fn non_utf8_key_is_rejected_everywhere() {
        let t = qi_hashmap_int_create();
        let f = qi_hashmap_float_create();
        let s = qi_hashmap_string_create();
        let bad = bad_utf8();
        let ok = c("v");
        assert_eq!(qi_hashmap_int_set(t, bad.as_ptr(), 5), 0);
        assert_eq!(qi_hashmap_float_set(f, bad.as_ptr(), 5.0), 0);
        assert_eq!(qi_hashmap_string_set(s, bad.as_ptr(), ok.as_ptr()), 0);
        // 非法值也拒绝，且不留下半截键
        assert_eq!(qi_hashmap_string_set(s, ok.as_ptr(), bad.as_ptr()), 0);
        assert_eq!(qi_hashmap_int_size(t), 0);
        assert_eq!(qi_hashmap_float_size(f), 0);
        assert_eq!(qi_hashmap_string_size(s), 0);
        assert_eq!(qi_hashmap_int_get(t, bad.as_ptr()), 0);
        assert_eq!(qi_hashmap_float_get(f, bad.as_ptr()), 0.0);
        assert!(qi_hashmap_string_get(s, bad.as_ptr()).is_null());
        assert_eq!(qi_hashmap_int_contains(t, bad.as_ptr()), 0);
        assert_eq!(qi_hashmap_contains(t, bad.as_ptr()), 0);
        assert_eq!(qi_hashmap_str_contains(s, bad.as_ptr()), 0);
        assert_eq!(qi_hashmap_int_remove(t, bad.as_ptr()), 0);
        assert_eq!(qi_hashmap_remove(t, bad.as_ptr()), 0);
        assert_eq!(qi_hashmap_str_remove(s, bad.as_ptr()), 0);
        for id in [t, f, s] {
            qi_hashmap_free(id);
        }
    }

    #[test]
    fn float_table_roundtrip() {
        let f = qi_hashmap_float_create();
        let k = c("pi");
        assert_eq!(qi_hashmap_float_set(f, k.as_ptr(), 3.14), 1);
        assert_eq!(qi_hashmap_float_set(f, k.as_ptr(), 3.5), 1);
        assert_eq!(qi_hashmap_float_get(f, k.as_ptr()), 3.5);
        assert_eq!(qi_hashmap_float_size(f), 1);
        assert_eq!(qi_hashmap_contains(f, k.as_ptr()), 1);
        assert_eq!(qi_hashmap_remove(f, k.as_ptr()), 1);
        assert_eq!(qi_hashmap_contains(f, k.as_ptr()), 0);
        qi_hashmap_free(f);
    }

    #[test]
    fn string_table_update_contains_remove() {
        let s = qi_hashmap_string_create();
        let k = c("名字");
        let v1 = c("一个比较长的值，让缓冲区容量够大");
        let v2 = c("短");
        let v3 = c("再换一个明显更长更长更长更长更长更长更长的值");
        assert_eq!(qi_hashmap_string_set(s, k.as_ptr(), v1.as_ptr()), 1);
        assert_eq!(qi_hashmap_string_set(s, k.as_ptr(), v2.as_ptr()), 1);
        assert_eq!(
            unsafe { read_and_free(qi_hashmap_string_get(s, k.as_ptr())) }.as_deref(),
            Some("短")
        );
        assert_eq!(qi_hashmap_string_set(s, k.as_ptr(), v3.as_ptr()), 1);
        assert_eq!(
            unsafe { read_and_free(qi_hashmap_string_get(s, k.as_ptr())) }.as_deref(),
            Some("再换一个明显更长更长更长更长更长更长更长的值")
        );
        assert_eq!(qi_hashmap_string_size(s), 1);
        assert_eq!(qi_hashmap_str_contains(s, k.as_ptr()), 1);
        assert_eq!(qi_hashmap_contains(s, k.as_ptr()), 1);
        assert_eq!(qi_hashmap_str_remove(s, k.as_ptr()), 1);
        assert_eq!(qi_hashmap_str_remove(s, k.as_ptr()), 0);
        assert_eq!(qi_hashmap_str_contains(s, k.as_ptr()), 0);
        assert_eq!(qi_hashmap_string_set(s, k.as_ptr(), v2.as_ptr()), 1);
        assert_eq!(qi_hashmap_remove(s, k.as_ptr()), 1);
        assert_eq!(qi_hashmap_string_size(s), 0);
        // 空串值是合法值，不是「不存在」
        let empty = c("");
        assert_eq!(qi_hashmap_string_set(s, k.as_ptr(), empty.as_ptr()), 1);
        assert_eq!(
            unsafe { read_and_free(qi_hashmap_string_get(s, k.as_ptr())) }.as_deref(),
            Some("")
        );
        qi_hashmap_free(s);
    }

    #[test]
    fn many_keys_and_concurrent_tables() {
        // 多线程各自一张表 + 共用一张表，句柄不串、计数正确
        let shared = qi_hashmap_int_create();
        let handles: Vec<_> = (0..8)
            .map(|t| {
                std::thread::spawn(move || {
                    let own = qi_hashmap_int_create();
                    for i in 0..2000i64 {
                        let k = c(&format!("k{}", i));
                        assert_eq!(qi_hashmap_int_set(own, k.as_ptr(), i), 1);
                        let sk = c(&format!("t{}-{}", t, i));
                        assert_eq!(qi_hashmap_int_set(shared, sk.as_ptr(), i), 1);
                    }
                    for i in 0..2000i64 {
                        let k = c(&format!("k{}", i));
                        assert_eq!(qi_hashmap_int_get(own, k.as_ptr()), i);
                        // 别的线程还在往共用表写：读锁下读自己写过的键
                        let sk = c(&format!("t{}-{}", t, i));
                        assert_eq!(qi_hashmap_int_get(shared, sk.as_ptr()), i);
                        assert_eq!(qi_hashmap_contains(shared, sk.as_ptr()), 1);
                    }
                    assert_eq!(qi_hashmap_int_size(own), 2000);
                    qi_hashmap_free(own);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(qi_hashmap_int_size(shared), 8 * 2000);
        let k = c("t3-1999");
        assert_eq!(qi_hashmap_int_get(shared, k.as_ptr()), 1999);
        qi_hashmap_free(shared);
    }

    #[test]
    fn id_hasher_spreads_sequential_ids() {
        // 顺序 id 的哈希高 7 位要有变化（hashbrown 的控制字节取高位）
        let tops: std::collections::HashSet<u64> = (1..=64u64)
            .map(|n| {
                let mut h = IdHasher::default();
                h.write_u64(n);
                h.finish() >> 57
            })
            .collect();
        assert!(tops.len() > 32, "only {} distinct top bits", tops.len());
    }
}
