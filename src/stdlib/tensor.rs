//! 标准库.张量 的本体 —— f32、行主序、二维的稠密张量，以及训练小模型要的算子。
//!
//! FFI 外壳在 `tensor_ffi.rs`；这里只有纯 Rust 的数据结构和算子，便于单测。
//!
//! # 为什么是 f32
//!
//! 训练用 f32 是行业惯例：同样的带宽搬两倍的数，SIMD 一条指令算两倍的数，
//! Apple 的 AMX / BLAS 的 sgemm 都比 dgemm 快一倍以上。精度对小模型的 SGD
//! 足够（梯度噪声远大于 f32 的舍入误差）。求和类归约在 f64 里累加，返回 f64，
//! 避免长向量求和的误差堆积。
//!
//! # 句柄
//!
//! 句柄就是 `Box<Tensor>` 的地址（转成 i64）。开头放一个魔数，取用时先验魔数：
//! 0、没对齐、魔数不对一律视为无效句柄。释放时先把魔数改成「已死」再回收，
//! 释放后再用多半能被魔数检查拦下（内存被别人复用时拦不住 —— 释放后别再用）。
//! 不走「全局表 + 锁」是因为 取/设 这种逐元素调用也要便宜。
//!
//! # 矩阵乘的后端
//!
//! - macOS：运行时 dlopen Accelerate（系统自带，不用改链接行），用 cblas_sgemm，
//!   M 系列芯片上会走 AMX 协处理器。没用到矩阵乘的程序一个字节也不多加载。
//! - 其余平台（以及 macOS 上 `QI_TENSOR_BACKEND=rust`）：纯 Rust 的 matrixmultiply，
//!   无系统 BLAS 依赖，wasm32-wasip1 也能编。

use std::sync::atomic::{AtomicI64, Ordering};

/// "QITENSOR"
const TENSOR_MAGIC: u64 = 0x5149_5445_4e53_4f52;
/// 释放时写进去的魔数，与有效魔数不同即可
const DEAD_MAGIC: u64 = 0x0dea_d7e4_50de_ad00;
/// 单个张量的元素上限（2^31）：再大基本是行列写反了或者传了垃圾值
const MAX_ELEMS: usize = 1 << 31;

/// 存活张量数（创建 +1、释放 -1）。查泄漏用：训练循环跑完它应当回到起点。
static LIVE: AtomicI64 = AtomicI64::new(0);

pub fn live_count() -> i64 {
    LIVE.load(Ordering::Relaxed)
}

#[derive(Debug)]
#[repr(C)]
pub struct Tensor {
    magic: u64,
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f32>,
}

/// 校验行列并算元素个数；负数 / 溢出 / 超上限 → Err。
pub fn checked_len(rows: i64, cols: i64) -> Result<usize, String> {
    if rows < 0 || cols < 0 {
        return Err(format!("行列不能为负（{rows}×{cols}）"));
    }
    match (rows as usize).checked_mul(cols as usize) {
        Some(n) if n <= MAX_ELEMS => Ok(n),
        _ => Err(format!("形状过大（{rows}×{cols}）")),
    }
}

impl Tensor {
    pub fn from_vec(rows: usize, cols: usize, data: Vec<f32>) -> Self {
        debug_assert_eq!(rows * cols, data.len());
        Tensor {
            magic: TENSOR_MAGIC,
            rows,
            cols,
            data,
        }
    }

    pub fn zeros(rows: usize, cols: usize) -> Self {
        Self::from_vec(rows, cols, vec![0.0; rows * cols])
    }

    pub fn full(rows: usize, cols: usize, v: f32) -> Self {
        Self::from_vec(rows, cols, vec![v; rows * cols])
    }

    pub fn shape(&self) -> String {
        format!("{}×{}", self.rows, self.cols)
    }

    pub fn into_handle(self) -> i64 {
        LIVE.fetch_add(1, Ordering::Relaxed);
        Box::into_raw(Box::new(self)) as usize as i64
    }

    fn handle_ptr(h: i64) -> Option<*mut Tensor> {
        if h <= 0 || !(h as usize).is_multiple_of(std::mem::align_of::<Tensor>()) {
            return None;
        }
        let p = h as usize as *mut Tensor;
        // SAFETY: 句柄由 into_handle 交出；非零且对齐，读魔数做最后一道检查
        if unsafe { (*p).magic } != TENSOR_MAGIC {
            return None;
        }
        Some(p)
    }

    /// 句柄 → 借用。无效句柄返回 None。
    ///
    /// # Safety
    /// 句柄必须来自 [`Tensor::into_handle`] 且尚未释放（魔数只能拦住大部分误用）。
    pub unsafe fn from_handle<'a>(h: i64) -> Option<&'a Tensor> {
        Self::handle_ptr(h).map(|p| &*p)
    }

    /// # Safety
    /// 同 [`Tensor::from_handle`]，且调用期间没有别的借用。
    pub unsafe fn from_handle_mut<'a>(h: i64) -> Option<&'a mut Tensor> {
        Self::handle_ptr(h).map(|p| &mut *p)
    }

    /// 释放句柄。无效句柄返回 false。
    ///
    /// # Safety
    /// 同 [`Tensor::from_handle`]。
    pub unsafe fn free_handle(h: i64) -> bool {
        match Self::handle_ptr(h) {
            Some(p) => {
                (*p).magic = DEAD_MAGIC;
                drop(Box::from_raw(p));
                LIVE.fetch_sub(1, Ordering::Relaxed);
                true
            }
            None => false,
        }
    }

    pub fn get(&self, r: usize, c: usize) -> f32 {
        self.data[r * self.cols + c]
    }

    fn same_shape(&self, other: &Tensor, op: &str) -> Result<(), String> {
        if self.rows == other.rows && self.cols == other.cols {
            Ok(())
        } else {
            Err(format!(
                "{op}: 形状不一致（{} 与 {}）",
                self.shape(),
                other.shape()
            ))
        }
    }

    fn map(&self, f: impl Fn(f32) -> f32) -> Tensor {
        Tensor::from_vec(
            self.rows,
            self.cols,
            self.data.iter().map(|&x| f(x)).collect(),
        )
    }

    fn zip(&self, other: &Tensor, op: &str, f: impl Fn(f32, f32) -> f32) -> Result<Tensor, String> {
        self.same_shape(other, op)?;
        let data = self
            .data
            .iter()
            .zip(&other.data)
            .map(|(&a, &b)| f(a, b))
            .collect();
        Ok(Tensor::from_vec(self.rows, self.cols, data))
    }

    pub fn add(&self, o: &Tensor) -> Result<Tensor, String> {
        self.zip(o, "加", |a, b| a + b)
    }

    pub fn sub(&self, o: &Tensor) -> Result<Tensor, String> {
        self.zip(o, "减", |a, b| a - b)
    }

    pub fn mul(&self, o: &Tensor) -> Result<Tensor, String> {
        self.zip(o, "乘", |a, b| a * b)
    }

    pub fn scale(&self, s: f32) -> Tensor {
        self.map(|x| x * s)
    }

    /// 目标 += 系数·源（原地）。SGD 更新 `权 -= 学习率·梯度` 不用再分配。
    pub fn axpy_inplace(&mut self, src: &Tensor, alpha: f32) -> Result<(), String> {
        self.same_shape(src, "加缩放")?;
        for (d, &s) in self.data.iter_mut().zip(&src.data) {
            *d += alpha * s;
        }
        Ok(())
    }

    /// 每行加同一个行向量（1×列）—— 全连接层的偏置。
    pub fn add_row_vector(&self, bias: &Tensor) -> Result<Tensor, String> {
        if bias.rows != 1 || bias.cols != self.cols {
            return Err(format!(
                "加偏置: 偏置须是 1×{}，实际 {}",
                self.cols,
                bias.shape()
            ));
        }
        let mut out = self.data.clone();
        if self.cols > 0 {
            for row in out.chunks_exact_mut(self.cols) {
                for (x, &b) in row.iter_mut().zip(&bias.data) {
                    *x += b;
                }
            }
        }
        Ok(Tensor::from_vec(self.rows, self.cols, out))
    }

    pub fn transpose(&self) -> Tensor {
        let (r, c) = (self.rows, self.cols);
        let mut out = vec![0.0f32; r * c];
        // 分块转置，两边访存都局部
        const B: usize = 32;
        for i0 in (0..r).step_by(B) {
            for j0 in (0..c).step_by(B) {
                for i in i0..(i0 + B).min(r) {
                    for j in j0..(j0 + B).min(c) {
                        out[j * r + i] = self.data[i * c + j];
                    }
                }
            }
        }
        Tensor::from_vec(c, r, out)
    }

    pub fn relu(&self) -> Tensor {
        self.map(|x| if x > 0.0 { x } else { 0.0 })
    }

    pub fn relu_mask(&self) -> Tensor {
        self.map(|x| if x > 0.0 { 1.0 } else { 0.0 })
    }

    /// ReLU 的反向：上游梯度 ⊙ (前向输入 > 0)。
    pub fn relu_backward(grad: &Tensor, input: &Tensor) -> Result<Tensor, String> {
        grad.zip(input, "ReLU反传", |g, x| if x > 0.0 { g } else { 0.0 })
    }

    /// 按行 softmax（每行减最大值再取指数，数值稳定）。
    pub fn softmax_rows(&self) -> Tensor {
        let mut out = self.data.clone();
        if self.cols > 0 {
            for row in out.chunks_exact_mut(self.cols) {
                let m = row.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                let mut s = 0.0f32;
                for x in row.iter_mut() {
                    *x = (*x - m).exp();
                    s += *x;
                }
                let inv = 1.0 / s;
                for x in row.iter_mut() {
                    *x *= inv;
                }
            }
        }
        Tensor::from_vec(self.rows, self.cols, out)
    }

    fn check_labels(&self, labels: &[i64], op: &str) -> Result<(), String> {
        if labels.len() != self.rows {
            return Err(format!(
                "{op}: 标签个数 {} 与行数 {} 不一致",
                labels.len(),
                self.rows
            ));
        }
        if let Some(&bad) = labels.iter().find(|&&y| y < 0 || y as usize >= self.cols) {
            return Err(format!("{op}: 标签 {bad} 越界（类别数 {}）", self.cols));
        }
        Ok(())
    }

    /// 平均交叉熵：-mean(ln p[i, y_i])。概率先夹到 1e-12 以上，避免 ln 0。
    pub fn cross_entropy(&self, labels: &[i64]) -> Result<f64, String> {
        self.check_labels(labels, "交叉熵")?;
        if self.rows == 0 {
            return Ok(0.0);
        }
        let mut s = 0.0f64;
        for (i, &y) in labels.iter().enumerate() {
            let p = self.get(i, y as usize) as f64;
            s -= p.max(1e-12).ln();
        }
        Ok(s / self.rows as f64)
    }

    /// softmax + 平均交叉熵 对 logits 的梯度：(概率 - 独热) / 行数。
    pub fn cross_entropy_grad(&self, labels: &[i64]) -> Result<Tensor, String> {
        self.check_labels(labels, "交叉熵梯度")?;
        let mut out = self.data.clone();
        for (i, &y) in labels.iter().enumerate() {
            out[i * self.cols + y as usize] -= 1.0;
        }
        let inv = if self.rows > 0 {
            1.0 / self.rows as f32
        } else {
            0.0
        };
        for x in out.iter_mut() {
            *x *= inv;
        }
        Ok(Tensor::from_vec(self.rows, self.cols, out))
    }

    pub fn one_hot(labels: &[i64], classes: usize) -> Result<Tensor, String> {
        let mut t = Tensor::zeros(labels.len(), classes);
        t.check_labels(labels, "独热")?;
        for (i, &y) in labels.iter().enumerate() {
            t.data[i * classes + y as usize] = 1.0;
        }
        Ok(t)
    }

    pub fn sum(&self) -> f64 {
        self.data.iter().map(|&x| x as f64).sum()
    }

    /// 按列求和 → 1×列（偏置的梯度）。
    pub fn sum_cols(&self) -> Tensor {
        let mut acc = vec![0.0f64; self.cols];
        if self.cols > 0 {
            for row in self.data.chunks_exact(self.cols) {
                for (a, &x) in acc.iter_mut().zip(row) {
                    *a += x as f64;
                }
            }
        }
        Tensor::from_vec(1, self.cols, acc.into_iter().map(|x| x as f32).collect())
    }

    /// 按行求和 → 行×1。
    pub fn sum_rows(&self) -> Tensor {
        let data = if self.cols == 0 {
            vec![0.0; self.rows]
        } else {
            self.data
                .chunks_exact(self.cols)
                .map(|row| row.iter().map(|&x| x as f64).sum::<f64>() as f32)
                .collect()
        };
        Tensor::from_vec(self.rows, 1, data)
    }

    /// 每行最大值的列下标（并列取最前）；0 列的张量每行给 -1。
    pub fn argmax_rows(&self) -> Vec<i64> {
        if self.cols == 0 {
            return vec![-1; self.rows];
        }
        self.data
            .chunks_exact(self.cols)
            .map(|row| {
                let mut best = 0usize;
                for (j, &x) in row.iter().enumerate().skip(1) {
                    if x > row[best] {
                        best = j;
                    }
                }
                best as i64
            })
            .collect()
    }

    /// 连续的若干行（小批量切片）。
    pub fn slice_rows(&self, start: i64, count: i64) -> Result<Tensor, String> {
        if start < 0 || count < 0 || (start as usize).saturating_add(count as usize) > self.rows {
            return Err(format!(
                "切行: 起始 {start} 行数 {count} 超出 {} 行",
                self.rows
            ));
        }
        let (s, n) = (start as usize, count as usize);
        let data = self.data[s * self.cols..(s + n) * self.cols].to_vec();
        Ok(Tensor::from_vec(n, self.cols, data))
    }

    /// 按下标挑行（打乱后的小批量）。
    pub fn gather_rows(&self, idx: &[i64]) -> Result<Tensor, String> {
        let mut data = Vec::with_capacity(idx.len() * self.cols);
        for &i in idx {
            if i < 0 || i as usize >= self.rows {
                return Err(format!("选行: 下标 {i} 越界（共 {} 行）", self.rows));
            }
            let s = i as usize * self.cols;
            data.extend_from_slice(&self.data[s..s + self.cols]);
        }
        Ok(Tensor::from_vec(idx.len(), self.cols, data))
    }
}

// ── 随机 ─────────────────────────────────────────────────────────────
//
// 自带 splitmix64：同一个种子在原生 / wasm、mac / Linux 上出同一串数，
// 跑分和回归才对得上。

pub struct SplitMix(u64);

impl SplitMix {
    pub fn new(seed: i64) -> Self {
        SplitMix(seed as u64)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// [0, 1)
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// 标准正态（Box–Muller，一次取一个，简单优先）
    pub fn next_normal(&mut self) -> f64 {
        let u1 = 1.0 - self.next_f64(); // (0, 1]
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

// ── 矩阵乘 ───────────────────────────────────────────────────────────

/// 当前矩阵乘后端的名字（"accelerate" / "matrixmultiply"）。
pub fn backend_name() -> &'static str {
    #[cfg(target_os = "macos")]
    if accelerate::sgemm().is_some() {
        return "accelerate";
    }
    "matrixmultiply"
}

/// op(A)·op(B)。`ta`/`tb` 表示先转置再乘（不实际搬数据，只换步长）。
pub fn matmul(a: &Tensor, ta: bool, b: &Tensor, tb: bool, op: &str) -> Result<Tensor, String> {
    let (m, k) = if ta {
        (a.cols, a.rows)
    } else {
        (a.rows, a.cols)
    };
    let (kb, n) = if tb {
        (b.cols, b.rows)
    } else {
        (b.rows, b.cols)
    };
    if k != kb {
        let sa = if ta {
            format!("{}ᵀ", a.shape())
        } else {
            a.shape()
        };
        let sb = if tb {
            format!("{}ᵀ", b.shape())
        } else {
            b.shape()
        };
        return Err(format!("{op}: 形状不匹配（{sa} · {sb}，内维 {k} ≠ {kb}）"));
    }
    if m == 0 || n == 0 || k == 0 {
        return Ok(Tensor::zeros(m, n));
    }
    // 结果不先清零：beta = 0 时 cblas 与 matrixmultiply 都保证不读 C、只写满 m×n。
    // 1024² 的结果是 4MB，省一遍 memset 在 Accelerate 上约是一成的耗时。
    let mut out: Vec<f32> = Vec::with_capacity(m * n);
    sgemm(
        m,
        n,
        k,
        &a.data,
        a.cols,
        ta,
        &b.data,
        b.cols,
        tb,
        out.as_mut_ptr(),
    );
    // SAFETY: sgemm 已写满前 m*n 个元素
    unsafe { out.set_len(m * n) };
    Ok(Tensor::from_vec(m, n, out))
}

/// C(m×n) = op(A)(m×k) · op(B)(k×n)，A/B/C 全是行主序连续存储，
/// lda/ldb 是**存储**的行宽（转不转置都一样）。C 至少有 m*n 个元素的空间，
/// 可以未初始化（beta = 0，只写不读）。
#[allow(clippy::too_many_arguments)]
fn sgemm(
    m: usize,
    n: usize,
    k: usize,
    a: &[f32],
    lda: usize,
    ta: bool,
    b: &[f32],
    ldb: usize,
    tb: bool,
    c: *mut f32,
) {
    #[cfg(target_os = "macos")]
    if let Some(f) = accelerate::sgemm() {
        // 维度超过 i32 走不了 cblas（LP64 接口），落到纯 Rust
        if [m, n, k, lda, ldb].iter().all(|&x| x <= i32::MAX as usize) {
            // SAFETY: 切片长度由调用方按形状保证；cblas 只读 A/B、只写 C 的 m×n
            unsafe {
                f(
                    101, // CblasRowMajor
                    if ta { 112 } else { 111 },
                    if tb { 112 } else { 111 },
                    m as i32,
                    n as i32,
                    k as i32,
                    1.0,
                    a.as_ptr(),
                    lda as i32,
                    b.as_ptr(),
                    ldb as i32,
                    0.0,
                    c,
                    n as i32,
                );
            }
            return;
        }
    }
    // 存储 (i, j) 在 i*ld + j；转置视图的 (i, j) 是存储的 (j, i)
    let (rsa, csa) = if ta { (1, lda) } else { (lda, 1) };
    let (rsb, csb) = if tb { (1, ldb) } else { (ldb, 1) };
    // SAFETY: 同上；matrixmultiply 按给定步长访问，不越出各自切片
    unsafe {
        matrixmultiply::sgemm(
            m,
            k,
            n,
            1.0,
            a.as_ptr(),
            rsa as isize,
            csa as isize,
            b.as_ptr(),
            rsb as isize,
            csb as isize,
            0.0,
            c,
            n as isize,
            1,
        );
    }
}

#[cfg(target_os = "macos")]
mod accelerate {
    use std::sync::OnceLock;

    pub type SgemmFn = unsafe extern "C" fn(
        i32,
        i32,
        i32,
        i32,
        i32,
        i32,
        f32,
        *const f32,
        i32,
        *const f32,
        i32,
        f32,
        *mut f32,
        i32,
    );

    static SGEMM: OnceLock<Option<SgemmFn>> = OnceLock::new();

    /// 第一次用到时 dlopen 系统 Accelerate；拿不到（或 QI_TENSOR_BACKEND=rust）就 None。
    pub fn sgemm() -> Option<SgemmFn> {
        *SGEMM.get_or_init(|| {
            if std::env::var("QI_TENSOR_BACKEND").is_ok_and(|v| v == "rust") {
                return None;
            }
            // SAFETY: 路径与符号名都是 NUL 结尾的常量；dlsym 结果按 cblas_sgemm 的 C 签名解释
            unsafe {
                let h = libc::dlopen(
                    c"/System/Library/Frameworks/Accelerate.framework/Accelerate".as_ptr(),
                    libc::RTLD_LAZY | libc::RTLD_LOCAL,
                );
                if h.is_null() {
                    return None;
                }
                let s = libc::dlsym(h, c"cblas_sgemm".as_ptr());
                if s.is_null() {
                    None
                } else {
                    Some(std::mem::transmute::<*mut libc::c_void, SgemmFn>(s))
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(r: usize, c: usize, v: &[f32]) -> Tensor {
        Tensor::from_vec(r, c, v.to_vec())
    }

    fn naive(a: &Tensor, b: &Tensor) -> Vec<f32> {
        let mut c = vec![0.0; a.rows * b.cols];
        for i in 0..a.rows {
            for p in 0..a.cols {
                for j in 0..b.cols {
                    c[i * b.cols + j] += a.get(i, p) * b.get(p, j);
                }
            }
        }
        c
    }

    #[test]
    fn matmul_and_transposed_variants_agree_with_naive() {
        let mut rng = SplitMix::new(7);
        let mut rand = |r, c| {
            let v: Vec<f32> = (0..r * c)
                .map(|_| (rng.next_f64() * 4.0 - 2.0) as f32)
                .collect();
            Tensor::from_vec(r, c, v)
        };
        let a = rand(37, 19);
        let b = rand(19, 23);
        let want = naive(&a, &b);
        let close = |x: &[f32]| x.iter().zip(&want).all(|(p, q)| (p - q).abs() < 1e-4);
        assert!(close(&matmul(&a, false, &b, false, "t").unwrap().data));
        let at = a.transpose();
        let bt = b.transpose();
        assert!(close(&matmul(&at, true, &b, false, "t").unwrap().data));
        assert!(close(&matmul(&a, false, &bt, true, "t").unwrap().data));
        assert!(close(&matmul(&at, true, &bt, true, "t").unwrap().data));
        assert!(matmul(&a, false, &a, false, "矩阵乘")
            .unwrap_err()
            .contains("内维 19 ≠ 37"));
    }

    #[test]
    fn empty_inner_dimension_gives_zeros() {
        let a = Tensor::zeros(3, 0);
        let b = Tensor::zeros(0, 2);
        let c = matmul(&a, false, &b, false, "t").unwrap();
        assert_eq!((c.rows, c.cols), (3, 2));
        assert!(c.data.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn handles_round_trip_and_reject_garbage() {
        let h = Tensor::zeros(2, 3).into_handle();
        unsafe {
            assert_eq!(Tensor::from_handle(h).unwrap().cols, 3);
            assert!(Tensor::from_handle(0).is_none());
            assert!(Tensor::from_handle(h + 1).is_none());
            assert!(Tensor::free_handle(h));
        }
    }

    #[test]
    fn softmax_cross_entropy_and_grad() {
        let x = t(2, 3, &[1.0, 2.0, 3.0, 1000.0, 0.0, -1000.0]);
        let p = x.softmax_rows();
        for row in p.data.chunks(3) {
            assert!((row.iter().sum::<f32>() - 1.0).abs() < 1e-6);
        }
        assert!((p.get(1, 0) - 1.0).abs() < 1e-6);
        let ce = p.cross_entropy(&[2, 0]).unwrap();
        let want = -(p.get(0, 2) as f64).ln() / 2.0;
        assert!((ce - want).abs() < 1e-6);
        let g = p.cross_entropy_grad(&[2, 0]).unwrap();
        assert!((g.get(0, 2) - (p.get(0, 2) - 1.0) / 2.0).abs() < 1e-7);
        assert!(p.cross_entropy(&[3, 0]).is_err());
        assert!(p.cross_entropy(&[0]).is_err());
    }

    #[test]
    fn bias_reductions_relu_and_rows() {
        let x = t(2, 2, &[1.0, -2.0, 3.0, 4.0]);
        let b = t(1, 2, &[10.0, 20.0]);
        assert_eq!(
            x.add_row_vector(&b).unwrap().data,
            vec![11.0, 18.0, 13.0, 24.0]
        );
        assert!(x.add_row_vector(&x).is_err());
        assert_eq!(x.sum_cols().data, vec![4.0, 2.0]);
        assert_eq!(x.sum_rows().data, vec![-1.0, 7.0]);
        assert_eq!(x.sum(), 6.0);
        assert_eq!(x.relu().data, vec![1.0, 0.0, 3.0, 4.0]);
        assert_eq!(x.relu_mask().data, vec![1.0, 0.0, 1.0, 1.0]);
        let g = t(2, 2, &[5.0, 5.0, 5.0, 5.0]);
        assert_eq!(
            Tensor::relu_backward(&g, &x).unwrap().data,
            vec![5.0, 0.0, 5.0, 5.0]
        );
        assert_eq!(x.argmax_rows(), vec![0, 1]);
        assert_eq!(x.transpose().data, vec![1.0, 3.0, -2.0, 4.0]);
        assert_eq!(x.slice_rows(1, 1).unwrap().data, vec![3.0, 4.0]);
        assert!(x.slice_rows(1, 2).is_err());
        assert_eq!(x.gather_rows(&[1, 0, 1]).unwrap().rows, 3);
        let mut w = x.scale(1.0);
        w.axpy_inplace(&g, -0.5).unwrap();
        assert_eq!(w.data, vec![-1.5, -4.5, 0.5, 1.5]);
        assert_eq!(
            Tensor::one_hot(&[1, 0], 2).unwrap().data,
            vec![0.0, 1.0, 1.0, 0.0]
        );
    }

    #[test]
    fn splitmix_is_deterministic() {
        let mut a = SplitMix::new(42);
        let mut b = SplitMix::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let u = SplitMix::new(1).next_f64();
        assert!((0.0..1.0).contains(&u));
    }

    #[test]
    fn checked_len_rejects_bad_shapes() {
        assert!(checked_len(-1, 2).is_err());
        assert!(checked_len(1 << 20, 1 << 20).is_err());
        assert_eq!(checked_len(3, 4).unwrap(), 12);
    }
}
