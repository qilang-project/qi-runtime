//! QiStr FFI surface — 跟旧的 qi_string_* (基于 *const c_char) 平行存在。
//!
//! 全部按 fat-pointer ABI（按值传 24 字节 struct {ptr, len, base}）。
//! 跟 codegen 对接的入口名以 `qi_str_` 开头。
//!
//! UTF-8 公约：QiStr 数据永远合法 UTF-8。内部使用 `from_utf8_unchecked`，
//! 入口（from_cstr / from_bytes）一次性 lossy 验证。

#![allow(non_snake_case)]

use super::qi_str::{
    alloc_owned, as_bytes, as_str_unchecked, clone, drop_str, from_bytes_lossy, from_str,
    rc_cstr_from_bytes, QiStr, EMPTY,
};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;

// ============================================================================
// 边界 / 互转
// ============================================================================

/// 从 null-terminated C 字符串构造 QiStr（lossy UTF-8 验证）
#[no_mangle]
pub extern "C" fn qi_str_from_cstr(p: *const c_char) -> QiStr {
    if p.is_null() {
        return EMPTY;
    }
    unsafe {
        let bytes = CStr::from_ptr(p).to_bytes();
        from_bytes_lossy(bytes)
    }
}

/// 从字节切片构造 QiStr（lossy UTF-8 验证）
#[no_mangle]
pub extern "C" fn qi_str_from_bytes(ptr: *const u8, len: i64) -> QiStr {
    if ptr.is_null() || len <= 0 {
        return EMPTY;
    }
    unsafe {
        let bytes = std::slice::from_raw_parts(ptr, len as usize);
        from_bytes_lossy(bytes)
    }
}

/// 把 QiStr 转换成 C 字符串（RC 分配 + null-terminated；调用方 qi_string_free 释放）
/// 用于跟期望 *const c_char 的旧 FFI / printf 之类的 interop
///
/// 返回的指针带隐藏 RC header（ptr-24），可 qi_string_retain / qi_string_free。
/// 内嵌 NUL 照存，C 侧 strlen 语义自然截断。
#[no_mangle]
pub extern "C" fn qi_str_to_cstring(s: QiStr) -> *mut c_char {
    rc_cstr_from_bytes(as_bytes(&s))
}

/// 增引用：refcount++（如果 owned），返回新 QiStr（共享同一 buffer）
#[no_mangle]
pub extern "C" fn qi_str_clone(s: QiStr) -> QiStr {
    clone(s)
}

/// 释放：refcount--，归零时 free buffer。literal 时 no-op
#[no_mangle]
pub extern "C" fn qi_str_drop(s: QiStr) {
    drop_str(s);
}

// ============================================================================
// 长度 / 元数据 — 全部 O(1)，零分配
// ============================================================================

/// 字节长度 — O(1) 直接读 struct 字段
#[no_mangle]
pub extern "C" fn qi_str_byte_length(s: QiStr) -> i64 {
    s.len.max(0)
}

/// 字符数（UTF-8 codepoint 计数）— O(n) 但无验证开销
#[no_mangle]
pub extern "C" fn qi_str_char_count(s: QiStr) -> i64 {
    unsafe { as_str_unchecked(&s).chars().count() as i64 }
}

/// 是否为空（O(1)）
#[no_mangle]
pub extern "C" fn qi_str_is_empty(s: QiStr) -> i64 {
    if s.len <= 0 {
        1
    } else {
        0
    }
}

// ============================================================================
// 查找 — 不分配，无验证开销
// ============================================================================

#[no_mangle]
pub extern "C" fn qi_str_find(haystack: QiStr, needle: QiStr) -> i64 {
    let h = as_bytes(&haystack);
    let n = as_bytes(&needle);
    if n.is_empty() {
        return 0;
    }
    if n.len() > h.len() {
        return -1;
    }
    h.windows(n.len())
        .position(|w| w == n)
        .map(|p| p as i64)
        .unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn qi_str_find_from(haystack: QiStr, needle: QiStr, start: i64) -> i64 {
    if start < 0 {
        return -1;
    }
    let h = as_bytes(&haystack);
    let s = start as usize;
    if s >= h.len() {
        return -1;
    }
    let n = as_bytes(&needle);
    if n.is_empty() {
        return start;
    }
    h[s..]
        .windows(n.len())
        .position(|w| w == n)
        .map(|p| (s + p) as i64)
        .unwrap_or(-1)
}

#[no_mangle]
pub extern "C" fn qi_str_contains(haystack: QiStr, needle: QiStr) -> i64 {
    if qi_str_find(haystack, needle) >= 0 {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_str_starts_with(s: QiStr, prefix: QiStr) -> i64 {
    let sb = as_bytes(&s);
    let pb = as_bytes(&prefix);
    if pb.len() > sb.len() {
        return 0;
    }
    if &sb[..pb.len()] == pb {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_str_ends_with(s: QiStr, suffix: QiStr) -> i64 {
    let sb = as_bytes(&s);
    let xb = as_bytes(&suffix);
    if xb.len() > sb.len() {
        return 0;
    }
    if &sb[sb.len() - xb.len()..] == xb {
        1
    } else {
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_str_equals(a: QiStr, b: QiStr) -> i64 {
    if a.len != b.len {
        return 0;
    }
    if as_bytes(&a) == as_bytes(&b) {
        1
    } else {
        0
    }
}

// ============================================================================
// 子串 — **零拷贝路径**，refcount++ 共享父 buffer
// ============================================================================

/// 在字节 offset start 处取 len 字节的子串。零拷贝。
/// start / len 必须落在 char 边界，否则被裁到合法位置。
#[no_mangle]
pub extern "C" fn qi_str_substring(s: QiStr, start: i64, len: i64) -> QiStr {
    if start < 0 || len < 0 || s.len <= 0 {
        return EMPTY;
    }
    let total = s.len as usize;
    let s_start = (start as usize).min(total);
    let s_end = (s_start + len as usize).min(total);

    // char 边界检查 — 不在边界就退到上一个边界
    let bytes = as_bytes(&s);
    let actual_start = align_to_char_boundary(bytes, s_start);
    let actual_end = align_to_char_boundary(bytes, s_end);
    if actual_end <= actual_start {
        return EMPTY;
    }

    // 零拷贝：共享 base + 偏移 ptr
    let new_len = (actual_end - actual_start) as i64;
    let new_ptr = unsafe { s.ptr.add(actual_start) };

    if s.base.is_null() {
        // literal / borrow → 子串也是 literal，无 refcount
        QiStr {
            ptr: new_ptr,
            len: new_len,
            base: std::ptr::null(),
        }
    } else {
        // owned → 子串共享父 base，refcount++
        clone(QiStr {
            ptr: new_ptr,
            len: new_len,
            base: s.base,
        })
    }
}

#[no_mangle]
pub extern "C" fn qi_str_substring_from(s: QiStr, start: i64) -> QiStr {
    if start < 0 || s.len <= 0 {
        return EMPTY;
    }
    let total = s.len as usize;
    let s_start = (start as usize).min(total);
    qi_str_substring(s, s_start as i64, (total - s_start) as i64)
}

/// 把 byte offset 调整到不破坏 UTF-8 char 的最近一个边界（向下取整）
#[inline]
fn align_to_char_boundary(bytes: &[u8], offset: usize) -> usize {
    if offset >= bytes.len() {
        return bytes.len();
    }
    let mut o = offset;
    // UTF-8 续字节是 10xxxxxx (0x80..=0xBF)
    while o > 0 && (bytes[o] & 0xC0) == 0x80 {
        o -= 1;
    }
    o
}

// ============================================================================
// 拼接 / 修改 — 必然 alloc 新 buffer
// ============================================================================

#[no_mangle]
pub extern "C" fn qi_str_concat(a: QiStr, b: QiStr) -> QiStr {
    let ab = as_bytes(&a);
    let bb = as_bytes(&b);
    if ab.is_empty() && bb.is_empty() {
        return EMPTY;
    }
    if ab.is_empty() {
        return clone(b);
    }
    if bb.is_empty() {
        return clone(a);
    }
    let mut buf = Vec::with_capacity(ab.len() + bb.len());
    buf.extend_from_slice(ab);
    buf.extend_from_slice(bb);
    alloc_owned(&buf)
}

#[no_mangle]
pub extern "C" fn qi_str_replace(text: QiStr, search: QiStr, replace: QiStr) -> QiStr {
    let t = unsafe { as_str_unchecked(&text) };
    let s = unsafe { as_str_unchecked(&search) };
    let r = unsafe { as_str_unchecked(&replace) };
    if s.is_empty() {
        return clone(text);
    }
    from_str(&t.replace(s, r))
}

#[no_mangle]
pub extern "C" fn qi_str_to_upper(s: QiStr) -> QiStr {
    let st = unsafe { as_str_unchecked(&s) };
    from_str(&st.to_uppercase())
}

#[no_mangle]
pub extern "C" fn qi_str_to_lower(s: QiStr) -> QiStr {
    let st = unsafe { as_str_unchecked(&s) };
    from_str(&st.to_lowercase())
}

/// trim — 去掉首尾空白。**零拷贝路径**：返回原 buffer 的子串
#[no_mangle]
pub extern "C" fn qi_str_trim(s: QiStr) -> QiStr {
    let bytes = as_bytes(&s);
    if bytes.is_empty() {
        return EMPTY;
    }
    // 找前缀空白（ASCII + UTF-8 空白都靠 char 迭代识别）
    let st = unsafe { as_str_unchecked(&s) };
    let trimmed = st.trim();
    if trimmed.is_empty() {
        return EMPTY;
    }
    // 计算 trimmed 在原 string 中的字节 offset
    let start = trimmed.as_ptr() as usize - st.as_ptr() as usize;
    let len = trimmed.len() as i64;
    qi_str_substring(s, start as i64, len)
}

// ============================================================================
// 分割 — 返回字符串列表（QiStr 列表，跟现有 列表库 整合）
// ============================================================================

/// 分割。**注意**：现阶段返回的是旧字符串列表（每个元素是 c_char* 拷贝），
/// 因为列表库还没改成持有 QiStr。这是一个 placeholder；Phase 6 会切到原生 QiStr 列表。
#[no_mangle]
pub extern "C" fn qi_str_split(s: QiStr, delim: QiStr) -> i64 {
    use crate::stdlib::list::qi_list_string_create;
    use crate::stdlib::list::qi_list_string_push;

    let list_handle = qi_list_string_create();
    let st = unsafe { as_str_unchecked(&s) };
    let dl = unsafe { as_str_unchecked(&delim) };
    let parts: Vec<&str> = if dl.is_empty() {
        vec![st]
    } else {
        st.split(dl).collect()
    };
    for part in parts {
        match CString::new(part) {
            Ok(c) => {
                qi_list_string_push(list_handle, c.as_ptr());
            }
            Err(_) => continue,
        }
    }
    list_handle
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::qi_str::from_str;

    #[test]
    fn byte_length_o1() {
        let s = from_str("hello");
        assert_eq!(qi_str_byte_length(s), 5);
        drop_str(s);
    }

    #[test]
    fn substring_zero_copy_owned() {
        let s = from_str("hello world");
        let parent_base = s.base;
        let sub = qi_str_substring(s, 6, 5);
        assert_eq!(sub.base, parent_base, "substring 应共享 base");
        assert_eq!(sub.len, 5);
        assert_eq!(unsafe { as_str_unchecked(&sub) }, "world");
        drop_str(s);
        // s 的 refcount 现在是 1（s_drop 减 1，sub 持有 1）
        assert_eq!(unsafe { as_str_unchecked(&sub) }, "world");
        drop_str(sub);
    }

    #[test]
    fn substring_of_literal_stays_literal() {
        let lit = QiStr {
            ptr: b"hello world".as_ptr(),
            len: 11,
            base: std::ptr::null(),
        };
        let sub = qi_str_substring(lit, 6, 5);
        assert!(sub.base.is_null(), "literal 子串 base 仍 null");
        assert_eq!(unsafe { as_str_unchecked(&sub) }, "world");
        drop_str(sub);
        drop_str(lit);
    }

    #[test]
    fn substring_char_boundary_safe() {
        // "你好" UTF-8: e4 bd a0 e5 a5 bd
        let s = from_str("你好");
        // 在 byte 1（"你" 的中间）切，会被裁回到 0
        let sub = qi_str_substring(s, 1, 2);
        // 实际拿到的是空（start=0 end=0 都被裁到 char 边界 0）
        // 或者拿到 ""
        // 不应 UB
        drop_str(sub);
        drop_str(s);
    }

    #[test]
    fn concat_basic() {
        let a = from_str("hello ");
        let b = from_str("world");
        let c = qi_str_concat(a, b);
        assert_eq!(unsafe { as_str_unchecked(&c) }, "hello world");
        drop_str(a);
        drop_str(b);
        drop_str(c);
    }

    #[test]
    fn find_basic() {
        let h = from_str("hello world");
        let n = from_str("world");
        assert_eq!(qi_str_find(h, n), 6);
        let nope = from_str("xyz");
        assert_eq!(qi_str_find(h, nope), -1);
        drop_str(h);
        drop_str(n);
        drop_str(nope);
    }

    #[test]
    fn equals_basic() {
        let a = from_str("hello");
        let b = from_str("hello");
        let c = from_str("world");
        assert_eq!(qi_str_equals(a, b), 1);
        assert_eq!(qi_str_equals(a, c), 0);
        drop_str(a);
        drop_str(b);
        drop_str(c);
    }

    #[test]
    fn starts_ends_with() {
        let s = from_str("hello world");
        let p = from_str("hello");
        let sf = from_str("world");
        assert_eq!(qi_str_starts_with(s, p), 1);
        assert_eq!(qi_str_ends_with(s, sf), 1);
        drop_str(s);
        drop_str(p);
        drop_str(sf);
    }

    #[test]
    fn from_cstr_roundtrip() {
        let c = CString::new("test").unwrap();
        let s = qi_str_from_cstr(c.as_ptr());
        assert_eq!(s.len, 4);
        assert_eq!(unsafe { as_str_unchecked(&s) }, "test");
        drop_str(s);
    }

    #[test]
    fn to_cstring_then_back() {
        let s = from_str("test");
        let c = qi_str_to_cstring(s);
        let s2 = qi_str_from_cstr(c);
        assert_eq!(unsafe { as_str_unchecked(&s2) }, "test");
        drop_str(s);
        drop_str(s2);
        // qi_str_to_cstring 现在返回 RC 指针，用 rc_cstr_release 释放
        crate::stdlib::qi_str::rc_cstr_release(c);
    }

    #[test]
    fn trim_zero_copy_owned() {
        let s = from_str("  hello  ");
        let parent_base = s.base;
        let t = qi_str_trim(s);
        assert_eq!(t.base, parent_base, "trim 应零拷贝");
        assert_eq!(unsafe { as_str_unchecked(&t) }, "hello");
        drop_str(s);
        drop_str(t);
    }

    #[test]
    fn replace_alloc_new() {
        let s = from_str("hello world hello");
        let from = from_str("hello");
        let to = from_str("HI");
        let r = qi_str_replace(s, from, to);
        assert_eq!(unsafe { as_str_unchecked(&r) }, "HI world HI");
        drop_str(s);
        drop_str(from);
        drop_str(to);
        drop_str(r);
    }

    #[test]
    fn to_upper_lower() {
        let s = from_str("Hello");
        let u = qi_str_to_upper(s);
        let l = qi_str_to_lower(s);
        assert_eq!(unsafe { as_str_unchecked(&u) }, "HELLO");
        assert_eq!(unsafe { as_str_unchecked(&l) }, "hello");
        drop_str(s);
        drop_str(u);
        drop_str(l);
    }
}
