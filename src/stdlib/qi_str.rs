//! Qi 字符串新模型 — fat-pointer + refcounted shared backing
//!
//! 设计决策（grill 后落定）：
//!   - struct: `{ ptr: *const u8, len: i64, base: *const u8 }`，24 字节按值传
//!   - buffer header: `{ magic: u64, refcount: AtomicI64, capacity: i64 }`，
//!     24 字节，位于 `base - 24` 处
//!   - literals / borrows: `base = null`，clone/drop 走 null 旁路
//!   - UTF-8: 边界一次性验证（lossy），内部使用 `from_utf8_unchecked`
//!   - substring: 零拷贝，共享 backing；refcount++
//!
//! Buffer 内存布局：
//! ```text
//! +-------------------+-------------------+--------------------+
//! | magic (u64)       | refcount (i64)    | capacity (i64)     |
//! +-------------------+-------------------+--------------------+ ← base
//! | data bytes (capacity 字节，UTF-8) + 尾部 NUL                 |
//! +------------------------------------------------------------+
//! ```
//! base 总是指向 data 起点。header 在 `base - 24` 处。数据区尾部带 NUL，
//! 因此 base（以及 rc_cstr_* 返回的指针）本身就是合法 C 字符串。
//!
//! magic 用于 `i8*` C 字符串 ABI 的防御：`qi_string_free` /
//! `qi_string_retain` 收到的裸指针可能不是本分配器分配的（历史
//! `CString::into_raw`、外部库串），magic 不符时宁泄漏不崩溃。
//!
//! refcount >= IMMORTAL_RC 表示 immortal（字面量 emit 的全局常量），
//! 一切增减都 no-op、永不释放。
//!
//! literals / borrows: ptr 指向某个数据区起点（可能是 .rodata，可能是子串
//! 偏移），base = null，永不参与 refcount。

#![allow(non_snake_case)]

use std::alloc::{alloc, dealloc, Layout};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

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

/// RC buffer 的识别 magic — header 首 8 字节（"QISRC1" 变体 + 版本号）
pub const QI_STR_MAGIC: u64 = 0x5149_5352_4331_0001;

/// refcount >= 此值 ⇒ immortal：增减皆 no-op，永不释放。
/// codegen 会把字面量 emit 成 refcount = IMMORTAL_RC 的全局常量。
pub const IMMORTAL_RC: i64 = 1 << 61;

/// Owned buffer 的 header，位于 `base - 24`
#[repr(C)]
struct BufHeader {
    magic: u64,
    refcount: AtomicI64,
    capacity: i64,
}

const HEADER_SIZE: usize = std::mem::size_of::<BufHeader>(); // 24

// 布局铁闸：header 必须正好 24 字节（magic/refcount/capacity 各 8）
const _: () = assert!(std::mem::size_of::<BufHeader>() == 24);

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
            magic: QI_STR_MAGIC,
            refcount: AtomicI64::new(1),
            capacity: data.len() as i64,
        });
        let data_ptr = raw.add(HEADER_SIZE);
        std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr, data.len());
        // 写尾部 NUL（buffer_layout 已多分配 1 字节），使 QiStr.ptr 是合法 C 字符串
        *data_ptr.add(data.len()) = 0;
        super::rc_obj::diag_str_alloc();
        QiStr {
            ptr: data_ptr,
            len: data.len() as i64,
            base: data_ptr,
        }
    }
}

// ============================================================================
// retain / release 核心 —— QiStr fat-pointer 路径与 rc_cstr 裸指针路径共用
// ============================================================================

/// 进程级一次性防御日志开关（magic 不符的裸指针 → 警告一次，之后静默泄漏）
static NON_RC_WARNED: AtomicBool = AtomicBool::new(false);

#[cold]
fn warn_non_rc_pointer_once() {
    if !NON_RC_WARNED.swap(true, Ordering::Relaxed) {
        eprintln!("qi_string_free: 非 RC 指针,已忽略(后续同类情况将静默泄漏,不再重复警告)");
    }
}

/// retain 核心：base 指向 data 起点（header 在 base-24）。
/// magic 不符 → 一次性警告后 no-op；immortal → no-op；否则 refcount++。
/// OBJ magic（rc_obj 结构体/数组本体，header 同形）→ 委托对象 retain
/// —— 数组<指针>/类型混淆路径下串与对象互认，计数始终平衡。
///
/// # Safety
/// base 非 null，且 base-24 起 24 字节可读（RC buffer 天然满足；
/// 非 RC 的 malloc 指针依赖分配器 header 区域可读 —— 这是 magic 防御的前提）。
#[inline]
unsafe fn retain_base(base: *const u8) {
    let header = header_of(base);
    if (*header).magic != QI_STR_MAGIC {
        if (*header).magic == super::rc_obj::QI_OBJ_MAGIC {
            super::rc_obj::obj_retain_raw(base);
            return;
        }
        if (*header).magic == super::closure_ffi::QI_CLO_MAGIC {
            super::closure_ffi::clo_retain_raw(base);
            return;
        }
        warn_non_rc_pointer_once();
        return;
    }
    if (*header).refcount.load(Ordering::Relaxed) >= IMMORTAL_RC {
        return;
    }
    (*header).refcount.fetch_add(1, Ordering::Relaxed);
}

/// release 核心：magic 不符 → 一次性警告后直接返回（宁泄漏不崩溃，铁律）；
/// immortal → no-op；否则 refcount--，前值 == 1 时按 capacity 释放整个 buffer。
/// OBJ magic → 委托对象**浅**释放（归零只回收本体，字段泄漏 —— 宁泄漏不崩）。
///
/// # Safety
/// 同 [`retain_base`]。
#[inline]
unsafe fn release_base(base: *const u8) {
    let header = header_of_mut(base);
    if (*header).magic != QI_STR_MAGIC {
        if (*header).magic == super::rc_obj::QI_OBJ_MAGIC {
            super::rc_obj::obj_release_shallow(base);
            return;
        }
        if (*header).magic == super::closure_ffi::QI_CLO_MAGIC {
            super::closure_ffi::clo_release_raw(base);
            return;
        }
        warn_non_rc_pointer_once();
        return;
    }
    if (*header).refcount.load(Ordering::Relaxed) >= IMMORTAL_RC {
        return;
    }
    let prev = (*header).refcount.fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        // 最后一个引用，释放
        let cap = (*header).capacity as usize;
        let layout = buffer_layout(cap);
        dealloc(header as *mut u8, layout);
        super::rc_obj::diag_str_free();
    }
}

/// 借引用：refcount++（如果 owned 且非 immortal），返回新的 QiStr 实例（共享同一 buffer）
pub fn clone(s: QiStr) -> QiStr {
    if !s.base.is_null() {
        unsafe {
            retain_base(s.base);
        }
    }
    QiStr {
        ptr: s.ptr,
        len: s.len,
        base: s.base,
    }
}

/// 释放：refcount--（如果 owned 且非 immortal），归零时 free buffer。
/// literal/borrow（base=null）与 immortal 时 no-op
pub fn drop_str(s: QiStr) {
    if s.base.is_null() {
        return;
    }
    unsafe {
        release_base(s.base);
    }
}

// ============================================================================
// rc_cstr —— 隐藏 header 引用计数 C 字符串（`i8*` ABI 不变，ptr-24 藏 header）
// ============================================================================

/// 静态 immortal 空串 buffer —— rc_cstr_from_bytes(b"") 的返回目标。
/// 布局与 BufHeader + data 完全一致（repr(C)，8 字节对齐），data 在偏移 24。
#[repr(C)]
struct StaticEmptyBuf {
    magic: u64,
    refcount: AtomicI64,
    capacity: i64,
    data: [u8; 1],
}

static RC_CSTR_EMPTY: StaticEmptyBuf = StaticEmptyBuf {
    magic: QI_STR_MAGIC,
    refcount: AtomicI64::new(IMMORTAL_RC),
    capacity: 0,
    data: [0u8],
};

/// 从字节切片分配一个 RC C 字符串，返回 data 指针（`*mut c_char` ABI）。
///
/// - 空输入 → 返回静态 immortal 空 buffer 的 data 指针（retain/release 皆 no-op）
/// - 数据含内部 NUL 时照存（C 侧 strlen 语义自然截断，不 panic）
/// - 返回的指针满足：ptr-24 处有合法 header，尾部带 NUL，可安全传给
///   `qi_string_retain` / `qi_string_free` 系列
pub fn rc_cstr_from_bytes(data: &[u8]) -> *mut c_char {
    if data.is_empty() {
        // 不走 alloc_owned 的 EMPTY 短路（那个 base=null 无 header）
        return RC_CSTR_EMPTY.data.as_ptr() as *mut c_char;
    }
    alloc_owned(data).ptr as *mut c_char
}

/// 便利函数：从 &str 分配 RC C 字符串
#[inline]
pub fn rc_cstr_from_str(s: &str) -> *mut c_char {
    rc_cstr_from_bytes(s.as_bytes())
}

/// 便利函数：从 String 分配 RC C 字符串（拷贝后丢弃原 String）
#[inline]
pub fn rc_cstr_from_string(s: String) -> *mut c_char {
    rc_cstr_from_bytes(s.as_bytes())
}

/// C FFI 边界：把一个裸 C 字符串（`*const c_char`，来自外部 C 库如 getenv）
/// **拷贝**进一条 Qi 拥有的 RC 堆串，返回 data 指针（ptr-24 带 magic header，rc=1）。
///
/// 语义与安全：Qi 只拥有这份拷贝，按正常 ARC 释放；**原 C 内存一概不碰**
/// （不 free、不改），因此 getenv 之类静态/借来的内存安全，C `malloc` 出来的原串
/// 所有权仍归 C（用户用 `指针` 绑定 + 外部 free 手动回收）。null → immortal 空串。
/// UTF-8 非法字节按 lossy 替换（U+FFFD），保证 QiStr 的 UTF-8 公约。
#[no_mangle]
pub extern "C" fn qi_string_from_cstr(p: *const c_char) -> *mut c_char {
    if p.is_null() {
        return rc_cstr_from_str("");
    }
    let bytes = unsafe { CStr::from_ptr(p).to_bytes() };
    match std::str::from_utf8(bytes) {
        Ok(s) => rc_cstr_from_str(s),
        Err(_) => rc_cstr_from_string(String::from_utf8_lossy(bytes).into_owned()),
    }
}

/// 增引用（C ABI）：null / magic 不符 / immortal 皆 no-op
#[no_mangle]
pub extern "C" fn qi_string_retain(s: *const c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        retain_base(s as *const u8);
    }
}

/// 减引用：null → 返回；magic 不符 → 一次性防御日志后返回（宁泄漏不崩溃）；
/// immortal → 返回；否则 refcount--，归零时释放整个 buffer（含 header）。
/// 各模块的 `qi_*_free_string` 全部委托到这里。
pub fn rc_cstr_release(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        release_base(s as *const u8);
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
    fn header_layout_is_24_bytes() {
        assert_eq!(HEADER_SIZE, 24);
        // 静态空 buffer 的 data 必须正好在偏移 HEADER_SIZE 处（与 alloc 布局一致）
        let base = &RC_CSTR_EMPTY as *const StaticEmptyBuf as usize;
        let data = RC_CSTR_EMPTY.data.as_ptr() as usize;
        assert_eq!(data - base, HEADER_SIZE);
        // 8 字节对齐
        assert_eq!(base % 8, 0);
    }

    #[test]
    fn alloc_owned_writes_magic() {
        let s = from_str("magic-check");
        unsafe {
            let h = header_of(s.base);
            assert_eq!((*h).magic, QI_STR_MAGIC);
            assert_eq!((*h).capacity, 11);
            assert_eq!((*h).refcount.load(Ordering::Relaxed), 1);
        }
        drop_str(s);
    }

    #[test]
    fn rc_cstr_retain_release_roundtrip() {
        let p = rc_cstr_from_bytes(b"hello rc");
        assert!(!p.is_null());
        unsafe {
            assert_eq!(std::ffi::CStr::from_ptr(p).to_str().unwrap(), "hello rc");
            let h = header_of(p as *const u8);
            assert_eq!((*h).refcount.load(Ordering::Relaxed), 1);
        }
        qi_string_retain(p);
        unsafe {
            let h = header_of(p as *const u8);
            assert_eq!((*h).refcount.load(Ordering::Relaxed), 2);
        }
        rc_cstr_release(p); // 2 → 1
        unsafe {
            assert_eq!(std::ffi::CStr::from_ptr(p).to_bytes(), b"hello rc");
        }
        rc_cstr_release(p); // 1 → 0，释放，不崩
    }

    #[test]
    fn rc_cstr_empty_is_immortal() {
        let p1 = rc_cstr_from_bytes(b"");
        let p2 = rc_cstr_from_bytes(b"");
        assert_eq!(p1, p2, "空串应返回同一个静态 buffer");
        unsafe {
            assert_eq!(*p1, 0, "空串 data 是单个 NUL");
        }
        // 任意次 retain/release 皆 no-op，refcount 不变
        qi_string_retain(p1);
        rc_cstr_release(p1);
        rc_cstr_release(p1);
        rc_cstr_release(p1);
        assert_eq!(RC_CSTR_EMPTY.refcount.load(Ordering::Relaxed), IMMORTAL_RC);
        unsafe {
            assert_eq!(*p1, 0, "释放后仍可读（immortal 永不释放）");
        }
    }

    #[test]
    fn rc_cstr_release_foreign_pointer_no_crash() {
        // 用 CString::into_raw 造一个非 RC 指针 —— release/retain 只警告不崩不释放
        let raw = std::ffi::CString::new("foreign").unwrap().into_raw();
        qi_string_retain(raw);
        rc_cstr_release(raw);
        rc_cstr_release(raw);
        unsafe {
            // 指针未被释放、内容未被改动
            assert_eq!(std::ffi::CStr::from_ptr(raw).to_bytes(), b"foreign");
            // 归还给 CString 正常释放，避免测试泄漏
            let _ = std::ffi::CString::from_raw(raw);
        }
    }

    #[test]
    fn rc_cstr_interior_nul_allocates_full_bytes() {
        let p = rc_cstr_from_bytes(b"ab\0cd");
        unsafe {
            // C 侧 strlen 语义：在内部 NUL 处截断
            assert_eq!(std::ffi::CStr::from_ptr(p).to_bytes(), b"ab");
            // 但 buffer 完整存了 5 字节（capacity 是真实字节数）
            let h = header_of(p as *const u8);
            assert_eq!((*h).capacity, 5);
            let full = std::slice::from_raw_parts(p as *const u8, 5);
            assert_eq!(full, b"ab\0cd");
            // 尾部 NUL 仍在
            assert_eq!(*(p as *const u8).add(5), 0);
        }
        rc_cstr_release(p);
    }

    #[test]
    fn immortal_refcount_never_changes() {
        // 手工把一个 owned buffer 置成 immortal，clone/drop 皆 no-op
        let s = from_str("immortal-test");
        unsafe {
            (*header_of_mut(s.base))
                .refcount
                .store(IMMORTAL_RC, Ordering::Relaxed);
        }
        let s2 = clone(s);
        drop_str(s);
        drop_str(s2);
        drop_str(s2);
        unsafe {
            let h = header_of(s.base);
            assert_eq!((*h).refcount.load(Ordering::Relaxed), IMMORTAL_RC);
            // buffer 未被释放，数据仍可读
            assert_eq!(as_str_unchecked(&s), "immortal-test");
        }
        // 故意泄漏（immortal 语义如此）
    }

    #[test]
    fn rc_cstr_from_str_and_string() {
        let p1 = rc_cstr_from_str("你好");
        let p2 = rc_cstr_from_string(String::from("世界"));
        unsafe {
            assert_eq!(std::ffi::CStr::from_ptr(p1).to_str().unwrap(), "你好");
            assert_eq!(std::ffi::CStr::from_ptr(p2).to_str().unwrap(), "世界");
        }
        rc_cstr_release(p1);
        rc_cstr_release(p2);
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
