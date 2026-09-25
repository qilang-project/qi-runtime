//! 宿主层（wasm 版）—— 主运行时 `executor.rs` 与 `io/io_ffi.rs` 里跟进程环境绑定的那部分
//!
//! 主运行时的 executor 抱着一个 RuntimeEnvironment（统计、GC 根、配置），再往下是
//! tokio。wasm 里这些都不存在，但 codegen 发射的符号名不变，所以这里按同名同签名
//! 重写一份「平凡版」：打印直接走 wasi stdout，分配走全局分配器，GC 全部空转
//! （qi 的字符串/对象靠 ARC，GC 钩子本来就只在旧路径上有意义），等待组与互斥在
//! 单线程里退化成计数器。
//!
//! 每行打印都 flush：wasm 里没有进程退出时的 atexit 冲刷保证（浏览器 shim 更没有），
//! 不 flush 就会出现「程序跑完了但一个字没有」。

use std::ffi::CStr;
use std::io::Write;
use std::os::raw::{c_char, c_int};

// ── 生命周期 ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn qi_runtime_initialize() -> c_int {
    0
}

#[no_mangle]
pub extern "C" fn qi_runtime_shutdown() -> c_int {
    qi_runtime_flush_stdout();
    0
}

#[no_mangle]
pub extern "C" fn qi_runtime_flush_stdout() {
    let _ = std::io::stdout().flush();
}

// ── 打印 ────────────────────────────────────────────────────────────

fn write_out(s: &str, newline: bool) -> c_int {
    let out = std::io::stdout();
    let mut lock = out.lock();
    let r = if newline {
        lock.write_all(s.as_bytes())
            .and_then(|_| lock.write_all(b"\n"))
    } else {
        lock.write_all(s.as_bytes())
    };
    let _ = lock.flush();
    if r.is_ok() {
        0
    } else {
        -1
    }
}

fn write_err(s: &str, newline: bool) -> c_int {
    let err = std::io::stderr();
    let mut lock = err.lock();
    let r = if newline {
        lock.write_all(s.as_bytes())
            .and_then(|_| lock.write_all(b"\n"))
    } else {
        lock.write_all(s.as_bytes())
    };
    let _ = lock.flush();
    if r.is_ok() {
        0
    } else {
        -1
    }
}

unsafe fn cstr_lossy<'a>(s: *const c_char) -> std::borrow::Cow<'a, str> {
    CStr::from_ptr(s).to_string_lossy()
}

#[no_mangle]
pub unsafe extern "C" fn qi_runtime_print(s: *const c_char) -> c_int {
    if s.is_null() {
        return -1;
    }
    write_out(&cstr_lossy(s), false)
}

#[no_mangle]
pub unsafe extern "C" fn qi_runtime_println(s: *const c_char) -> c_int {
    if s.is_null() {
        return -1;
    }
    write_out(&cstr_lossy(s), true)
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_eprint(s: *const c_char) -> c_int {
    if s.is_null() {
        return -1;
    }
    write_err(&cstr_lossy(s), false)
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_eprintln(s: *const c_char) -> c_int {
    if s.is_null() {
        return -1;
    }
    write_err(&cstr_lossy(s), true)
}

#[no_mangle]
pub extern "C" fn qi_runtime_print_int(value: i64) -> c_int {
    write_out(&value.to_string(), false)
}

#[no_mangle]
pub extern "C" fn qi_runtime_println_int(value: i64) -> c_int {
    write_out(&value.to_string(), true)
}

#[no_mangle]
pub extern "C" fn qi_runtime_print_float(value: f64) -> c_int {
    write_out(&value.to_string(), false)
}

/// 与主运行时一致：整值浮点带一位小数（`12.0`），非整值按默认格式
#[no_mangle]
pub extern "C" fn qi_runtime_println_float(value: f64) -> c_int {
    if value.fract() == 0.0 {
        write_out(&format!("{:.1}", value), true)
    } else {
        write_out(&value.to_string(), true)
    }
}

#[no_mangle]
pub extern "C" fn qi_runtime_print_bool(value: i32) -> c_int {
    write_out(if value != 0 { "真" } else { "假" }, false)
}

#[no_mangle]
pub extern "C" fn qi_runtime_println_bool(value: i32) -> c_int {
    write_out(if value != 0 { "真" } else { "假" }, true)
}

// ── 字符串 / 数值转换（与 executor.rs 逐字节同义）───────────────────

#[no_mangle]
pub unsafe extern "C" fn qi_runtime_string_concat(
    s1: *const c_char,
    s2: *const c_char,
) -> *mut c_char {
    crate::stdlib::str_format::concat2(s1, s2)
}

#[no_mangle]
pub extern "C" fn qi_runtime_int_to_string(value: i64) -> *mut c_char {
    crate::stdlib::str_format::rc_cstr_from_i64(value)
}

#[no_mangle]
pub extern "C" fn qi_runtime_float_to_string(value: f64) -> *mut c_char {
    crate::stdlib::str_format::rc_cstr_from_f64(value)
}

#[no_mangle]
pub unsafe extern "C" fn qi_runtime_string_to_int(s: *const c_char) -> i64 {
    if s.is_null() {
        return 0;
    }
    CStr::from_ptr(s)
        .to_str()
        .ok()
        .and_then(|t| t.parse::<i64>().ok())
        .unwrap_or(0)
}

#[no_mangle]
pub unsafe extern "C" fn qi_runtime_string_to_float(s: *const c_char) -> f64 {
    if s.is_null() {
        return 0.0;
    }
    CStr::from_ptr(s)
        .to_str()
        .ok()
        .and_then(|t| t.parse::<f64>().ok())
        .unwrap_or(0.0)
}

#[no_mangle]
pub unsafe extern "C" fn qi_runtime_string_compare(s1: *const c_char, s2: *const c_char) -> c_int {
    if s1.is_null() || s2.is_null() {
        return -1;
    }
    match (CStr::from_ptr(s1).to_str(), CStr::from_ptr(s2).to_str()) {
        (Ok(a), Ok(b)) => a.cmp(b) as c_int,
        _ => -1,
    }
}

#[no_mangle]
pub extern "C" fn qi_runtime_int_to_float(value: i64) -> f64 {
    value as f64
}

#[no_mangle]
pub extern "C" fn qi_runtime_float_to_int(value: f64) -> i64 {
    value as i64
}

// ── 分配 / GC（GC 钩子全部空转：qi 靠 ARC）──────────────────────────

#[no_mangle]
// 参数按 codegen 的声明用 i64（不是 usize）：wasm32 上 usize 是 i32，跟 codegen
// 发的 i64 实参对不上，wasm-ld 会插一个 unreachable 桩，一调用就 trap。
pub extern "C" fn qi_runtime_alloc(size: i64) -> *mut u8 {
    if size <= 0 {
        return std::ptr::null_mut();
    }
    let layout = std::alloc::Layout::from_size_align(size as usize, 8).unwrap();
    unsafe { std::alloc::alloc_zeroed(layout) }
}

#[no_mangle]
pub extern "C" fn qi_runtime_dealloc(ptr: *mut u8, size: i64) -> c_int {
    if ptr.is_null() || size <= 0 {
        return -1;
    }
    let layout = std::alloc::Layout::from_size_align(size as usize, 8).unwrap();
    unsafe { std::alloc::dealloc(ptr, layout) };
    0
}

#[no_mangle]
pub extern "C" fn qi_runtime_gc_should_collect() -> i64 {
    0
}

#[no_mangle]
pub extern "C" fn qi_runtime_gc_collect() {}

#[no_mangle]
pub extern "C" fn qi_runtime_gc_add_root(_ptr: *mut u8) -> i64 {
    0
}

#[no_mangle]
pub extern "C" fn qi_runtime_gc_remove_root(_ptr: *mut u8) -> i64 {
    0
}

// ── 等待组 / 互斥（单线程：goroutine 就地跑完才返回，所以计数永远归零）──

pub struct QiWaitGroup {
    counter: i64,
}

pub struct QiMutex {
    locked: bool,
}

#[no_mangle]
pub extern "C" fn qi_runtime_waitgroup_create() -> *mut QiWaitGroup {
    Box::into_raw(Box::new(QiWaitGroup { counter: 0 }))
}

#[no_mangle]
pub extern "C" fn qi_runtime_waitgroup_add(wg: *mut QiWaitGroup, delta: i32) -> i32 {
    if wg.is_null() {
        return -1;
    }
    unsafe { (*wg).counter += delta as i64 };
    0
}

#[no_mangle]
pub extern "C" fn qi_runtime_waitgroup_done(wg: *mut QiWaitGroup) -> i32 {
    if wg.is_null() {
        return -1;
    }
    unsafe { (*wg).counter -= 1 };
    0
}

#[no_mangle]
pub extern "C" fn qi_runtime_waitgroup_wait(wg: *mut QiWaitGroup) -> i32 {
    if wg.is_null() {
        return -1;
    }
    // 单线程里不可能有人在别处 done：计数若还大于 0，等也等不到，直接返回
    0
}

#[no_mangle]
pub extern "C" fn qi_runtime_mutex_create() -> *mut QiMutex {
    Box::into_raw(Box::new(QiMutex { locked: false }))
}

#[no_mangle]
pub extern "C" fn qi_runtime_mutex_lock(m: *mut QiMutex) -> i32 {
    if m.is_null() {
        return -1;
    }
    unsafe { (*m).locked = true };
    0
}

#[no_mangle]
pub extern "C" fn qi_runtime_mutex_trylock(m: *mut QiMutex) -> i32 {
    if m.is_null() {
        return -1;
    }
    unsafe {
        if (*m).locked {
            1
        } else {
            (*m).locked = true;
            0
        }
    }
}

#[no_mangle]
pub extern "C" fn qi_runtime_mutex_unlock(m: *mut QiMutex) -> i32 {
    if m.is_null() {
        return -1;
    }
    unsafe { (*m).locked = false };
    0
}

// ── 文件（wasi 有文件系统：wasmtime 用 --dir 授权目录，浏览器 shim 里没有）────
//
// 主运行时这一组在 io/io_ffi.rs，经过一层「文件模块」抽象；这里直接落 std::fs，
// 返回约定照抄：读失败给空串不给 null，写/删/改名 成功 0 失败 -1，存在性 1/0。

unsafe fn path_of(p: *const c_char) -> Option<String> {
    if p.is_null() {
        None
    } else {
        Some(cstr_lossy(p).into_owned())
    }
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_read_file(path: *const c_char) -> *mut c_char {
    let empty = || crate::stdlib::qi_str::rc_cstr_from_str("");
    let Some(p) = path_of(path) else {
        return empty();
    };
    match std::fs::read(&p) {
        Ok(bytes) => {
            crate::stdlib::qi_str::rc_cstr_from_string(String::from_utf8_lossy(&bytes).into_owned())
        }
        Err(_) => empty(),
    }
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_write_file(path: *const c_char, content: *const c_char) -> i64 {
    let (Some(p), false) = (path_of(path), content.is_null()) else {
        return -1;
    };
    let data = CStr::from_ptr(content).to_bytes();
    if std::fs::write(&p, data).is_ok() {
        0
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_append_file(path: *const c_char, content: *const c_char) -> i64 {
    let (Some(p), false) = (path_of(path), content.is_null()) else {
        return -1;
    };
    let data = CStr::from_ptr(content).to_bytes();
    let f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&p);
    match f {
        Ok(mut f) => {
            if f.write_all(data).is_ok() {
                0
            } else {
                -1
            }
        }
        Err(_) => -1,
    }
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_delete_file(path: *const c_char) -> i64 {
    let Some(p) = path_of(path) else { return -1 };
    if std::fs::remove_file(&p).is_ok() {
        0
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_file_exists(path: *const c_char) -> i64 {
    let Some(p) = path_of(path) else { return 0 };
    std::path::Path::new(&p).exists() as i64
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_file_size(path: *const c_char) -> i64 {
    let Some(p) = path_of(path) else { return -1 };
    std::fs::metadata(&p).map(|m| m.len() as i64).unwrap_or(-1)
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_rename(from: *const c_char, to: *const c_char) -> i64 {
    let (Some(a), Some(b)) = (path_of(from), path_of(to)) else {
        return -1;
    };
    if std::fs::rename(&a, &b).is_ok() {
        0
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_create_file(path: *const c_char) -> i64 {
    let Some(p) = path_of(path) else { return -1 };
    if std::fs::File::create(&p).is_ok() {
        0
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_create_dir(path: *const c_char) -> i64 {
    let Some(p) = path_of(path) else { return -1 };
    if std::fs::create_dir_all(&p).is_ok() {
        0
    } else {
        -1
    }
}

#[no_mangle]
pub unsafe extern "C" fn qi_io_delete_dir(path: *const c_char) -> i64 {
    let Some(p) = path_of(path) else { return -1 };
    if std::fs::remove_dir_all(&p).is_ok() {
        0
    } else {
        -1
    }
}
