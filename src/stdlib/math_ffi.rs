//! 数学 模块 FFI —— 浮点标量数学函数。
//!
//! math.rs 早有完整实现（MathModule），但只是 Rust 结构体方法、从未导出
//! C ABI，qilang 代码够不着。这里补一层零开销的 extern "C" 包装：直接用
//! f64 内置方法（底层 libm），指数/对数/开方/三角/双曲全齐。
//!
//! 全部 (f64,..) -> f64，无状态、线程安全、不分配。module_registry 注册为
//! 「数学」模块（中文名 → 这些符号）。

#![allow(non_snake_case)]

/// e^x —— 神经网络 sigmoid/softmax 的基石
#[no_mangle]
pub extern "C" fn qi_math_exp(x: f64) -> f64 {
    x.exp()
}

/// 自然对数 ln(x)
#[no_mangle]
pub extern "C" fn qi_math_ln(x: f64) -> f64 {
    x.ln()
}

/// 以 10 为底的对数
#[no_mangle]
pub extern "C" fn qi_math_log10(x: f64) -> f64 {
    x.log10()
}

/// 以 2 为底的对数
#[no_mangle]
pub extern "C" fn qi_math_log2(x: f64) -> f64 {
    x.log2()
}

/// 平方根
#[no_mangle]
pub extern "C" fn qi_math_sqrt(x: f64) -> f64 {
    x.sqrt()
}

/// 立方根
#[no_mangle]
pub extern "C" fn qi_math_cbrt(x: f64) -> f64 {
    x.cbrt()
}

/// 幂 base^exp（支持小数指数）
#[no_mangle]
pub extern "C" fn qi_math_pow(base: f64, exp: f64) -> f64 {
    base.powf(exp)
}

/// 绝对值
#[no_mangle]
pub extern "C" fn qi_math_abs(x: f64) -> f64 {
    x.abs()
}

/// 向上取整
#[no_mangle]
pub extern "C" fn qi_math_ceil(x: f64) -> f64 {
    x.ceil()
}

/// 向下取整
#[no_mangle]
pub extern "C" fn qi_math_floor(x: f64) -> f64 {
    x.floor()
}

/// 四舍五入
#[no_mangle]
pub extern "C" fn qi_math_round(x: f64) -> f64 {
    x.round()
}

/// 正弦（弧度）
#[no_mangle]
pub extern "C" fn qi_math_sin(x: f64) -> f64 {
    x.sin()
}

/// 余弦（弧度）
#[no_mangle]
pub extern "C" fn qi_math_cos(x: f64) -> f64 {
    x.cos()
}

/// 正切（弧度）
#[no_mangle]
pub extern "C" fn qi_math_tan(x: f64) -> f64 {
    x.tan()
}

/// 反正弦，返回弧度
#[no_mangle]
pub extern "C" fn qi_math_asin(x: f64) -> f64 {
    x.asin()
}

/// 反余弦，返回弧度
#[no_mangle]
pub extern "C" fn qi_math_acos(x: f64) -> f64 {
    x.acos()
}

/// 反正切，返回弧度
#[no_mangle]
pub extern "C" fn qi_math_atan(x: f64) -> f64 {
    x.atan()
}

/// 双曲正切 —— 神经网络常用激活函数
#[no_mangle]
pub extern "C" fn qi_math_tanh(x: f64) -> f64 {
    x.tanh()
}

/// 两数较大者
#[no_mangle]
pub extern "C" fn qi_math_max(a: f64, b: f64) -> f64 {
    a.max(b)
}

/// 两数较小者
#[no_mangle]
pub extern "C" fn qi_math_min(a: f64, b: f64) -> f64 {
    a.min(b)
}

/// 圆周率 π
#[no_mangle]
pub extern "C" fn qi_math_pi() -> f64 {
    std::f64::consts::PI
}

/// 自然常数 e
#[no_mangle]
pub extern "C" fn qi_math_e() -> f64 {
    std::f64::consts::E
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 指数对数互逆() {
        assert!((qi_math_exp(1.0) - std::f64::consts::E).abs() < 1e-12);
        assert!((qi_math_ln(qi_math_exp(3.0)) - 3.0).abs() < 1e-12);
    }

    #[test]
    fn 开方与幂() {
        assert!((qi_math_sqrt(9.0) - 3.0).abs() < 1e-12);
        assert!((qi_math_pow(2.0, 10.0) - 1024.0).abs() < 1e-9);
    }

    #[test]
    fn 三角与取整() {
        assert!((qi_math_sin(qi_math_pi() / 2.0) - 1.0).abs() < 1e-12);
        assert_eq!(qi_math_floor(3.7), 3.0);
        assert_eq!(qi_math_ceil(3.2), 4.0);
        assert!((qi_math_tanh(0.0)).abs() < 1e-12);
    }
}
