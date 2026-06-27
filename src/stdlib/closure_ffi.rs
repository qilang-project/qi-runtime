//! 闭包对象 FFI
//!
//! 闭包对象布局：
//!   offset 0..8  : function pointer (调用闭包时 jump 到这里)
//!   offset 8..16 : capture[0]
//!   offset 16..24: capture[1]
//!   ...
//!
//! 整数和指针都按 i64 槽存（64-bit 平台 ptr/i64 等大）

#![allow(non_snake_case)]

use std::alloc::{alloc_zeroed, Layout};
use std::ffi::c_void;

const SLOT_SIZE: usize = 8;

fn obj_layout(num_caps: i64) -> Layout {
    let n = num_caps.max(0) as usize;
    let bytes = SLOT_SIZE * (1 + n);
    // 8-byte alignment for both ptr and i64
    Layout::from_size_align(bytes, SLOT_SIZE).unwrap()
}

/// 创建闭包对象，写入函数指针，捕获槽留 0 待填
#[no_mangle]
pub extern "C" fn qi_closure_create(fn_ptr: *const c_void, num_caps: i64) -> *mut c_void {
    unsafe {
        let layout = obj_layout(num_caps);
        let raw = alloc_zeroed(layout);
        if raw.is_null() {
            return std::ptr::null_mut();
        }
        // 写 fn_ptr 到 offset 0
        *(raw as *mut *const c_void) = fn_ptr;
        raw as *mut c_void
    }
}

/// 取闭包的函数指针（offset 0）— 调用闭包时用
#[no_mangle]
pub extern "C" fn qi_closure_get_fn(env: *const c_void) -> *const c_void {
    if env.is_null() {
        return std::ptr::null();
    }
    unsafe { *(env as *const *const c_void) }
}

/// 写一个 i64 槽
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

/// 读一个 i64 槽（闭包函数序言用）
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

/// 写一个 ptr 槽（捕获字符串、结构体等）
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
