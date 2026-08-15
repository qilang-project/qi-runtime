//! Qi 语言 WASM 最小运行时（spike）
//!
//! 只实现「一个纯计算的 qi 程序」链接所必需的那几个 FFI 符号。目标三元组
//! `wasm32-wasip1`，用 Rust std（打印经 std::io::stdout 落到 wasi `fd_write`），
//! 无任何第三方依赖 —— 这是它能编到 wasm 而完整 qi-runtime 编不动的全部原因。
//!
//! # 为什么不直接裁 qi-runtime
//!
//! qi-runtime 的**非可选**依赖里有两棵原生 C 库树：`rusqlite`(bundled
//! libsqlite3-sys) 和 `reqwest`→`rustls`→`aws-lc-sys`。`cargo build
//! --target wasm32-wasip1 --no-default-features` 在这两个 build script 上
//! 直接失败，跟 feature 开关无关（它们压根不在 feature 后面）。要走那条路
//! 得先把 Cargo.toml 里十来个依赖改成 optional、再给 55 个 stdlib FFI 文件
//! 加 `#[cfg(feature)]`。spike 阶段绕开。
//!
//! # ABI 必须与 qi-runtime 逐字节一致
//!
//! RC 字符串 / RC 对象的隐藏 header 位于 `data - 24`，布局：
//!
//! ```text
//! +-------------+------------------+----------------+
//! | magic (u64) | refcount (i64)   | capacity (i64) |
//! +-------------+------------------+----------------+ ← data（FFI 里传的指针）
//! | data bytes (capacity 字节, UTF-8) + 尾部 NUL      |
//! +--------------------------------------------------+
//! ```
//!
//! 三个字段都是定宽整数，所以在 wasm32（4 字节指针）上 header 仍是 24 字节，
//! 与 x86_64/aarch64 同形 —— codegen 发射的字面量全局常量无需改动即可复用。
//! `refcount >= IMMORTAL_RC` 表示不朽（codegen 把字面量 emit 成这种），增减皆 no-op。
//!
//! 铁律沿用主运行时：**magic 不符时宁泄漏，绝不崩溃**。

use std::alloc::{alloc, dealloc, Layout};
use std::ffi::CStr;
use std::io::Write;
use std::os::raw::{c_char, c_int};
use std::sync::atomic::{AtomicI64, Ordering};

/// RC 字符串 buffer 识别 magic（与 qi_str::QI_STR_MAGIC 一致）
const QI_STR_MAGIC: u64 = 0x5149_5352_4331_0001;
/// RC 对象 buffer 识别 magic（与 rc_obj::QI_OBJ_MAGIC 一致）
const QI_OBJ_MAGIC: u64 = 0x5149_4F42_4A43_0001;
/// refcount >= 此值 ⇒ 不朽：增减皆 no-op，永不释放
const IMMORTAL_RC: i64 = 1 << 61;

const HEADER_SIZE: usize = 24;

#[repr(C)]
struct BufHeader {
    magic: u64,
    refcount: AtomicI64,
    /// 字符串是 capacity，对象是 size；两者都指「data 区字节数，不含 header」
    capacity: i64,
}

// 布局铁闸：跨语言 ABI 的前提，编译期就要拍死
const _: () = assert!(std::mem::size_of::<BufHeader>() == HEADER_SIZE);

#[inline]
unsafe fn header_of(data: *const u8) -> *mut BufHeader {
    data.sub(HEADER_SIZE) as *mut BufHeader
}

/// 多分配 1 字节放尾部 NUL，使 data 指针本身就是合法 C 字符串
fn buffer_layout(cap: usize) -> Layout {
    Layout::from_size_align(HEADER_SIZE + cap + 1, 8).expect("invalid buffer layout")
}

fn obj_layout(size: usize) -> Layout {
    Layout::from_size_align(HEADER_SIZE + size, 8).expect("invalid obj layout")
}

/// 静态不朽空串 —— 空输入不进堆，retain/release 全 no-op
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

/// 从字节切片分配一条 RC C 字符串，返回 data 指针（`ptr-24` 处是 header，rc=1）
fn rc_cstr_from_bytes(data: &[u8]) -> *mut c_char {
    if data.is_empty() {
        return RC_CSTR_EMPTY.data.as_ptr() as *mut c_char;
    }
    let layout = buffer_layout(data.len());
    unsafe {
        let raw = alloc(layout);
        if raw.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        (raw as *mut BufHeader).write(BufHeader {
            magic: QI_STR_MAGIC,
            refcount: AtomicI64::new(1),
            capacity: data.len() as i64,
        });
        let data_ptr = raw.add(HEADER_SIZE);
        std::ptr::copy_nonoverlapping(data.as_ptr(), data_ptr, data.len());
        *data_ptr.add(data.len()) = 0;
        data_ptr as *mut c_char
    }
}

#[inline]
fn rc_cstr_from_string(s: String) -> *mut c_char {
    rc_cstr_from_bytes(s.as_bytes())
}

/// 把 FFI 传进来的裸 C 串读成 &str（非法 UTF-8 → None）
///
/// # Safety
/// `p` 必须非空且指向 NUL 结尾的合法内存。
unsafe fn cstr(p: *const c_char) -> Option<&'static str> {
    CStr::from_ptr(p).to_str().ok()
}

// ============================================================================
// 引用计数核心
// ============================================================================

/// # Safety
/// `base` 必须非空，且 `base-24` 可读。
unsafe fn retain_base(base: *const u8) {
    let h = header_of(base);
    let magic = (*h).magic;
    // 字符串与对象 header 同形，这里两种 magic 都按同一套增引用处理
    if magic != QI_STR_MAGIC && magic != QI_OBJ_MAGIC {
        return; // 宁泄漏不崩溃
    }
    if (*h).refcount.load(Ordering::Relaxed) >= IMMORTAL_RC {
        return;
    }
    (*h).refcount.fetch_add(1, Ordering::Relaxed);
}

/// 减引用，归零时按 magic 对应的 layout 释放整个 buffer（含 header）。
///
/// 注意：对象走的是**浅释放** —— 只回收本体，字段里的 RC 指针泄漏。
/// 与主运行时 `obj_release_shallow` 的行为一致（宁泄漏不崩）。
///
/// # Safety
/// 同 [`retain_base`]。
unsafe fn release_base(base: *const u8) {
    let h = header_of(base);
    let magic = (*h).magic;
    if magic != QI_STR_MAGIC && magic != QI_OBJ_MAGIC {
        return;
    }
    if (*h).refcount.load(Ordering::Relaxed) >= IMMORTAL_RC {
        return;
    }
    if (*h).refcount.fetch_sub(1, Ordering::AcqRel) == 1 {
        let n = (*h).capacity as usize;
        let layout = if magic == QI_STR_MAGIC {
            buffer_layout(n)
        } else {
            obj_layout(n)
        };
        dealloc(h as *mut u8, layout);
    }
}

// ============================================================================
// 打印
// ============================================================================

/// 打印一行 UTF-8 字符串。wasip1 下 stdout 走 `fd_write`。
///
/// 每行都显式 flush：wasm 里没有进程退出时的 atexit 冲刷保证，
/// 不 flush 就会出现「程序跑完了但一个字没有」。
///
/// # Safety
/// `s` 为 NUL 结尾的 C 字符串或 null。
#[no_mangle]
pub unsafe extern "C" fn qi_runtime_println(s: *const c_char) -> c_int {
    if s.is_null() {
        return -1;
    }
    match cstr(s) {
        Some(text) => {
            let out = std::io::stdout();
            let mut lock = out.lock();
            let _ = lock.write_all(text.as_bytes());
            let _ = lock.write_all(b"\n");
            let _ = lock.flush();
            0
        }
        None => -1,
    }
}

/// 打印一行整数
#[no_mangle]
pub extern "C" fn qi_runtime_println_int(value: i64) -> c_int {
    let out = std::io::stdout();
    let mut lock = out.lock();
    let _ = writeln!(lock, "{}", value);
    let _ = lock.flush();
    0
}

/// 打印一行浮点数
#[no_mangle]
pub extern "C" fn qi_runtime_println_float(value: f64) -> c_int {
    let out = std::io::stdout();
    let mut lock = out.lock();
    let _ = writeln!(lock, "{}", value);
    let _ = lock.flush();
    0
}

// ============================================================================
// 字符串 / 数值转换
// ============================================================================

/// 拼接两条字符串，返回新的 RC 串（调用方负责 release）
///
/// # Safety
/// 两个参数为 NUL 结尾的 C 字符串或 null。
#[no_mangle]
pub unsafe extern "C" fn qi_runtime_string_concat(
    s1: *const c_char,
    s2: *const c_char,
) -> *mut c_char {
    if s1.is_null() || s2.is_null() {
        return std::ptr::null_mut();
    }
    match (cstr(s1), cstr(s2)) {
        (Some(a), Some(b)) => rc_cstr_from_string(format!("{}{}", a, b)),
        _ => std::ptr::null_mut(),
    }
}

/// 整数 → RC 字符串
#[no_mangle]
pub extern "C" fn qi_runtime_int_to_string(value: i64) -> *mut c_char {
    rc_cstr_from_string(value.to_string())
}

/// 浮点数 → RC 字符串
#[no_mangle]
pub extern "C" fn qi_runtime_float_to_string(value: f64) -> *mut c_char {
    rc_cstr_from_string(value.to_string())
}

/// 字符串 → 整数（解析失败返回 0，与主运行时一致）
///
/// # Safety
/// `s` 为 NUL 结尾的 C 字符串或 null。
#[no_mangle]
pub unsafe extern "C" fn qi_runtime_string_to_int(s: *const c_char) -> i64 {
    if s.is_null() {
        return 0;
    }
    cstr(s)
        .and_then(|t| t.trim().parse::<i64>().ok())
        .unwrap_or(0)
}

/// 字符串字节长度
///
/// # Safety
/// `s` 为 NUL 结尾的 C 字符串或 null。
#[no_mangle]
pub unsafe extern "C" fn qi_str_byte_length_cstr(s: *const c_char) -> i64 {
    if s.is_null() {
        return 0;
    }
    CStr::from_ptr(s).to_bytes().len() as i64
}

// ============================================================================
// RC 字符串 C ABI 入口（codegen 直接调用这几个名字）
// ============================================================================

/// 增引用：null / magic 不符 / 不朽 皆 no-op
///
/// # Safety
/// `s` 要么是 null，要么是本分配器（或 codegen 字面量）产出的 data 指针。
#[no_mangle]
pub unsafe extern "C" fn qi_string_retain(s: *const c_char) {
    if s.is_null() {
        return;
    }
    retain_base(s as *const u8);
}

/// 减引用，归零释放
///
/// # Safety
/// 同 [`qi_string_retain`]。
#[no_mangle]
pub unsafe extern "C" fn qi_string_free(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    release_base(s as *const u8);
}

// ============================================================================
// RC 对象 C ABI 入口
// ============================================================================

/// 减引用（**不释放**），返回旧值。codegen 的每类型释放函数据旧值 ==1 决定
/// 是否走「释放字段 + qi_obj_free」路径。
///
/// # Safety
/// `p` 要么是 null，要么是带 24 字节 header 的 RC data 指针。
#[no_mangle]
pub unsafe extern "C" fn qi_obj_dec(p: *const u8) -> i64 {
    if p.is_null() {
        return 0;
    }
    let h = header_of(p);
    match (*h).magic {
        QI_OBJ_MAGIC => {
            let cur = (*h).refcount.load(Ordering::Relaxed);
            if cur >= IMMORTAL_RC {
                return cur; // ≠1：调用方不会释放
            }
            (*h).refcount.fetch_sub(1, Ordering::AcqRel)
        }
        // 字符串走完整 release，返回 0 让调用方不要再走对象释放路径
        QI_STR_MAGIC => {
            release_base(p);
            0
        }
        _ => 0,
    }
}

/// 按 header 里的 size 释放对象本体。只应在 `qi_obj_dec` 返回 1 后调用。
///
/// # Safety
/// 同 [`qi_obj_dec`]，且此刻引用计数已归零。
#[no_mangle]
pub unsafe extern "C" fn qi_obj_free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    let h = header_of(p);
    if (*h).magic != QI_OBJ_MAGIC {
        return; // 宁泄漏不崩溃
    }
    let size = (*h).capacity as usize;
    dealloc(h as *mut u8, obj_layout(size));
}

/// 动态派发释放 —— 数组元素等「编译期不知具体 RC 类型」的保守入口。
/// null / magic 不符 静默 no-op（此处混入非 RC 指针属预期，不告警）。
///
/// # Safety
/// `p` 可以是任意指针；magic 不符时安全返回。
#[no_mangle]
pub unsafe extern "C" fn qi_rc_release_any(p: *const u8) {
    if p.is_null() {
        return;
    }
    let magic = (*header_of(p)).magic;
    if magic == QI_STR_MAGIC || magic == QI_OBJ_MAGIC {
        release_base(p);
    }
}

// ============================================================================
// wasi 启动垫片
// ============================================================================

// qi 的 codegen 把入口发射成 `int main(void)`。LLVM 的 WebAssembly 后端见到
// 无参 main 时会把函数体改名成 `__original_main`，再留一个 `() -> i32` 的
// `main` 包装。而 wasi-libc 的 crt1-command.o 调的是 `__main_void`，它内部
// 按 `main(int, char**)`（`(i32,i32) -> i32`）去取符号 —— 签名对不上，
// wasm-ld 于是把 main 解析成 undefined weak 存根，一进去就 `unreachable` 陷阱。
//
// 这里在**运行时归档里**自己给出 `__main_void`。归档在 `-lc` 之前被扫到，
// 所以 libc 里那份永远不会被拉进来，签名冲突自然消失。
// 命令行参数在 wasm playground 场景用不上，直接忽略。
extern "C" {
    fn __original_main() -> c_int;
}

/// wasi command 模块的真正入口（crt1-command.o 的 `_start` 调这个）
///
/// # Safety
/// 由 wasi 启动代码调用一次，转调 qi 生成的入口函数。
#[no_mangle]
pub unsafe extern "C" fn __main_void() -> c_int {
    __original_main()
}

// ============================================================================
// 反射注册 —— wasm 上不做反射，收下就丢
// ============================================================================

/// codegen 会为每个函数发一条注册调用。最小运行时不保存反射表，空实现即可。
#[no_mangle]
pub extern "C" fn qi_reflect_register_function(_name: *const c_char, _signature: *const c_char) {}
