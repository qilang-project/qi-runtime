//! 向量模块 FFI —— 直接按 **Qi 数组内存布局** 收发，Qi 侧零出参缓冲。
//!
//! Qi 数组布局（见 qi/src/codegen/inkwell_gen/数组.rs）：
//! ```text
//! ptr → [长度:i64][elem0:8字节][elem1:8字节]...
//! ```
//! 浮点数组元素槽存 f64 位模式。本模块所有入参数组按 f64 元素解读
//! （整数数组误入会算出垃圾值 —— 调用方须传浮点数组，如 `[1.0, 2.0]`）。
//!
//! 约定：
//! - 入参数组一律**借用读**（不私藏指针、不释放）；
//! - 返回数组一律经 [`qi_obj_alloc`] 分配（带 RC header，rc=1 交出），
//!   QI_ARC=1 时 codegen 的 `qi.release.arr.v` 可正确回收；QI_ARC=0 时
//!   与 codegen 自身的 qi_runtime_alloc 数组同样不回收（该模式常态）；
//! - 维度不匹配 / 空指针 / 零向量归一化 → 返回 0.0 或零向量，并发一次性
//!   防御日志（宁保守，绝不崩）。

// FFI 入口按 C ABI 收裸指针（codegen 直接 call），内部对空指针/坏长度头
// 自防御，无法也不应标 unsafe（LLVM 侧不认识 Rust 的 unsafe）。
#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::rc_obj::qi_obj_alloc;
use super::vector::向量;
use std::sync::atomic::{AtomicBool, Ordering};

/// 防御长度上限：长度头超出此值视为坏指针/类型混淆（不是浮点数组）。
const 最大长度: i64 = 1 << 32;

/// 一次性防御日志（进程级，首次异常入参提示后静默）。
static 已警告: AtomicBool = AtomicBool::new(false);

#[cold]
fn 防御警告(msg: &str) {
    if !已警告.swap(true, Ordering::Relaxed) {
        eprintln!(
            "[qi-vector] {}（后续同类情况将静默处理，不再重复警告）",
            msg
        );
    }
}

/// 按 Qi 数组布局借用读一个 f64 切片。空指针 / 长度头异常 → None。
unsafe fn 读浮点数组<'a>(p: *const u8) -> Option<&'a [f64]> {
    if p.is_null() {
        return None;
    }
    let len = *(p as *const i64);
    if !(0..=最大长度).contains(&len) {
        return None;
    }
    Some(std::slice::from_raw_parts(
        (p as *const f64).add(1),
        len as usize,
    ))
}

/// 按 Qi 数组布局分配返回数组（qi_obj_alloc：RC header + 长度头 + 元素）。
fn 新建返回数组(元素: &[f64]) -> *mut u8 {
    let n = 元素.len();
    // (n+1) 槽：0 号长度头，其余元素，每槽 8 字节（与 codegen 数组字面量同构）
    let p = qi_obj_alloc(((n + 1) * 8) as i64);
    unsafe {
        *(p as *mut i64) = n as i64;
        std::ptr::copy_nonoverlapping(元素.as_ptr(), (p as *mut f64).add(1), n);
    }
    p
}

/// 向量点积：`向量.点积(数组, 数组) : 浮点数`。
/// 空指针 / 维度不匹配 → 0.0（一次性防御日志）。
#[no_mangle]
pub extern "C" fn qi_vector_dot(a: *const u8, b: *const u8) -> f64 {
    unsafe {
        let (甲, 乙) = match (读浮点数组(a), 读浮点数组(b)) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                防御警告("点积: 无效数组指针，返回 0.0");
                return 0.0;
            }
        };
        match 向量::从数组(甲).点积(&向量::从数组(乙)) {
            Ok(v) => v,
            Err(_) => {
                防御警告("点积: 向量维度不匹配，返回 0.0");
                0.0
            }
        }
    }
}

/// 向量加法：`向量.加(数组, 数组) : 数组`（返回新数组，rc=1 交出）。
/// 空指针 / 维度不匹配 → 零长度数组（一次性防御日志）。
#[no_mangle]
pub extern "C" fn qi_vector_add(a: *const u8, b: *const u8) -> *mut u8 {
    unsafe {
        let (甲, 乙) = match (读浮点数组(a), 读浮点数组(b)) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                防御警告("加: 无效数组指针，返回空数组");
                return 新建返回数组(&[]);
            }
        };
        match 向量::从数组(甲).加(&向量::从数组(乙)) {
            Ok(v) => 新建返回数组(&v.元素),
            Err(_) => {
                防御警告("加: 向量维度不匹配，返回空数组");
                新建返回数组(&[])
            }
        }
    }
}

/// 向量长度（模）：`向量.长度(数组) : 浮点数`。空指针 → 0.0。
#[no_mangle]
pub extern "C" fn qi_vector_magnitude(a: *const u8) -> f64 {
    unsafe {
        match 读浮点数组(a) {
            Some(x) => 向量::从数组(x).长度(),
            None => {
                防御警告("长度: 无效数组指针，返回 0.0");
                0.0
            }
        }
    }
}

/// 向量归一化：`向量.归一化(数组) : 数组`（返回新数组，rc=1 交出）。
/// 零向量 → 同长度零向量；空指针 → 零长度数组（一次性防御日志）。
#[no_mangle]
pub extern "C" fn qi_vector_normalize(a: *const u8) -> *mut u8 {
    unsafe {
        let 甲 = match 读浮点数组(a) {
            Some(x) => x,
            None => {
                防御警告("归一化: 无效数组指针，返回空数组");
                return 新建返回数组(&[]);
            }
        };
        match 向量::从数组(甲).归一化() {
            Ok(v) => 新建返回数组(&v.元素),
            Err(_) => {
                防御警告("归一化: 零向量，返回同长度零向量");
                新建返回数组(&vec![0.0; 甲.len()])
            }
        }
    }
}

/// 向量数乘：`向量.数乘(数组, 浮点数) : 数组`（返回新数组，rc=1 交出）。
#[no_mangle]
pub extern "C" fn qi_vector_scale(a: *const u8, 标量: f64) -> *mut u8 {
    unsafe {
        match 读浮点数组(a) {
            Some(x) => 新建返回数组(&向量::从数组(x).数乘(标量).元素),
            None => {
                防御警告("数乘: 无效数组指针，返回空数组");
                新建返回数组(&[])
            }
        }
    }
}

/// 余弦相似度：`向量.余弦相似度(数组, 数组) : 浮点数`。
/// 空指针 / 维度不匹配 / 任一零向量 → 0.0（一次性防御日志）。
#[no_mangle]
pub extern "C" fn qi_vector_cosine_similarity(a: *const u8, b: *const u8) -> f64 {
    unsafe {
        let (甲, 乙) = match (读浮点数组(a), 读浮点数组(b)) {
            (Some(x), Some(y)) => (x, y),
            _ => {
                防御警告("余弦相似度: 无效数组指针，返回 0.0");
                return 0.0;
            }
        };
        let (v1, v2) = (向量::从数组(甲), 向量::从数组(乙));
        let 点积 = match v1.点积(&v2) {
            Ok(v) => v,
            Err(_) => {
                防御警告("余弦相似度: 向量维度不匹配，返回 0.0");
                return 0.0;
            }
        };
        let 模积 = v1.长度() * v2.长度();
        if 模积 == 0.0 {
            防御警告("余弦相似度: 含零向量，返回 0.0");
            return 0.0;
        }
        点积 / 模积
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::rc_obj::{qi_obj_dec, qi_obj_free};

    /// 按 Qi 数组布局造一个测试数组（[len][f64 位模式...]）。
    fn qi数组(vals: &[f64]) -> Vec<i64> {
        let mut v = vec![vals.len() as i64];
        v.extend(vals.iter().map(|f| f.to_bits() as i64));
        v
    }

    fn 指针(buf: &[i64]) -> *const u8 {
        buf.as_ptr() as *const u8
    }

    /// 读返回数组内容并按 RC 约定释放（rc=1 交出 → dec 到 0 后 free）。
    fn 取回并释放(p: *mut u8) -> Vec<f64> {
        assert!(!p.is_null());
        let out = unsafe { 读浮点数组(p).expect("返回数组布局非法").to_vec() };
        assert_eq!(qi_obj_dec(p), 1, "返回数组必须 rc=1 交出");
        qi_obj_free(p);
        out
    }

    #[test]
    fn dot_qi_layout() {
        let a = qi数组(&[1.0, 2.0, 3.0]);
        let b = qi数组(&[4.0, 5.0, 6.0]);
        assert_eq!(qi_vector_dot(指针(&a), 指针(&b)), 32.0);
    }

    #[test]
    fn dot_mismatch_returns_zero() {
        let a = qi数组(&[1.0, 2.0]);
        let b = qi数组(&[1.0, 2.0, 3.0]);
        assert_eq!(qi_vector_dot(指针(&a), 指针(&b)), 0.0);
        assert_eq!(qi_vector_dot(std::ptr::null(), 指针(&b)), 0.0);
    }

    #[test]
    fn add_returns_new_rc_array() {
        let a = qi数组(&[1.0, 2.0]);
        let b = qi数组(&[3.0, 4.5]);
        let out = 取回并释放(qi_vector_add(指针(&a), 指针(&b)));
        assert_eq!(out, vec![4.0, 6.5]);
    }

    #[test]
    fn add_mismatch_returns_empty() {
        let a = qi数组(&[1.0]);
        let b = qi数组(&[1.0, 2.0]);
        let out = 取回并释放(qi_vector_add(指针(&a), 指针(&b)));
        assert!(out.is_empty());
    }

    #[test]
    fn magnitude_3_4_5() {
        let a = qi数组(&[3.0, 4.0]);
        assert_eq!(qi_vector_magnitude(指针(&a)), 5.0);
        assert_eq!(qi_vector_magnitude(std::ptr::null()), 0.0);
    }

    #[test]
    fn normalize_unit_length() {
        let a = qi数组(&[3.0, 4.0]);
        let out = 取回并释放(qi_vector_normalize(指针(&a)));
        assert_eq!(out, vec![0.6, 0.8]);
        // 零向量 → 同长度零向量，不崩
        let z = qi数组(&[0.0, 0.0, 0.0]);
        let out = 取回并释放(qi_vector_normalize(指针(&z)));
        assert_eq!(out, vec![0.0, 0.0, 0.0]);
    }

    #[test]
    fn scale_by_scalar() {
        let a = qi数组(&[1.5, -2.0, 0.0]);
        let out = 取回并释放(qi_vector_scale(指针(&a), 2.0));
        assert_eq!(out, vec![3.0, -4.0, 0.0]);
    }

    #[test]
    fn cosine_parallel_and_orthogonal() {
        let a = qi数组(&[2.0, 0.0]);
        let b = qi数组(&[7.5, 0.0]);
        assert!((qi_vector_cosine_similarity(指针(&a), 指针(&b)) - 1.0).abs() < 1e-10);
        let c = qi数组(&[0.0, 3.0]);
        assert!(qi_vector_cosine_similarity(指针(&a), 指针(&c)).abs() < 1e-10);
        // 零向量 → 0.0
        let z = qi数组(&[0.0, 0.0]);
        assert_eq!(qi_vector_cosine_similarity(指针(&a), 指针(&z)), 0.0);
    }

    #[test]
    fn bogus_length_header_rejected() {
        // 长度头是垃圾（负数 / 天文数字）→ 按无效数组处理，不越界读
        let bad = vec![-3i64, 0, 0];
        assert_eq!(qi_vector_magnitude(指针(&bad)), 0.0);
        let bad2 = vec![i64::MAX, 0, 0];
        assert_eq!(qi_vector_dot(指针(&bad2), 指针(&bad2)), 0.0);
    }
}
