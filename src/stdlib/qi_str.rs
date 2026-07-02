//! Qi 字符串新模型 — fat-pointer + refcounted shared backing
//!
//! 设计决策（grill 后落定）：
//!   - struct: `{ ptr: *const u8, len: i64, base: *const u8 }`，24 字节按值传
//!   - buffer header: `{ refcount: AtomicI64, capacity: i64 }`，16 字节，
//!     位于 `base - 16` 处
//!   - literals / borrows: `base = null`，clone/drop 走 null 旁路
//!   - UTF-8: 边界一次性验证（lossy），内部使用 `from_utf8_unchecked`
//!   - substring: 零拷贝，共享 backing；refcount++
//!
//! Buffer 内存布局：
//! ```
//! +-------------------+--------------------+
//! | refcount (i64)    | capacity (i64)     |
//! +-------------------+--------------------+ ← base
//! | data bytes (capacity 字节，UTF-8)       |
//! +----------------------------------------+
//! ```
//! base 总是指向 data 起点。header 在 `base - 16` 处。
//!
//! literals / borrows: ptr 指向某个数据区起点（可能是 .rodata，可能是子串
//! 偏移），base = null，永不参与 refcount。

#![allow(non_snake_case)]

use std::alloc::{alloc, dealloc, Layout};
use std::sync::atomic::{AtomicI64, Ordering};

/// Fat-pointer 字符串 — Qi 的新 字符串 类型在 ABI 上的表示
#[repr(C)]
#[derive(Copy, Clone)]
pub struct QiStr {
    /// 当前视图起点（字面量时指向 rodata，owned 时指向 buffer 数据区，substring 时指向偏移位置）
    pub ptr: *const u8,
    /// 当前视图字节长度
    pub len: i64,
    /// owning buffer 的 data 起点；null 表示这条字符串不参与 refcount
    /// （字面量、外部借入、substring of literal 等）
    pub base: *const u8,
}

unsafe impl Send for QiStr {}
unsafe impl Sync for QiStr {}

/// Owned buffer 的 header，位于 `base - 16`
#[repr(C)]
struct BufHeader {
    refcount: AtomicI64,
    capacity: i64,
}

const HEADER_SIZE: usize = std::mem::size_of::<BufHeader>(); // 16

#[inline]
unsafe fn header_of(base: *const u8) -> *const BufHeader {
    base.sub(HEADER_SIZE) as *const BufHeader
}

#[inline]
unsafe fn header_of_mut(base: *const u8) -> *mut BufHeader {
    base.sub(HEADER_SIZE) as *mut BufHeader
}

fn buffer_layout(capacity: usize) -> Layout {
    // +1 尾部 NUL：让 QiStr.ptr 永远是合法 C 字符串，可直接传给 c_char FFI
    // （strlen/CStr::from_ptr 安全，不越界）。capacity（header 内）仍是真实字节数。
    Layout::from_size_align(HEADER_SIZE + capacity + 1, 8).expect("invalid layout")
}

/// 分配一个 owned buffer，写入 data 内容，返回 QiStr (refcount = 1)
///
/// data 必须是合法 UTF-8（调用方保证 — 通常来自字面量、from_cstr 验证后、concat
/// 等不破坏 UTF-8 的操作）。
pub fn alloc_owned(data: &[u8]) -> QiStr {
    if data.is_empty() {
        return EMPTY;
    }
    let layout = buffer_layout(data.len());
    unsafe {
        let raw = alloc(layout);
        if raw.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        // 写 header
        (raw as *mut BufHeader).write(BufHeader {
            refcount: AtomicI64::new(1),
            capacity: data.len() as i64,
        });
        let data_ptr = raw.add(HEADER_SIZE);
        std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr, data.len());
        // 写尾部 NUL（buffer_layout 已多分配 1 字节），使 QiStr.ptr 是合法 C 字符串
        *data_ptr.add(data.len()) = 0;
        QiStr {
            ptr: data_ptr,
            len: data.len() as i64,
            base: data_ptr,
        }
    }
}

/// 借引用：refcount++（如果 owned），返回新的 QiStr 实例（共享同一 buffer）
pub fn clone(s: QiStr) -> QiStr {
    if !s.base.is_null() {
        unsafe {
            (*header_of(s.base))
                .refcount
                .fetch_add(1, Ordering::Relaxed);
        }
    }
    QiStr {
        ptr: s.ptr,
        len: s.len,
        base: s.base,
    }
}

/// 释放：refcount--（如果 owned），归零时 free buffer。literal/borrow 时 no-op
pub fn drop_str(s: QiStr) {
    if s.base.is_null() {
        return;
    }
    unsafe {
        let header = header_of_mut(s.base);
        let prev = (*header).refcount.fetch_sub(1, Ordering::Release);
        if prev == 1 {
            // 最后一个引用，释放
            std::sync::atomic::fence(Ordering::Acquire);
            let cap = (*header).capacity as usize;
            let layout = buffer_layout(cap);
            dealloc(header as *mut u8, layout);
        }
    }
}

/// 永远空的字面量字符串，base=null 不需要 free
pub const EMPTY: QiStr = QiStr {
    ptr: std::ptr::null(),
    len: 0,
    base: std::ptr::null(),
};

/// 从 &str 构造一个 owned QiStr（拷贝数据）
pub fn from_str(s: &str) -> QiStr {
    alloc_owned(s.as_bytes())
}

/// 从字节切片构造（lossy UTF-8，可能替换非法字节为 U+FFFD）
pub fn from_bytes_lossy(bytes: &[u8]) -> QiStr {
    match std::str::from_utf8(bytes) {
        Ok(_) => alloc_owned(bytes),
        Err(_) => {
            let lossy = String::from_utf8_lossy(bytes);
            alloc_owned(lossy.as_bytes())
        }
    }
}

/// 把 QiStr 当作 `&str`（**调用方负责保证 ptr/len 都合法且数据是合法 UTF-8**；
/// 这是 trust-the-invariant unchecked 路径，hot path 用）
#[inline]
pub unsafe fn as_str_unchecked(s: &QiStr) -> &str {
    if s.len <= 0 || s.ptr.is_null() {
        return "";
    }
    let bytes = std::slice::from_raw_parts(s.ptr, s.len as usize);
    std::str::from_utf8_unchecked(bytes)
}

/// 把 QiStr 当作 `&[u8]`（不需要 UTF-8 假设）
#[inline]
pub fn as_bytes(s: &QiStr) -> &[u8] {
    if s.len <= 0 || s.ptr.is_null() {
        return &[];
    }
    unsafe { std::slice::from_raw_parts(s.ptr, s.len as usize) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_safe_to_drop() {
        drop_str(EMPTY);
    }

    #[test]
    fn alloc_clone_drop_roundtrip() {
        let s = from_str("hello");
        assert_eq!(s.len, 5);
        assert_eq!(unsafe { as_str_unchecked(&s) }, "hello");

        let s2 = clone(s);
        assert_eq!(s2.ptr, s.ptr);

        drop_str(s);
        // s2 仍然有效
        assert_eq!(unsafe { as_str_unchecked(&s2) }, "hello");
        drop_str(s2);
    }

    #[test]
    fn refcount_at_one_releases() {
        let s = from_str("foo");
        let base = s.base;
        drop_str(s);
        // base 已释放 — 不能再访问，单纯确保不 panic 即可
        let _ = base;
    }

    #[test]
    fn substring_shares_backing() {
        let s = from_str("hello world");
        // 模拟 substring: 共享 base，refcount++
        let sub = clone(QiStr {
            ptr: unsafe { s.ptr.add(6) },
            len: 5,
            base: s.base,
        });
        assert_eq!(unsafe { as_str_unchecked(&sub) }, "world");
        // 父先 drop，sub 仍有效
        drop_str(s);
        assert_eq!(unsafe { as_str_unchecked(&sub) }, "world");
        drop_str(sub);
    }

    #[test]
    fn literal_no_refcount() {
        let lit = QiStr {
            ptr: b"static\0".as_ptr(),
            len: 6,
            base: std::ptr::null(),
        };
        // clone / drop 都是 no-op（不动 refcount，不释放）
        let lit2 = clone(lit);
        drop_str(lit);
        drop_str(lit2);
        // 重复 drop 也安全
        drop_str(QiStr {
            ptr: b"x".as_ptr(),
            len: 1,
            base: std::ptr::null(),
        });
    }

    #[test]
    fn empty_string_constant() {
        assert_eq!(EMPTY.len, 0);
        assert!(EMPTY.base.is_null());
        assert_eq!(unsafe { as_str_unchecked(&EMPTY) }, "");
    }

    #[test]
    fn from_bytes_lossy_handles_invalid() {
        let bad = &[0xFF, 0xFE, b'a'];
        let s = from_bytes_lossy(bad);
        // U+FFFD 替换符是 3 字节 UTF-8，2 个非法字节 → 2 个 FFFD + 'a'
        let st = unsafe { as_str_unchecked(&s) };
        assert!(st.ends_with('a'));
        assert!(st.chars().any(|c| c == '\u{FFFD}'));
        drop_str(s);
    }
}
