//! 标准库.张量 的 FFI 外壳。算子本体在 `tensor.rs`。
//!
//! 约定：
//! - 张量是**整数句柄**，创建类函数交出新句柄，用完要 `张量.释放`；
//!   算子一律返回新张量，入参只借用读。唯一的原地操作是 `加缩放` 和 `设`。
//! - Qi 数组入参（`数组<浮点数>` / `数组<整数>`）只借用读，不私藏指针；
//!   返回数组经 [`qi_obj_alloc`] 分配（rc=1 交出），与 向量 模块同一口径。
//! - 出错（无效句柄、形状不匹配、下标越界）**抛 qi 异常**，消息带上算子名与
//!   两边的形状，能被 `尝试 / 捕获` 接住。形状错是写错了代码，默默返回 0
//!   只会让训练悄悄算出一堆垃圾。所有入口因此是 `C-unwind`：协程里没有
//!   `尝试` 时异常转成 panic 往外走，必须允许穿过这一层。
//!
//! 抛异常走 longjmp，会整帧跳过本层的栈 —— 所以 [`raise`] 之前一个拥有堆内存的
//! 局部都不能活着（消息 String 在 raise 里先拷进异常槽、再显式 drop）。

#![allow(clippy::not_unsafe_ptr_arg_deref)]

use super::exception_ffi::{qi_exc_stage, qi_exc_throw_staged};
use super::qi_str::rc_cstr_from_string;
use super::rc_obj::qi_obj_alloc;
use super::tensor::{backend_name, checked_len, live_count, matmul, SplitMix, Tensor};
use std::ffi::CString;
use std::os::raw::c_char;

/// Qi 数组长度头的防御上限（与 向量 模块相同）
const MAX_ARRAY_LEN: i64 = 1 << 32;

#[cold]
fn raise(msg: String) -> ! {
    let c = CString::new(format!("张量.{msg}")).unwrap_or_default();
    qi_exc_stage(c.as_ptr());
    drop(c);
    qi_exc_throw_staged()
}

fn ok<T>(r: Result<T, String>) -> T {
    match r {
        Ok(v) => v,
        Err(e) => raise(e),
    }
}

fn get<'a>(h: i64, op: &str) -> &'a Tensor {
    // SAFETY: 句柄合法性由魔数兜底，见 Tensor::from_handle
    match unsafe { Tensor::from_handle(h) } {
        Some(t) => t,
        None => raise(format!("{op}: 无效句柄 {h}（未创建、已释放或不是张量）")),
    }
}

fn get_mut<'a>(h: i64, op: &str) -> &'a mut Tensor {
    // SAFETY: 同 get
    match unsafe { Tensor::from_handle_mut(h) } {
        Some(t) => t,
        None => raise(format!("{op}: 无效句柄 {h}（未创建、已释放或不是张量）")),
    }
}

/// 按 Qi 数组布局（`[长度:i64][元素:8 字节]...`）借用读。
fn qi_array<'a, T>(p: *const u8, op: &str) -> &'a [T] {
    if p.is_null() {
        raise(format!("{op}: 数组为空指针"));
    }
    // SAFETY: Qi 数组头是 i64 长度，后面紧跟 8 字节元素槽
    unsafe {
        let len = *(p as *const i64);
        if !(0..=MAX_ARRAY_LEN).contains(&len) {
            raise(format!("{op}: 数组长度头异常（{len}）"));
        }
        std::slice::from_raw_parts((p as *const i64).add(1) as *const T, len as usize)
    }
}

/// 分配一个 Qi 数组（8 字节元素），元素由 `fill` 写入。
fn new_qi_array(n: usize, fill: impl FnOnce(*mut u64)) -> *mut u8 {
    let p = qi_obj_alloc(((n + 1) * 8) as i64);
    // SAFETY: qi_obj_alloc 交出 (n+1)*8 字节的已清零本体
    unsafe {
        *(p as *mut i64) = n as i64;
        fill((p as *mut u64).add(1));
    }
    p
}

fn index(t: &Tensor, r: i64, c: i64, op: &str) -> usize {
    if r < 0 || c < 0 || r as usize >= t.rows || c as usize >= t.cols {
        raise(format!("{op}: 下标 ({r}, {c}) 越界（形状 {}）", t.shape()));
    }
    r as usize * t.cols + c as usize
}

fn shape(rows: i64, cols: i64, op: &str) -> (usize, usize, usize) {
    let n = ok(checked_len(rows, cols).map_err(|e| format!("{op}: {e}")));
    (rows as usize, cols as usize, n)
}

// ── 创建 / 释放 ──────────────────────────────────────────────────────

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_zeros(rows: i64, cols: i64) -> i64 {
    let (r, c, _) = shape(rows, cols, "全零");
    Tensor::zeros(r, c).into_handle()
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_full(rows: i64, cols: i64, value: f64) -> i64 {
    let (r, c, _) = shape(rows, cols, "填充");
    Tensor::full(r, c, value as f32).into_handle()
}

/// `从数组(数组<浮点数>, 行, 列)`：行主序，长度必须正好是 行×列。
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_from_array(arr: *const u8, rows: i64, cols: i64) -> i64 {
    let (r, c, n) = shape(rows, cols, "从数组");
    let src: &[f64] = qi_array(arr, "从数组");
    if src.len() != n {
        raise(format!("从数组: 数组长度 {} 不等于 {r}×{c}={n}", src.len()));
    }
    Tensor::from_vec(r, c, src.iter().map(|&x| x as f32).collect()).into_handle()
}

/// `随机均匀(行, 列, 下界, 上界, 种子)`：同一种子在各平台出同一串数。
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_rand_uniform(
    rows: i64,
    cols: i64,
    low: f64,
    high: f64,
    seed: i64,
) -> i64 {
    let (r, c, n) = shape(rows, cols, "随机均匀");
    let mut g = SplitMix::new(seed);
    let data = (0..n)
        .map(|_| (low + (high - low) * g.next_f64()) as f32)
        .collect();
    Tensor::from_vec(r, c, data).into_handle()
}

/// `随机正态(行, 列, 均值, 标准差, 种子)`
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_rand_normal(
    rows: i64,
    cols: i64,
    mean: f64,
    std: f64,
    seed: i64,
) -> i64 {
    let (r, c, n) = shape(rows, cols, "随机正态");
    let mut g = SplitMix::new(seed);
    let data = (0..n)
        .map(|_| (mean + std * g.next_normal()) as f32)
        .collect();
    Tensor::from_vec(r, c, data).into_handle()
}

/// `独热(标签: 数组<整数>, 类别数)` → 样本数×类别数
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_one_hot(labels: *const u8, classes: i64) -> i64 {
    let ys: &[i64] = qi_array(labels, "独热");
    let (_, c, _) = shape(ys.len() as i64, classes, "独热");
    ok(Tensor::one_hot(ys, c)).into_handle()
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_clone(h: i64) -> i64 {
    let t = get(h, "复制");
    Tensor::from_vec(t.rows, t.cols, t.data.clone()).into_handle()
}

/// 释放后句柄作废。重复释放 / 无效句柄 → 返回 0（不抛异常：清理路径上别再出事）。
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_free(h: i64) -> i64 {
    // SAFETY: 魔数兜底
    unsafe { Tensor::free_handle(h) as i64 }
}

/// 当前存活的张量个数（查泄漏用）
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_live_count() -> i64 {
    live_count()
}

/// 矩阵乘后端名："accelerate"（macOS 系统 BLAS）或 "matrixmultiply"（纯 Rust）
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_backend() -> *mut c_char {
    rc_cstr_from_string(backend_name().to_string())
}

// ── 形状 / 读写 ─────────────────────────────────────────────────────

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_rows(h: i64) -> i64 {
    get(h, "行数").rows as i64
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_cols(h: i64) -> i64 {
    get(h, "列数").cols as i64
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_get(h: i64, r: i64, c: i64) -> f64 {
    let t = get(h, "取");
    t.data[index(t, r, c, "取")] as f64
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_set(h: i64, r: i64, c: i64, value: f64) -> i64 {
    let t = get_mut(h, "设");
    let i = index(t, r, c, "设");
    t.data[i] = value as f32;
    1
}

/// 转回 `数组<浮点数>`（行主序）
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_to_array(h: i64) -> *mut u8 {
    let t = get(h, "转数组");
    new_qi_array(t.data.len(), |dst| {
        for (i, &x) in t.data.iter().enumerate() {
            // SAFETY: dst 有 data.len() 个槽
            unsafe { *dst.add(i) = (x as f64).to_bits() };
        }
    })
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_slice_rows(h: i64, start: i64, count: i64) -> i64 {
    ok(get(h, "切行").slice_rows(start, count)).into_handle()
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_gather_rows(h: i64, idx: *const u8) -> i64 {
    let t = get(h, "选行");
    ok(t.gather_rows(qi_array(idx, "选行"))).into_handle()
}

// ── 线性代数 ─────────────────────────────────────────────────────────

/// 甲·乙
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_matmul(a: i64, b: i64) -> i64 {
    let (x, y) = (get(a, "矩阵乘"), get(b, "矩阵乘"));
    ok(matmul(x, false, y, false, "矩阵乘")).into_handle()
}

/// 甲ᵀ·乙（反向传播求权重梯度：输入ᵀ·上游梯度），不实际转置
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_matmul_tn(a: i64, b: i64) -> i64 {
    let (x, y) = (get(a, "转置乘"), get(b, "转置乘"));
    ok(matmul(x, true, y, false, "转置乘")).into_handle()
}

/// 甲·乙ᵀ（反向传播把梯度传回上一层：上游梯度·权重ᵀ），不实际转置
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_matmul_nt(a: i64, b: i64) -> i64 {
    let (x, y) = (get(a, "乘转置"), get(b, "乘转置"));
    ok(matmul(x, false, y, true, "乘转置")).into_handle()
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_transpose(h: i64) -> i64 {
    get(h, "转置").transpose().into_handle()
}

// ── 逐元素 ───────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_add(a: i64, b: i64) -> i64 {
    ok(get(a, "加").add(get(b, "加"))).into_handle()
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_sub(a: i64, b: i64) -> i64 {
    ok(get(a, "减").sub(get(b, "减"))).into_handle()
}

/// 逐元素乘（Hadamard），不是矩阵乘
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_mul(a: i64, b: i64) -> i64 {
    ok(get(a, "乘").mul(get(b, "乘"))).into_handle()
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_scale(h: i64, s: f64) -> i64 {
    get(h, "数乘").scale(s as f32).into_handle()
}

/// 每行加 1×列 的偏置
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_add_bias(h: i64, bias: i64) -> i64 {
    ok(get(h, "加偏置").add_row_vector(get(bias, "加偏置"))).into_handle()
}

/// 原地：目标 += 系数·源。返回 1。
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_axpy(dst: i64, src: i64, alpha: f64) -> i64 {
    if dst == src {
        // 同一个张量：不能同时可变借用和只读借用，按 (1+α)·x 处理
        let t = get_mut(dst, "加缩放");
        let k = 1.0 + alpha as f32;
        t.data.iter_mut().for_each(|x| *x *= k);
        return 1;
    }
    let s = get(src, "加缩放");
    ok(get_mut(dst, "加缩放").axpy_inplace(s, alpha as f32));
    1
}

// ── 神经网络 ─────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_relu(h: i64) -> i64 {
    get(h, "ReLU").relu().into_handle()
}

/// 前向输入 > 0 的位置为 1，其余 0
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_relu_mask(h: i64) -> i64 {
    get(h, "ReLU掩码").relu_mask().into_handle()
}

/// `ReLU反传(上游梯度, 前向输入)` = 上游梯度 ⊙ (前向输入 > 0)
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_relu_backward(grad: i64, input: i64) -> i64 {
    ok(Tensor::relu_backward(
        get(grad, "ReLU反传"),
        get(input, "ReLU反传"),
    ))
    .into_handle()
}

#[no_mangle]
pub extern "C-unwind" fn qi_tensor_softmax_rows(h: i64) -> i64 {
    get(h, "按行softmax").softmax_rows().into_handle()
}

/// `交叉熵(概率, 标签: 数组<整数>)` → 平均损失
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_cross_entropy(probs: i64, labels: *const u8) -> f64 {
    let p = get(probs, "交叉熵");
    ok(p.cross_entropy(qi_array(labels, "交叉熵")))
}

/// `交叉熵梯度(概率, 标签)` = (概率 - 独热) / 样本数：softmax + 平均交叉熵对 logits 的梯度
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_cross_entropy_grad(probs: i64, labels: *const u8) -> i64 {
    let p = get(probs, "交叉熵梯度");
    ok(p.cross_entropy_grad(qi_array(labels, "交叉熵梯度"))).into_handle()
}

// ── 归约 ─────────────────────────────────────────────────────────────

/// 全部元素之和（f64 累加）
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_sum(h: i64) -> f64 {
    get(h, "求和").sum()
}

/// → 1×列
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_sum_cols(h: i64) -> i64 {
    get(h, "按列求和").sum_cols().into_handle()
}

/// → 行×1
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_sum_rows(h: i64) -> i64 {
    get(h, "按行求和").sum_rows().into_handle()
}

/// 每行最大值的列下标 → `数组<整数>`
#[no_mangle]
pub extern "C-unwind" fn qi_tensor_argmax_rows(h: i64) -> *mut u8 {
    let idx = get(h, "按行最大下标").argmax_rows();
    new_qi_array(idx.len(), |dst| {
        for (i, &x) in idx.iter().enumerate() {
            // SAFETY: dst 有 idx.len() 个槽
            unsafe { *dst.add(i) = x as u64 };
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::rc_obj::qi_obj_free;

    fn qi_floats(v: &[f64]) -> Vec<i64> {
        let mut out = vec![v.len() as i64];
        out.extend(v.iter().map(|x| x.to_bits() as i64));
        out
    }

    #[test]
    fn ffi_round_trip_through_qi_arrays() {
        let src = qi_floats(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
        let a = qi_tensor_from_array(src.as_ptr() as *const u8, 2, 3);
        let b = qi_tensor_transpose(a);
        let c = qi_tensor_matmul(a, b); // 2×2
        assert_eq!((qi_tensor_rows(c), qi_tensor_cols(c)), (2, 2));
        assert_eq!(qi_tensor_get(c, 0, 0), 14.0);
        assert_eq!(qi_tensor_get(c, 1, 1), 77.0);
        let tn = qi_tensor_matmul_tn(a, a); // 3×3 = aᵀa
        assert_eq!(qi_tensor_get(tn, 2, 2), 45.0);
        let nt = qi_tensor_matmul_nt(a, a); // 2×2 = a aᵀ
        assert_eq!(qi_tensor_get(nt, 0, 1), 32.0);

        let arr = qi_tensor_to_array(c);
        // SAFETY: qi 数组布局
        unsafe {
            assert_eq!(*(arr as *const i64), 4);
            assert_eq!(f64::from_bits(*(arr as *const u64).add(2)), 32.0);
        }
        qi_obj_free(arr);

        let am = qi_tensor_argmax_rows(a);
        // SAFETY: qi 数组布局
        unsafe {
            assert_eq!(*(am as *const i64).add(1), 2);
        }
        qi_obj_free(am);
        // Qi 数组：[长度=2, 2, 0]
        let oh = qi_tensor_one_hot([2i64, 2, 0].as_ptr() as *const u8, 3);
        assert_eq!(qi_tensor_sum(oh), 2.0);
        assert_eq!(qi_tensor_get(oh, 1, 0), 1.0);

        assert_eq!(qi_tensor_axpy(c, c, 1.0), 1);
        assert_eq!(qi_tensor_get(c, 0, 0), 28.0);

        for h in [a, b, c, tn, nt, oh] {
            assert_eq!(qi_tensor_free(h), 1);
        }
        assert_eq!(qi_tensor_free(a), 0);
    }
}
