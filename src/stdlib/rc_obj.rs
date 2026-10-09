//! RC 对象分配器 —— 结构体本体 / 数组本体（QI_ARC=1 codegen 专用）。
//!
//! 与 qi_str.rs 的 RC 字符串同族：`ptr-24` 处藏 header，i8* ABI 不变。
//! header 布局（与 BufHeader 完全同形，24 字节，8 对齐）：
//! ```text
//! +-------------------+-------------------+----------------+
//! | magic (u64)       | refcount (i64)    | size (i64)     |
//! +-------------------+-------------------+----------------+ ← data（返回给 codegen 的指针）
//! | data bytes (size 字节，零初始化)                          |
//! +--------------------------------------------------------+
//! ```
//!
//! 设计要点：
//! - **绕开 memory_manager**：直接 std::alloc（顺带甩掉热路径全局 RwLock 读锁）。
//! - **零初始化**：结构体/数组的 RC 字段（字符串/嵌套结构体指针槽）天然是
//!   null，释放函数 load 到 null 时安全 no-op。
//! - **跨 magic 互认**：字符串 header（QI_STR_MAGIC）与对象 header
//!   （QI_OBJ_MAGIC）布局同形。类型混淆时（如 数组<指针> 元素既可能是串
//!   也可能是结构体）互相委托：retain 委托对方 retain；release 委托对方的
//!   **浅释放**（对象侧只释放本体，字段泄漏 —— 宁泄漏不崩）。
//! - **magic 全不符** → 一次性警告后 no-op（铁律：宁泄漏，绝不崩溃）。
//!   `qi_rc_release_any`（数组元素动态释放入口）不警告 —— 它的调用点
//!   本来就是"元素类型未知"的保守路径，遇到闭包 fat obj / 外部句柄属预期。
//!
//! QI_ARC=0 时 codegen 仍走 qi_runtime_alloc，本模块所有入口无人调用，
//! 两个分配器互不相见，无双释放可能。
//!
//! ## 已知限制：循环引用不回收
//!
//! Qi 的内存管理是**纯引用计数**（与 Swift/Objective-C ARC 同族），**不带
//! 循环收集器**：两个对象互相强持有（A.字段=B 且 B.字段=A，或闭包捕获了
//! 持有该闭包的结构体）时引用计数永不归零，整环泄漏。建议数据结构避免
//! 强循环（父子结构中让一侧只存整数 id / 句柄而非对象指针）。可用
//! `QI_RC_REPORT=1` 观测进程退出时的活跃对象/字符串/闭包计数来发现此类泄漏。

#![allow(non_snake_case)]

use std::alloc::{alloc_zeroed, dealloc, Layout};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

use super::closure_ffi::QI_CLO_MAGIC;
use super::qi_str::{IMMORTAL_RC, QI_STR_MAGIC};

// ─────────────────── 泄漏诊断计数器（QI_RC_REPORT=1 时 atexit 报告） ───────────────────
//
// 三类 RC 分配的活跃计数（alloc++ / 真实 dealloc--）。计数常开（一次 relaxed
// 原子加减，热路径可忽略）；打印默认关，进程退出时 QI_RC_REPORT=1 才输出一行。

static LIVE_OBJS: AtomicI64 = AtomicI64::new(0);
static LIVE_STRS: AtomicI64 = AtomicI64::new(0);
static LIVE_CLOS: AtomicI64 = AtomicI64::new(0);

extern "C" fn rc_report_at_exit() {
    eprintln!(
        "[qi-rc] 活跃对象={} 活跃字符串={} 活跃闭包={}",
        LIVE_OBJS.load(Ordering::Relaxed),
        LIVE_STRS.load(Ordering::Relaxed),
        LIVE_CLOS.load(Ordering::Relaxed),
    );
}

/// 首次分配时注册 atexit 报告（仅当 QI_RC_REPORT=1）。
fn maybe_register_report() {
    use std::sync::OnceLock;
    static REGISTERED: OnceLock<()> = OnceLock::new();
    REGISTERED.get_or_init(|| {
        if std::env::var("QI_RC_REPORT")
            .map(|v| v == "1")
            .unwrap_or(false)
        {
            unsafe {
                libc::atexit(rc_report_at_exit);
            }
        }
    });
}

#[inline]
pub(crate) fn diag_obj_alloc() {
    maybe_register_report();
    LIVE_OBJS.fetch_add(1, Ordering::Relaxed);
}
#[inline]
pub(crate) fn diag_obj_free() {
    LIVE_OBJS.fetch_sub(1, Ordering::Relaxed);
}
#[inline]
pub(crate) fn diag_str_alloc() {
    maybe_register_report();
    LIVE_STRS.fetch_add(1, Ordering::Relaxed);
}
#[inline]
pub(crate) fn diag_str_free() {
    LIVE_STRS.fetch_sub(1, Ordering::Relaxed);
}
#[inline]
pub(crate) fn diag_clo_alloc() {
    maybe_register_report();
    LIVE_CLOS.fetch_add(1, Ordering::Relaxed);
}
#[inline]
pub(crate) fn diag_clo_free() {
    LIVE_CLOS.fetch_sub(1, Ordering::Relaxed);
}

/// 测试 / 诊断用：读当前活跃计数 (对象, 字符串, 闭包)。
pub fn rc_live_counts() -> (i64, i64, i64) {
    (
        LIVE_OBJS.load(Ordering::Relaxed),
        LIVE_STRS.load(Ordering::Relaxed),
        LIVE_CLOS.load(Ordering::Relaxed),
    )
}

/// RC 对象 buffer 识别 magic —— "QIOBJC1" 变体 + 版本号（≠ QI_STR_MAGIC）。
pub const QI_OBJ_MAGIC: u64 = 0x5149_4F42_4A43_0001;

/// 对象 header（位于 data-24 处；与 qi_str::BufHeader 同形）。
#[repr(C)]
struct ObjHeader {
    magic: u64,
    refcount: AtomicI64,
    /// data 区字节数（不含 header）。
    size: i64,
}

const HEADER_SIZE: usize = std::mem::size_of::<ObjHeader>(); // 24

// 布局铁闸：header 必须正好 24 字节（与字符串 header 同形，跨 magic 委托的前提）
const _: () = assert!(std::mem::size_of::<ObjHeader>() == 24);

#[inline]
unsafe fn header_of(data: *const u8) -> *mut ObjHeader {
    data.sub(HEADER_SIZE) as *mut ObjHeader
}

fn obj_layout(size: usize) -> Layout {
    Layout::from_size_align(HEADER_SIZE + size, 8).expect("invalid obj layout")
}

/// 进程级一次性防御日志（对象侧独立于字符串侧的开关）。
static NON_RC_OBJ_WARNED: AtomicBool = AtomicBool::new(false);

#[cold]
fn warn_non_rc_obj_once(who: &str) {
    if !NON_RC_OBJ_WARNED.swap(true, Ordering::Relaxed) {
        eprintln!(
            "{}: 非 RC 对象指针,已忽略(后续同类情况将静默泄漏,不再重复警告)",
            who
        );
    }
}

/// 分配一个 RC 对象（refcount=1，data 区零初始化），返回 data 指针。
/// size <= 0 时按 8 字节分配（空结构体也要一个合法可释放的本体）。
#[no_mangle]
pub extern "C" fn qi_obj_alloc(size: i64) -> *mut u8 {
    let size = if size <= 0 { 8 } else { size as usize };
    let layout = obj_layout(size);
    unsafe {
        let raw = alloc_zeroed(layout);
        if raw.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        // 写 header（alloc_zeroed 已清零，写 magic/refcount/size 即可）
        let h = raw as *mut ObjHeader;
        (*h).magic = QI_OBJ_MAGIC;
        (*h).refcount = AtomicI64::new(1);
        (*h).size = size as i64;
        diag_obj_alloc();
        raw.add(HEADER_SIZE)
    }
}

/// 对象浅 retain（供 qi_str 跨 magic 委托；调用方已验过 OBJ magic）。
///
/// # Safety
/// data-24 起 24 字节必须是合法 ObjHeader。
pub(crate) unsafe fn obj_retain_raw(data: *const u8) {
    let h = header_of(data);
    if (*h).refcount.load(Ordering::Relaxed) >= IMMORTAL_RC {
        return;
    }
    (*h).refcount.fetch_add(1, Ordering::Relaxed);
}

/// 对象浅 release（供 qi_str 跨 magic 委托 + qi_rc_release_any）：
/// refcount--，归零时**只释放本体**（不知道字段布局，字段泄漏 —— 宁泄漏不崩）。
///
/// # Safety
/// 同 [`obj_retain_raw`]。
pub(crate) unsafe fn obj_release_shallow(data: *const u8) {
    let h = header_of(data);
    if (*h).refcount.load(Ordering::Relaxed) >= IMMORTAL_RC {
        return;
    }
    let prev = (*h).refcount.fetch_sub(1, Ordering::AcqRel);
    if prev == 1 {
        let size = (*h).size as usize;
        dealloc(h as *mut u8, obj_layout(size));
        diag_obj_free();
    }
}

/// 判别一个指针是否为 RC 字符串（STR magic）。null / OBJ / CLO / 其它 → 0。
///
/// 供 codegen 的「延迟解码 future」使用：`异步询问::<T>` 返回的 未来<T> 物理上
/// 装的是**原始回复字符串**（在飞 HTTP），而普通结构体 future 装的是结构体指针。
/// `等待` 在 IR 层用本函数区分两者：STR → JSON 解码建 T；OBJ → 直接就是 T。
/// 前提与 qi_obj_retain 相同：所有 RC 分配的 header 在 base-24 且可读。
#[no_mangle]
pub extern "C" fn qi_rc_is_string(p: *const u8) -> i64 {
    if p.is_null() {
        return 0;
    }
    unsafe {
        let magic = *(p.sub(HEADER_SIZE) as *const u64);
        i64::from(magic == QI_STR_MAGIC)
    }
}

/// 增引用：null no-op；OBJ magic → refcount++；STR magic → 委托字符串 retain；
/// CLO magic → 委托闭包 retain；其余 → 一次性警告后 no-op。
#[no_mangle]
pub extern "C" fn qi_obj_retain(p: *const u8) {
    if p.is_null() {
        return;
    }
    unsafe {
        let magic = *(p.sub(HEADER_SIZE) as *const u64);
        if magic == QI_OBJ_MAGIC {
            obj_retain_raw(p);
        } else if magic == QI_STR_MAGIC {
            super::qi_str::qi_string_retain(p as *const std::os::raw::c_char);
        } else if magic == QI_CLO_MAGIC {
            super::closure_ffi::clo_retain_raw(p);
        } else {
            warn_non_rc_obj_once("qi_obj_retain");
        }
    }
}

/// 减引用（不释放！），返回**旧值**。codegen 的每类型释放函数据此决定是否
/// 走"释放字段 + qi_obj_free"路径（旧值==1 时）。
///
/// - null → 0
/// - OBJ magic → fetch_sub 返回旧值；immortal → 不减，返回当前值（≠1，永不释放）
/// - STR magic → 委托字符串**完整** release，返回 0（调用方不得再走对象释放路径）
/// - CLO magic → 委托闭包**完整** release（归零调 dtor + free），返回 0（同上）
/// - 其余 → 一次性警告后返回 0（泄漏）
#[no_mangle]
pub extern "C" fn qi_obj_dec(p: *const u8) -> i64 {
    if p.is_null() {
        return 0;
    }
    unsafe {
        let magic = *(p.sub(HEADER_SIZE) as *const u64);
        if magic == QI_OBJ_MAGIC {
            let h = header_of(p);
            let cur = (*h).refcount.load(Ordering::Relaxed);
            if cur >= IMMORTAL_RC {
                return cur; // ≠1：调用方不会释放
            }
            (*h).refcount.fetch_sub(1, Ordering::AcqRel)
        } else if magic == QI_STR_MAGIC {
            super::qi_str::rc_cstr_release(p as *mut std::os::raw::c_char);
            0
        } else if magic == QI_CLO_MAGIC {
            super::closure_ffi::clo_release_raw(p);
            0
        } else {
            warn_non_rc_obj_once("qi_obj_dec");
            0
        }
    }
}

/// 按 header size 释放对象本体。只应在 qi_obj_dec 返回 1 后调用
/// （codegen 释放函数保证）。null / magic 不符 → 警告后 no-op。
#[no_mangle]
pub extern "C" fn qi_obj_free(p: *mut u8) {
    if p.is_null() {
        return;
    }
    unsafe {
        let magic = *(p.sub(HEADER_SIZE) as *const u64);
        if magic != QI_OBJ_MAGIC {
            warn_non_rc_obj_once("qi_obj_free");
            return;
        }
        let h = header_of(p);
        let size = (*h).size as usize;
        dealloc(h as *mut u8, obj_layout(size));
        diag_obj_free();
    }
}

/// 动态派发释放 —— 数组<指针> 元素等"编译期不知具体 RC 类型"的保守释放入口：
/// - STR magic → 字符串完整 release
/// - OBJ magic → 对象浅 release（归零只释放本体，字段泄漏）
/// - CLO magic → 闭包**完整** release（dtor 藏在对象末槽，可安全级联释放捕获）
/// - null / 其它（外部句柄…）→ **静默** no-op（此入口调用点
///   本就是元素类型未知的保守路径，混入非 RC 指针属预期，不告警）
#[no_mangle]
pub extern "C" fn qi_rc_release_any(p: *const u8) {
    if p.is_null() {
        return;
    }
    unsafe {
        let magic = *(p.sub(HEADER_SIZE) as *const u64);
        if magic == QI_OBJ_MAGIC {
            obj_release_shallow(p);
        } else if magic == QI_STR_MAGIC {
            super::qi_str::rc_cstr_release(p as *mut std::os::raw::c_char);
        } else if magic == QI_CLO_MAGIC {
            super::closure_ffi::clo_release_raw(p);
        }
        // 其余：静默泄漏
    }
}

// ─────────────────── 可增长数组：添加 / 弹出 的存储准备 ───────────────────
//
// 数组本体布局不变：`[len, e0, e1, …]`，每槽 8 字节，对象 header 的 size 是
// **容量**（字节）。`数组<T>()` 只分配一个长度槽，`添加` 时按需翻倍扩容。
//
// 由 codegen 在 `甲.添加(x)` / `甲.弹出()` 处调用，返回值**写回接收者那个
// 变量 / 字段**（扩容可能搬家）。元素的读写、ARC 的 retain/release 仍在
// codegen 里做 —— 这里只管「这块存储能不能原地改」。

/// 新容量（槽数，不含长度槽）：至少放得下 need 个元素，按 2 倍增长，起步 4。
fn grown_slots(cur_slots: usize, need: usize) -> usize {
    let mut cap = cur_slots.max(4);
    while cap < need {
        cap *= 2;
    }
    cap
}

/// 让 `arr` 变成**可以原地修改**、且至少还能再放 `extra` 个元素的数组，
/// 返回（可能是新的）数组指针。调用方必须把返回值写回原来存放 arr 的地方。
///
/// - `arc == 0`（QI_ARC=0 的泄漏模式）：数组本体可能不是 RC 对象，header
///   不可信 —— 每次都拷一份新的（旧的照泄漏模式的规矩不回收）。
/// - 唯一持有（refcount == 1）：容量够就原样返回；不够就 realloc 原地扩容
///   （元素槽整体搬家，所有权随槽转移，不需要 retain/release）。
/// - 被共享（refcount > 1，含 immortal）或不是 RC 对象：**写时复制** ——
///   拷出一份新数组，RC 元素（`elem_rc != 0`）逐个 retain，旧的减一次引用。
///   于是 `乙 = 甲; 甲.添加(x);` 之后 乙 还是原来的长度，不会跟着变，也不会
///   被搬家搬成野指针。函数参数在序言里被 retain 过，所以对参数 添加 也走这条。
#[no_mangle]
pub extern "C" fn qi_array_prepare_mut(
    arr: *mut u8,
    extra: i64,
    elem_rc: i64,
    arc: i64,
) -> *mut u8 {
    let extra = extra.max(0) as usize;
    let len = if arr.is_null() {
        0
    } else {
        unsafe { (*(arr as *const i64)).max(0) as usize }
    };
    let need = len + extra;

    let unique = arc != 0
        && !arr.is_null()
        && unsafe {
            let h = header_of(arr);
            (*h).magic == QI_OBJ_MAGIC && (*h).refcount.load(Ordering::Acquire) == 1
        };

    if unique {
        unsafe {
            let h = header_of(arr);
            let size = (*h).size as usize;
            let need_bytes = (need + 1) * 8;
            if size >= need_bytes {
                return arr;
            }
            let new_slots = grown_slots(size / 8 - 1, need);
            let new_bytes = (new_slots + 1) * 8;
            let raw = std::alloc::realloc(h as *mut u8, obj_layout(size), HEADER_SIZE + new_bytes);
            if raw.is_null() {
                std::alloc::handle_alloc_error(obj_layout(new_bytes));
            }
            // 新增的尾部清零：RC 元素槽必须是 null 才能安全释放
            std::ptr::write_bytes(raw.add(HEADER_SIZE + size), 0, new_bytes - size);
            let h = raw as *mut ObjHeader;
            (*h).size = new_bytes as i64;
            return raw.add(HEADER_SIZE);
        }
    }

    // 写时复制 / 泄漏模式：新开一块，拷长度和元素
    let new_slots = if extra == 0 {
        need.max(1)
    } else {
        grown_slots(len, need)
    };
    let fresh = qi_obj_alloc(((new_slots + 1) * 8) as i64);
    if !arr.is_null() {
        unsafe {
            std::ptr::copy_nonoverlapping(arr as *const i64, fresh as *mut i64, len + 1);
            if arc != 0 {
                if elem_rc != 0 {
                    for i in 1..=len {
                        let e = *(fresh as *const i64).add(i) as usize as *const u8;
                        qi_obj_retain(e);
                    }
                }
                // 只放掉我们这一份引用；对方还持有，不会归零。非 RC 对象 /
                // immortal 时 qi_obj_dec 本身就是 no-op（或一次性警告）。
                let magic = *(arr.sub(HEADER_SIZE) as *const u64);
                if magic == QI_OBJ_MAGIC {
                    let old = qi_obj_dec(arr);
                    if old == 1 {
                        // 只有并发竞争才会走到：别人在这期间放掉了它。按释放
                        // 函数的约定，元素引用随旧本体一起交回 —— 我们刚才已为
                        // 新数组 retain 过，旧数组那份要还掉。
                        if elem_rc != 0 {
                            for i in 1..=len {
                                let e = *(arr as *const i64).add(i) as usize as *const u8;
                                qi_rc_release_any(e);
                            }
                        }
                        qi_obj_free(arr);
                    }
                }
            }
        }
    } else {
        unsafe {
            *(fresh as *mut i64) = 0;
        }
    }
    fresh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::qi_str::rc_cstr_from_bytes;
    use std::ffi::CStr;

    unsafe fn rc_of(p: *const u8) -> i64 {
        (*header_of(p)).refcount.load(Ordering::Relaxed)
    }

    #[test]
    fn alloc_writes_header_and_zeroes_data() {
        let p = qi_obj_alloc(40);
        assert!(!p.is_null());
        unsafe {
            let h = header_of(p);
            assert_eq!((*h).magic, QI_OBJ_MAGIC);
            assert_eq!((*h).size, 40);
            assert_eq!(rc_of(p), 1);
            for i in 0..40 {
                assert_eq!(*p.add(i), 0, "data 必须零初始化");
            }
        }
        qi_obj_free(p); // rc 仍为 1，直接 free（模拟 dec==1 后的调用）
    }

    #[test]
    fn alloc_zero_size_is_valid() {
        let p = qi_obj_alloc(0);
        assert!(!p.is_null());
        unsafe {
            assert_eq!((*header_of(p)).size, 8);
        }
        assert_eq!(qi_obj_dec(p), 1);
        qi_obj_free(p);
    }

    #[test]
    fn retain_dec_roundtrip() {
        let p = qi_obj_alloc(16);
        qi_obj_retain(p);
        unsafe {
            assert_eq!(rc_of(p), 2);
        }
        assert_eq!(qi_obj_dec(p), 2); // 旧值 2 → 不释放
        assert_eq!(qi_obj_dec(p), 1); // 旧值 1 → 调用方负责 free
        qi_obj_free(p);
    }

    unsafe fn arr_len(p: *const u8) -> i64 {
        *(p as *const i64)
    }

    #[test]
    fn prepare_mut_grows_unique_array_in_place_semantics() {
        // 空数组：只有长度槽
        let mut a = qi_obj_alloc(8);
        for i in 0..100i64 {
            a = qi_array_prepare_mut(a, 1, 0, 1);
            unsafe {
                let n = arr_len(a);
                *(a as *mut i64).add(n as usize + 1) = i * 10;
                *(a as *mut i64) = n + 1;
                assert!((*header_of(a)).size >= (n + 2) * 8);
                assert_eq!(rc_of(a), 1, "扩容不应改变引用计数");
            }
        }
        unsafe {
            assert_eq!(arr_len(a), 100);
            for i in 0..100 {
                assert_eq!(*(a as *const i64).add(i + 1), i as i64 * 10);
            }
        }
        assert_eq!(qi_obj_dec(a), 1);
        qi_obj_free(a);
    }

    #[test]
    fn prepare_mut_copies_shared_array_and_retains_elements() {
        let s = rc_cstr_from_bytes(b"elem");
        let a = qi_obj_alloc(16);
        unsafe {
            *(a as *mut i64) = 1;
            *(a as *mut i64).add(1) = s as usize as i64;
        }
        qi_obj_retain(a); // 两个持有者
        let b = qi_array_prepare_mut(a, 1, 1, 1);
        assert_ne!(a, b, "共享数组必须写时复制");
        unsafe {
            assert_eq!(rc_of(a), 1, "旧数组只放掉我们这一份");
            assert_eq!(rc_of(b), 1);
            assert_eq!(arr_len(b), 1);
            assert_eq!(*(b as *const i64).add(1), s as usize as i64);
            // 字符串 header 与对象 header 同形，rc_of 读得出它的引用计数
            assert_eq!(rc_of(s as *const u8), 2, "元素被两个数组各持有一份");
        }
    }

    #[test]
    fn prepare_mut_unique_with_room_returns_same_pointer() {
        let a = qi_obj_alloc(8 * 5);
        unsafe {
            *(a as *mut i64) = 2;
        }
        assert_eq!(qi_array_prepare_mut(a, 1, 0, 1), a);
        assert_eq!(qi_array_prepare_mut(a, 0, 0, 1), a);
        assert_eq!(qi_obj_dec(a), 1);
        qi_obj_free(a);
    }

    #[test]
    fn null_safety_everywhere() {
        qi_obj_retain(std::ptr::null());
        assert_eq!(qi_obj_dec(std::ptr::null()), 0);
        qi_obj_free(std::ptr::null_mut());
        qi_rc_release_any(std::ptr::null());
    }

    #[test]
    fn foreign_pointer_is_leaked_not_crashed() {
        // 非 RC 分配（无 header）：retain/dec/free 全都 no-op，不崩、不改内容
        let raw = std::ffi::CString::new("foreign-object").unwrap().into_raw();
        qi_obj_retain(raw as *const u8);
        assert_eq!(qi_obj_dec(raw as *const u8), 0);
        qi_obj_free(raw as *mut u8);
        qi_rc_release_any(raw as *const u8);
        unsafe {
            assert_eq!(CStr::from_ptr(raw).to_bytes(), b"foreign-object");
            let _ = std::ffi::CString::from_raw(raw); // 正常归还，避免测试泄漏
        }
    }

    #[test]
    fn obj_retain_delegates_to_string_rc() {
        // 类型混淆：字符串指针误入对象 retain/dec —— 委托字符串 RC，计数平衡
        let s = rc_cstr_from_bytes(b"cross-magic");
        qi_obj_retain(s as *const u8); // 字符串 rc 1 → 2
        assert_eq!(qi_obj_dec(s as *const u8), 0); // 完整 release：2 → 1，返回 0
        unsafe {
            assert_eq!(CStr::from_ptr(s).to_bytes(), b"cross-magic");
        }
        crate::stdlib::qi_str::rc_cstr_release(s); // 1 → 0 释放
    }

    #[test]
    fn string_free_delegates_to_obj_shallow() {
        // 反向混淆：对象指针误入字符串 retain/free —— 委托对象浅 RC
        let p = qi_obj_alloc(24);
        crate::stdlib::qi_str::qi_string_retain(p as *const std::os::raw::c_char); // 1 → 2
        unsafe {
            assert_eq!(rc_of(p), 2);
        }
        crate::stdlib::qi_str::rc_cstr_release(p as *mut std::os::raw::c_char); // 2 → 1
        unsafe {
            assert_eq!(rc_of(p), 1);
        }
        crate::stdlib::qi_str::rc_cstr_release(p as *mut std::os::raw::c_char); // 1 → 0 浅释放本体
    }

    #[test]
    fn release_any_dispatches_by_magic() {
        // 字符串 → 完整释放
        let s = rc_cstr_from_bytes(b"any-str");
        qi_rc_release_any(s as *const u8); // 1 → 0 释放，不崩
                                           // 对象 → 浅释放
        let p = qi_obj_alloc(16);
        qi_obj_retain(p);
        qi_rc_release_any(p as *const u8); // 2 → 1
        unsafe {
            assert_eq!(rc_of(p), 1);
        }
        qi_rc_release_any(p as *const u8); // 1 → 0 释放本体
    }

    #[test]
    fn concurrent_retain_release_is_balanced() {
        let p = qi_obj_alloc(32) as usize;
        let mut handles = Vec::new();
        for _ in 0..8 {
            handles.push(std::thread::spawn(move || {
                for _ in 0..1000 {
                    qi_obj_retain(p as *const u8);
                    assert!(qi_obj_dec(p as *const u8) > 1);
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        unsafe {
            assert_eq!(rc_of(p as *const u8), 1);
        }
        assert_eq!(qi_obj_dec(p as *const u8), 1);
        qi_obj_free(p as *mut u8);
    }

    #[test]
    fn immortal_object_never_freed() {
        let p = qi_obj_alloc(8);
        unsafe {
            (*header_of(p))
                .refcount
                .store(IMMORTAL_RC, Ordering::Relaxed);
        }
        qi_obj_retain(p);
        let v = qi_obj_dec(p);
        assert!(
            v >= IMMORTAL_RC,
            "immortal dec 返回当前巨值，调用方不会 free"
        );
        unsafe {
            assert_eq!(rc_of(p), IMMORTAL_RC, "计数不变");
        }
        // 故意泄漏（immortal 语义）
    }
}
