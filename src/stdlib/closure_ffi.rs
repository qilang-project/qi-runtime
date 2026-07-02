//! 闭包对象 FFI —— RC 化 fat 对象（Round E）
//!
//! 闭包对象布局（data 区；ptr-24 处藏 RC header，与 qi_str/rc_obj 同形）：
//!   slot 0        : function pointer (调用闭包时 jump 到这里)
//!   slot 1..=n    : capture[0..n-1]
//!   slot n+1(末槽): dtor 指针（codegen 合成的捕获释放函数；0 = 无 RC 捕获）
//!
//! 整数和指针都按 i64 槽存（64-bit 平台 ptr/i64 等大）。
//!
//! header 布局（与 BufHeader/ObjHeader 完全同形，24 字节，8 对齐）：
//! ```text
//! | magic (u64) | refcount (i64) | size (i64) | ← data（返回给 codegen 的指针）
//! ```
//! - magic = QI_CLO_MAGIC，跨 magic 与字符串/对象 RC 互认（见 rc_obj.rs / qi_str.rs）。
//! - size = data 区字节数 = 8 * (2 + num_caps)，release 时据此定位末槽 dtor 与布局。
//! - refcount 归零：先调 dtor(env)（级联释放 RC 捕获），再 free 本体。
//! - dtor 由 codegen 在填完捕获后 `qi_closure_set_dtor` 写入；QI_ARC=0 时
//!   codegen 不写也不 release —— 行为与旧版一致（纯泄漏）。
//!
//! ABI 兼容性：get/set 槽的下标语义不变（capture i 在 slot 1+i）；
//! dtor 藏在**末槽**，不影响任何既有下标。

#![allow(non_snake_case)]

use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::ffi::c_void;
use std::sync::atomic::{AtomicI64, Ordering};

use super::qi_str::IMMORTAL_RC;

/// 闭包 fat 对象识别 magic —— "QICLOS1" 变体 + 版本号（≠ STR/OBJ magic）。
pub const QI_CLO_MAGIC: u64 = 0x5149_434C_4F53_0001;

const SLOT_SIZE: usize = 8;
const HEADER_SIZE: usize = 24;

/// 闭包 header（位于 data-24 处；与 qi_str::BufHeader / rc_obj::ObjHeader 同形）。
#[repr(C)]
struct CloHeader {
    magic: u64,
    refcount: AtomicI64,
    /// data 区字节数（不含 header）= 8 * (2 + num_caps)。
    size: i64,
}

// 布局铁闸：header 必须正好 24 字节（跨 magic 委托的前提）
const _: () = assert!(std::mem::size_of::<CloHeader>() == 24);

#[inline]
unsafe fn header_of(data: *const u8) -> *mut CloHeader {
    data.sub(HEADER_SIZE) as *mut CloHeader
}

fn clo_layout(size: usize) -> Layout {
    Layout::from_size_align(HEADER_SIZE + size, 8).expect("invalid closure layout")
}

/// 创建闭包对象（refcount=1）：写入函数指针，捕获槽 + dtor 槽零初始化待填。
#[no_mangle]
pub extern "C" fn qi_closure_create(fn_ptr: *const c_void, num_caps: i64) -> *mut c_void {
    let n = num_caps.max(0) as usize;
    // fn_ptr + n 个捕获 + 末槽 dtor
    let size = SLOT_SIZE * (2 + n);
    let layout = clo_layout(size);
    unsafe {
        let raw = alloc_zeroed(layout);
        if raw.is_null() {
            return std::ptr::null_mut();
        }
        let h = raw as *mut CloHeader;
        (*h).magic = QI_CLO_MAGIC;
        (*h).refcount = AtomicI64::new(1);
        (*h).size = size as i64;
        let data = raw.add(HEADER_SIZE);
        // 写 fn_ptr 到 slot 0
        *(data as *mut *const c_void) = fn_ptr;
        super::rc_obj::diag_clo_alloc();
        data as *mut c_void
    }
}

/// 取闭包的函数指针（slot 0）— 调用闭包时用
#[no_mangle]
pub extern "C" fn qi_closure_get_fn(env: *const c_void) -> *const c_void {
    if env.is_null() {
        return std::ptr::null();
    }
    unsafe { *(env as *const *const c_void) }
}

/// 写一个 i64 捕获槽
#[no_mangle]
pub extern "C" fn qi_closure_set_int(env: *mut c_void, idx: i64, val: i64) {
    if env.is_null() || idx < 0 {
        return;
    }
    unsafe {
        let base = (env as *mut i64).add(1 + idx as usize);
        *base = val;
    }
}

/// 读一个 i64 捕获槽（闭包函数序言用）
#[no_mangle]
pub extern "C" fn qi_closure_get_int(env: *const c_void, idx: i64) -> i64 {
    if env.is_null() || idx < 0 {
        return 0;
    }
    unsafe {
        let base = (env as *const i64).add(1 + idx as usize);
        *base
    }
}

/// 写一个 ptr 捕获槽（捕获字符串、结构体等）
#[no_mangle]
pub extern "C" fn qi_closure_set_ptr(env: *mut c_void, idx: i64, val: *const c_void) {
    if env.is_null() || idx < 0 {
        return;
    }
    unsafe {
        let base = (env as *mut *const c_void).add(1 + idx as usize);
        *base = val;
    }
}

#[no_mangle]
pub extern "C" fn qi_closure_get_ptr(env: *const c_void, idx: i64) -> *const c_void {
    if env.is_null() || idx < 0 {
        return std::ptr::null();
    }
    unsafe {
        let base = (env as *const *const c_void).add(1 + idx as usize);
        *base
    }
}

/// 写 dtor（末槽）：codegen 在填完捕获后调用。仅 CLO magic 生效，其余静默忽略。
#[no_mangle]
pub extern "C" fn qi_closure_set_dtor(env: *mut c_void, dtor: *const c_void) {
    if env.is_null() {
        return;
    }
    unsafe {
        let h = header_of(env as *const u8);
        if (*h).magic != QI_CLO_MAGIC {
            return;
        }
        let slots = ((*h).size as usize) / SLOT_SIZE;
        if slots < 2 {
            return;
        }
        let base = (env as *mut *const c_void).add(slots - 1);
        *base = dtor;
    }
}

/// 闭包浅 retain（供跨 magic 委托；调用方已验过 CLO magic）。
///
/// # Safety
/// data-24 起 24 字节必须是合法 CloHeader。
pub(crate) unsafe fn clo_retain_raw(data: *const u8) {
    let h = header_of(data);
    if (*h).refcount.load(Ordering::Relaxed) >= IMMORTAL_RC {
        return;
    }
    (*h).refcount.fetch_add(1, Ordering::Relaxed);
}

/// 闭包完整 release（调用方已验过 CLO magic）：refcount--，归零时先调 dtor
/// （级联释放 RC 捕获），再 free 本体。
///
/// # Safety
/// 同 [`clo_retain_raw`]。
pub(crate) unsafe fn clo_release_raw(data: *const u8) {
    let h = header_of(data);
    if (*h).refcount.load(Ordering::Relaxed) >= IMMORTAL_RC {
        return;
    }
    let prev = (*h).refcount.fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        let size = (*h).size as usize;
        let slots = size / SLOT_SIZE;
        if slots >= 2 {
            let dtor = *(data as *const *const c_void).add(slots - 1);
            if !dtor.is_null() {
                let f: extern "C" fn(*const c_void) = std::mem::transmute(dtor);
                f(data as *const c_void);
            }
        }
        dealloc(h as *mut u8, clo_layout(size));
        super::rc_obj::diag_clo_free();
    }
}

/// 增引用（动态派发）：null no-op；CLO → refcount++；STR/OBJ → 委托对方 retain；
/// 其余 → 静默 no-op（宁泄漏不崩）。
#[no_mangle]
pub extern "C" fn qi_closure_retain(p: *const c_void) {
    if p.is_null() {
        return;
    }
    unsafe {
        let data = p as *const u8;
        let magic = *(data.sub(HEADER_SIZE) as *const u64);
        if magic == QI_CLO_MAGIC {
            clo_retain_raw(data);
        } else if magic == super::rc_obj::QI_OBJ_MAGIC {
            super::rc_obj::obj_retain_raw(data);
        } else if magic == super::qi_str::QI_STR_MAGIC {
            super::qi_str::qi_string_retain(p as *const std::os::raw::c_char);
        }
        // 其余：静默（函数值槽可能混入外部句柄）
    }
}

/// 减引用（动态派发）—— codegen 的 函数值 类型 release 入口：
/// - CLO → 完整 release（归零调 dtor + free）
/// - STR → 字符串完整 release
/// - OBJ → 对象浅 release
/// - null / 其余 → 静默 no-op
#[no_mangle]
pub extern "C" fn qi_closure_release(p: *const c_void) {
    if p.is_null() {
        return;
    }
    unsafe {
        let data = p as *const u8;
        let magic = *(data.sub(HEADER_SIZE) as *const u64);
        if magic == QI_CLO_MAGIC {
            clo_release_raw(data);
        } else if magic == super::rc_obj::QI_OBJ_MAGIC {
            super::rc_obj::obj_release_shallow(data);
        } else if magic == super::qi_str::QI_STR_MAGIC {
            super::qi_str::rc_cstr_release(p as *mut std::os::raw::c_char);
        }
        // 其余：静默泄漏
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    unsafe fn rc_of(p: *const c_void) -> i64 {
        (*header_of(p as *const u8))
            .refcount
            .load(Ordering::Relaxed)
    }

    #[test]
    fn create_writes_header_and_fn_ptr() {
        let f = 0xDEAD_0000usize as *const c_void;
        let env = qi_closure_create(f, 3);
        assert!(!env.is_null());
        assert_eq!(qi_closure_get_fn(env), f);
        unsafe {
            let h = header_of(env as *const u8);
            assert_eq!((*h).magic, QI_CLO_MAGIC);
            assert_eq!((*h).size, 8 * 5); // fn + 3 caps + dtor
            assert_eq!(rc_of(env), 1);
        }
        // 捕获槽 + dtor 槽零初始化
        assert_eq!(qi_closure_get_int(env, 0), 0);
        assert_eq!(qi_closure_get_int(env, 2), 0);
        qi_closure_release(env);
    }

    #[test]
    fn set_get_slots_roundtrip() {
        let env = qi_closure_create(std::ptr::null(), 2);
        qi_closure_set_int(env, 0, 42);
        let s = b"x\0".as_ptr() as *const c_void;
        qi_closure_set_ptr(env, 1, s);
        assert_eq!(qi_closure_get_int(env, 0), 42);
        assert_eq!(qi_closure_get_ptr(env, 1), s);
        qi_closure_release(env);
    }

    #[test]
    fn retain_release_roundtrip() {
        let env = qi_closure_create(std::ptr::null(), 0);
        qi_closure_retain(env);
        unsafe {
            assert_eq!(rc_of(env), 2);
        }
        qi_closure_release(env); // 2 → 1
        unsafe {
            assert_eq!(rc_of(env), 1);
        }
        qi_closure_release(env); // 1 → 0 释放，不崩
    }

    static DTOR_CALLS: AtomicUsize = AtomicUsize::new(0);
    static DTOR_LAST_ENV: AtomicUsize = AtomicUsize::new(0);

    extern "C" fn test_dtor(env: *const c_void) {
        DTOR_CALLS.fetch_add(1, Ordering::SeqCst);
        DTOR_LAST_ENV.store(env as usize, Ordering::SeqCst);
    }

    #[test]
    fn dtor_called_exactly_once_on_last_release() {
        DTOR_CALLS.store(0, Ordering::SeqCst);
        let env = qi_closure_create(std::ptr::null(), 1);
        qi_closure_set_int(env, 0, 7);
        qi_closure_set_dtor(env, test_dtor as *const c_void);
        // dtor 藏末槽，不影响捕获槽
        assert_eq!(qi_closure_get_int(env, 0), 7);
        qi_closure_retain(env);
        qi_closure_release(env); // 2 → 1，不调 dtor
        assert_eq!(DTOR_CALLS.load(Ordering::SeqCst), 0);
        qi_closure_release(env); // 1 → 0：先 dtor 后 free
        assert_eq!(DTOR_CALLS.load(Ordering::SeqCst), 1);
        assert_eq!(DTOR_LAST_ENV.load(Ordering::SeqCst), env as usize);
    }

    #[test]
    fn release_without_dtor_is_safe() {
        let env = qi_closure_create(std::ptr::null(), 4);
        qi_closure_release(env); // dtor 槽为 0 → 直接 free
    }

    #[test]
    fn null_and_foreign_pointers_safe() {
        qi_closure_retain(std::ptr::null());
        qi_closure_release(std::ptr::null());
        qi_closure_set_dtor(std::ptr::null_mut(), std::ptr::null());
        // 非 RC 指针：全部静默 no-op，不崩不改内容
        let raw = std::ffi::CString::new("foreign-closure")
            .unwrap()
            .into_raw();
        qi_closure_retain(raw as *const c_void);
        qi_closure_release(raw as *const c_void);
        qi_closure_set_dtor(raw as *mut c_void, test_dtor as *const c_void);
        unsafe {
            assert_eq!(std::ffi::CStr::from_ptr(raw).to_bytes(), b"foreign-closure");
            let _ = std::ffi::CString::from_raw(raw);
        }
    }

    #[test]
    fn cross_magic_delegation() {
        // 字符串指针进闭包 retain/release → 委托字符串 RC
        let s = crate::stdlib::qi_str::rc_cstr_from_bytes(b"clo-cross");
        qi_closure_retain(s as *const c_void); // 1 → 2
        qi_closure_release(s as *const c_void); // 2 → 1
        unsafe {
            assert_eq!(std::ffi::CStr::from_ptr(s).to_bytes(), b"clo-cross");
        }
        crate::stdlib::qi_str::rc_cstr_release(s);
        // 对象指针进闭包 release → 浅释放
        let p = crate::stdlib::rc_obj::qi_obj_alloc(16);
        qi_closure_retain(p as *const c_void);
        qi_closure_release(p as *const c_void); // 2 → 1
        qi_closure_release(p as *const c_void); // 1 → 0 浅释放本体
    }

    #[test]
    fn concurrent_retain_release_balanced() {
        let env = qi_closure_create(std::ptr::null(), 0) as usize;
        let mut handles = Vec::new();
        for _ in 0..8 {
            handles.push(std::thread::spawn(move || {
                for _ in 0..1000 {
                    qi_closure_retain(env as *const c_void);
                    qi_closure_release(env as *const c_void);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        unsafe {
            assert_eq!(rc_of(env as *const c_void), 1);
        }
        qi_closure_release(env as *const c_void);
    }
}
