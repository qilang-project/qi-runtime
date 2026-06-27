//! Mathematical Operations Module
//!
//! This module provides comprehensive mathematical operations including
//! basic arithmetic, advanced functions, and Chinese number formatting.

use crate::{RuntimeError, RuntimeResult};
use std::collections::HashMap;

/// Mathematical operation configuration
#[derive(Debug, Clone)]
pub struct MathConfig {
    /// Precision for floating point operations
    pub precision: u32,
    /// Use Chinese number formatting
    pub chinese_formatting: bool,
    /// Maximum value for operations
    pub max_value: Option<f64>,
    /// Minimum value for operations
    pub min_value: Option<f64>,
    /// Enable angle mode (degrees/radians)
    pub angle_mode: AngleMode,
}

/// Angle measurement mode
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AngleMode {
    /// Radians mode
    Radians,
    /// Degrees mode
    Degrees,
}

impl Default for MathConfig {
    fn default() -> Self {
        Self {
            precision: 6,
            chinese_formatting: false,
            max_value: Some(f64::MAX / 2.0),
            min_value: Some(f64::MIN / 2.0),
            angle_mode: AngleMode::Radians,
        }
    }
}

/// Mathematical operations module
#[derive(Debug)]
pub struct MathModule {
    /// Configuration
    config: MathConfig,
    /// Cached mathematical constants
    constants: HashMap<String, f64>,
    /// Chinese digit characters
    chinese_digits: Vec<char>,
    /// Chinese unit characters
    chinese_units: Vec<char>,
    /// Chinese large unit characters
    chinese_large_units: Vec<char>,
}

impl MathModule {
    /// Create new math module
    pub fn new() -> Self {
        let config = MathConfig::default();
        Self::with_config(config)
    }

    /// Create math module with configuration
    pub fn with_config(config: MathConfig) -> Self {
        let mut module = Self {
            constants: HashMap::new(),
            chinese_digits: vec!['零', '一', '二', '三', '四', '五', '六', '七', '八', '九'],
            chinese_units: vec!['十', '百', '千'],
            chinese_large_units: vec!['万', '亿', '兆'],
            config,
        };

        module.initialize_constants();
        module
    }

    /// Initialize mathematical constants
    fn initialize_constants(&mut self) {
        self.constants
            .insert("PI".to_string(), std::f64::consts::PI);
        self.constants.insert("E".to_string(), std::f64::consts::E);
        self.constants
            .insert("LN_2".to_string(), std::f64::consts::LN_2);
        self.constants
            .insert("LN_10".to_string(), std::f64::consts::LN_10);
        self.constants
            .insert("LOG2_10".to_string(), std::f64::consts::LOG2_10);
        self.constants
            .insert("LOG10_2".to_string(), std::f64::consts::LOG10_2);
        self.constants
            .insert("SQRT_2".to_string(), std::f64::consts::SQRT_2);
        self.constants
            .insert("SQRT_PI".to_string(), std::f64::consts::PI.sqrt());
    }

    /// Add two numbers
    pub fn add(&self, a: f64, b: f64) -> RuntimeResult<f64> {
        self.check_bounds(a)?;
        self.check_bounds(b)?;

        let result = a + b;
        self.check_bounds(result)?;

        Ok(self.round_to_precision(result))
    }

    /// Subtract two numbers
    pub fn subtract(&self, a: f64, b: f64) -> RuntimeResult<f64> {
        self.check_bounds(a)?;
        self.check_bounds(b)?;

        let result = a - b;
        self.check_bounds(result)?;

        Ok(self.round_to_precision(result))
    }

    /// Multiply two numbers
    pub fn multiply(&self, a: f64, b: f64) -> RuntimeResult<f64> {
        self.check_bounds(a)?;
        self.check_bounds(b)?;

        let result = a * b;
        self.check_bounds(result)?;

        Ok(self.round_to_precision(result))
    }

    /// Divide two numbers
    pub fn divide(&self, a: f64, b: f64) -> RuntimeResult<f64> {
        self.check_bounds(a)?;
        self.check_bounds(b)?;

        if b == 0.0 {
            return Err(RuntimeError::internal_error("除零错误", "除零错误"));
        }

        let result = a / b;
        self.check_bounds(result)?;

        Ok(self.round_to_precision(result))
    }

    /// Calculate modulus
    pub fn modulus(&self, a: f64, b: f64) -> RuntimeResult<f64> {
        self.check_bounds(a)?;
        self.check_bounds(b)?;

        if b == 0.0 {
            return Err(RuntimeError::internal_error("除零错误", "除零错误"));
        }

        let result = a % b;
        Ok(self.round_to_precision(result))
    }

    /// Calculate power
    pub fn power(&self, base: f64, exponent: f64) -> RuntimeResult<f64> {
        self.check_bounds(base)?;
        self.check_bounds(exponent)?;

        let result = base.powf(exponent);
        self.check_bounds(result)?;

        Ok(self.round_to_precision(result))
    }

    /// Calculate square root
    pub fn sqrt(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        if value < 0.0 {
            return Err(RuntimeError::internal_error(
                "负数平方根错误",
                "负数平方根错误",
            ));
        }

        let result = value.sqrt();
        Ok(self.round_to_precision(result))
    }

    /// Calculate cube root
    pub fn cbrt(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.cbrt();
        Ok(self.round_to_precision(result))
    }

    /// Calculate absolute value
    pub fn abs(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.abs();
        Ok(self.round_to_precision(result))
    }

    /// Calculate ceiling
    pub fn ceil(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.ceil();
        Ok(result)
    }

    /// Calculate floor
    pub fn floor(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.floor();
        Ok(result)
    }

    /// Round to nearest integer
    pub fn round(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.round();
        Ok(result)
    }

    /// Calculate natural logarithm
    pub fn ln(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        if value <= 0.0 {
            return Err(RuntimeError::internal_error(
                "对数定义域错误",
                "对数定义域错误",
            ));
        }

        let result = value.ln();
        Ok(self.round_to_precision(result))
    }

    /// Calculate base-10 logarithm
    pub fn log10(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        if value <= 0.0 {
            return Err(RuntimeError::internal_error(
                "对数定义域错误",
                "对数定义域错误",
            ));
        }

        let result = value.log10();
        Ok(self.round_to_precision(result))
    }

    /// Calculate base-2 logarithm
    pub fn log2(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        if value <= 0.0 {
            return Err(RuntimeError::internal_error(
                "对数定义域错误",
                "对数定义域错误",
            ));
        }

        let result = value.log2();
        Ok(self.round_to_precision(result))
    }

    /// Calculate exponential function (e^x)
    pub fn exp(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.exp();
        self.check_bounds(result)?;

        Ok(self.round_to_precision(result))
    }

    /// Calculate sine
    pub fn sin(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let radians = match self.config.angle_mode {
            AngleMode::Radians => value,
            AngleMode::Degrees => value.to_radians(),
        };

        let result = radians.sin();
        Ok(self.round_to_precision(result))
    }

    /// Calculate cosine
    pub fn cos(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let radians = match self.config.angle_mode {
            AngleMode::Radians => value,
            AngleMode::Degrees => value.to_radians(),
        };

        let result = radians.cos();
        Ok(self.round_to_precision(result))
    }

    /// Calculate tangent
    pub fn tan(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let radians = match self.config.angle_mode {
            AngleMode::Radians => value,
            AngleMode::Degrees => value.to_radians(),
        };

        let result = radians.tan();
        Ok(self.round_to_precision(result))
    }

    /// Calculate arcsine
    pub fn asin(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        if value < -1.0 || value > 1.0 {
            return Err(RuntimeError::internal_error(
                "反正弦定义域错误",
                "反正弦定义域错误",
            ));
        }

        let result = value.asin();
        let final_result = match self.config.angle_mode {
            AngleMode::Radians => result,
            AngleMode::Degrees => result.to_degrees(),
        };

        Ok(self.round_to_precision(final_result))
    }

    /// Calculate arccosine
    pub fn acos(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        if value < -1.0 || value > 1.0 {
            return Err(RuntimeError::internal_error(
                "反余弦定义域错误",
                "反余弦定义域错误",
            ));
        }

        let result = value.acos();
        let final_result = match self.config.angle_mode {
            AngleMode::Radians => result,
            AngleMode::Degrees => result.to_degrees(),
        };

        Ok(self.round_to_precision(final_result))
    }

    /// Calculate arctangent
    pub fn atan(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.atan();
        let final_result = match self.config.angle_mode {
            AngleMode::Radians => result,
            AngleMode::Degrees => result.to_degrees(),
        };

        Ok(self.round_to_precision(final_result))
    }

    /// Calculate hyperbolic sine
    pub fn sinh(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.sinh();
        self.check_bounds(result)?;

        Ok(self.round_to_precision(result))
    }

    /// Calculate hyperbolic cosine
    pub fn cosh(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.cosh();
        self.check_bounds(result)?;

        Ok(self.round_to_precision(result))
    }

    /// Calculate hyperbolic tangent
    pub fn tanh(&self, value: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;

        let result = value.tanh();
        Ok(self.round_to_precision(result))
    }

    /// Get mathematical constant
    pub fn constant(&self, name: &str) -> RuntimeResult<f64> {
        match self.constants.get(name) {
            Some(value) => Ok(*value),
            None => Err(RuntimeError::internal_error(
                format!("未知的数学常数: {}", name),
                "未知的数学常数".to_string(),
            )),
        }
    }

    /// Get minimum of two numbers
    pub fn min(&self, a: f64, b: f64) -> RuntimeResult<f64> {
        self.check_bounds(a)?;
        self.check_bounds(b)?;

        Ok(a.min(b))
    }

    /// Get maximum of two numbers
    pub fn max(&self, a: f64, b: f64) -> RuntimeResult<f64> {
        self.check_bounds(a)?;
        self.check_bounds(b)?;

        Ok(a.max(b))
    }

    /// Clamp value between min and max
    pub fn clamp(&self, value: f64, min: f64, max: f64) -> RuntimeResult<f64> {
        self.check_bounds(value)?;
        self.check_bounds(min)?;
        self.check_bounds(max)?;

        if min > max {
            return Err(RuntimeError::internal_error(
                "最小值不能大于最大值",
                "最小值不能大于最大值",
            ));
        }

        Ok(value.clamp(min, max))
    }

    /// Check if value is finite
    pub fn is_finite(&self, value: f64) -> bool {
        value.is_finite()
    }

    /// Check if value is infinite
    pub fn is_infinite(&self, value: f64) -> bool {
        value.is_infinite()
    }

    /// Check if value is NaN
    pub fn is_nan(&self, value: f64) -> bool {
        value.is_nan()
    }

    /// Format number as Chinese text
    pub fn format_chinese(&self, value: i64) -> RuntimeResult<String> {
        if self.config.chinese_formatting {
            Ok(self.number_to_chinese(value))
        } else {
            Ok(value.to_string())
        }
    }

    /// Convert number to Chinese text
    fn number_to_chinese(&self, mut num: i64) -> String {
        if num == 0 {
            return "零".to_string();
        }

        if num < 0 {
            return format!("负{}", self.number_to_chinese(-num));
        }

        let mut result = String::new();
        let mut large_unit_index = 0;
        let large_units = ["", "万", "亿", "兆"];

        while num > 0 {
            if num % 10000 != 0 {
                let segment_result = self.convert_four_digits(num % 10000);
                if !result.is_empty() && large_unit_index > 0 {
                    result = format!(
                        "{}{}{}",
                        segment_result, large_units[large_unit_index], result
                    );
                } else {
                    result = format!("{}{}", segment_result, result);
                }
            } else if !result.is_empty() && large_unit_index > 0 {
                result = format!("{}{}", large_units[large_unit_index], result);
            }

            num /= 10000;
            large_unit_index += 1;
        }

        result
    }

    /// Convert 4-digit number to Chinese
    fn convert_four_digits(&self, num: i64) -> String {
        if num == 0 {
            return String::new();
        }

        let mut result = String::new();
        let mut temp_num = num;
        let mut need_zero = false;

        let units = ["", "十", "百", "千"];
        for i in 0..4 {
            let digit = temp_num % 10;
            if digit != 0 {
                if need_zero {
                    result = format!("{}零", result);
                    need_zero = false;
                }
                result = format!(
                    "{}{}{}",
                    self.chinese_digits[digit as usize], units[i], result
                );
            } else if !result.is_empty() {
                need_zero = true;
            }
            temp_num /= 10;
        }

        // Handle special case for "十" (ten)
        if result.starts_with("一十") {
            result = result[3..].to_string();
        }

        result
    }

    /// Check if value is within bounds
    fn check_bounds(&self, value: f64) -> RuntimeResult<()> {
        if let Some(max) = self.config.max_value {
            if value > max {
                return Err(RuntimeError::internal_error(
                    "数值超出最大范围",
                    "数值超出最大范围",
                ));
            }
        }

        if let Some(min) = self.config.min_value {
            if value < min {
                return Err(RuntimeError::internal_error(
                    "数值超出最小范围",
                    "数值超出最小范围",
                ));
            }
        }

        Ok(())
    }

    /// Round value to configured precision
    fn round_to_precision(&self, value: f64) -> f64 {
        if self.config.precision == 0 {
            value.round()
        } else {
            let multiplier = 10_f64.powi(self.config.precision as i32);
            (value * multiplier).round() / multiplier
        }
    }

    /// Get configuration
    pub fn config(&self) -> &MathConfig {
        &self.config
    }

    /// Update configuration
    pub fn update_config(&mut self, config: MathConfig) {
        self.config = config;
    }

    /// Set angle mode
    pub fn set_angle_mode(&mut self, mode: AngleMode) {
        self.config.angle_mode = mode;
    }

    /// Set precision
    pub fn set_precision(&mut self, precision: u32) {
        self.config.precision = precision;
    }

    /// Enable/disable Chinese formatting
    pub fn set_chinese_formatting(&mut self, enabled: bool) {
        self.config.chinese_formatting = enabled;
    }
}

impl Default for MathModule {
    fn default() -> Self {
        Self::new()
    }
}

/// Mathematical operation types for runtime execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MathOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Modulo,
    Power,
    SquareRoot,
    CubeRoot,
    Absolute,
    Ceiling,
    Floor,
    Round,
    NaturalLog,
    Log10,
    Log2,
    Exponential,
    Sine,
    Cosine,
    Tangent,
    ArcSine,
    ArcCosine,
    ArcTangent,
    HyperbolicSine,
    HyperbolicCosine,
    HyperbolicTangent,
    Minimum,
    Maximum,
    Clamp,
}

impl MathOperation {
    /// Get Chinese name for the operation
    pub fn chinese_name(&self) -> &'static str {
        match self {
            MathOperation::Add => "加法",
            MathOperation::Subtract => "减法",
            MathOperation::Multiply => "乘法",
            MathOperation::Divide => "除法",
            MathOperation::Modulo => "取模",
            MathOperation::Power => "幂运算",
            MathOperation::SquareRoot => "平方根",
            MathOperation::CubeRoot => "立方根",
            MathOperation::Absolute => "绝对值",
            MathOperation::Ceiling => "向上取整",
            MathOperation::Floor => "向下取整",
            MathOperation::Round => "四舍五入",
            MathOperation::NaturalLog => "自然对数",
            MathOperation::Log10 => "常用对数",
            MathOperation::Log2 => "二进制对数",
            MathOperation::Exponential => "指数函数",
            MathOperation::Sine => "正弦",
            MathOperation::Cosine => "余弦",
            MathOperation::Tangent => "正切",
            MathOperation::ArcSine => "反正弦",
            MathOperation::ArcCosine => "反余弦",
            MathOperation::ArcTangent => "反正切",
            MathOperation::HyperbolicSine => "双曲正弦",
            MathOperation::HyperbolicCosine => "双曲余弦",
            MathOperation::HyperbolicTangent => "双曲正切",
            MathOperation::Minimum => "最小值",
            MathOperation::Maximum => "最大值",
            MathOperation::Clamp => "数值限制",
        }
    }

    /// Get operation symbol
    pub fn symbol(&self) -> &'static str {
        match self {
            MathOperation::Add => "+",
            MathOperation::Subtract => "-",
            MathOperation::Multiply => "×",
            MathOperation::Divide => "÷",
            MathOperation::Modulo => "%",
            MathOperation::Power => "^",
            MathOperation::SquareRoot => "√",
            MathOperation::CubeRoot => "∛",
            MathOperation::Absolute => "|x|",
            MathOperation::Ceiling => "⌈x⌉",
            MathOperation::Floor => "⌊x⌋",
            MathOperation::Round => "round",
            MathOperation::NaturalLog => "ln",
            MathOperation::Log10 => "log10",
            MathOperation::Log2 => "log2",
            MathOperation::Exponential => "exp",
            MathOperation::Sine => "sin",
            MathOperation::Cosine => "cos",
            MathOperation::Tangent => "tan",
            MathOperation::ArcSine => "asin",
            MathOperation::ArcCosine => "acos",
            MathOperation::ArcTangent => "atan",
            MathOperation::HyperbolicSine => "sinh",
            MathOperation::HyperbolicCosine => "cosh",
            MathOperation::HyperbolicTangent => "tanh",
            MathOperation::Minimum => "min",
            MathOperation::Maximum => "max",
            MathOperation::Clamp => "clamp",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_operations() {
        let math = MathModule::new();

        assert_eq!(math.add(2.0, 3.0).unwrap(), 5.0);
        assert_eq!(math.subtract(5.0, 3.0).unwrap(), 2.0);
        assert_eq!(math.multiply(4.0, 3.0).unwrap(), 12.0);
        assert_eq!(math.divide(12.0, 3.0).unwrap(), 4.0);
        assert_eq!(math.modulus(10.0, 3.0).unwrap(), 1.0);
    }

    #[test]
    fn test_power_operations() {
        let math = MathModule::new();

        assert_eq!(math.power(2.0, 3.0).unwrap(), 8.0);
        assert_eq!(math.sqrt(16.0).unwrap(), 4.0);
        assert_eq!(math.cbrt(27.0).unwrap(), 3.0);
    }

    #[test]
    fn test_rounding_operations() {
        let math = MathModule::new();

        assert_eq!(math.abs(-5.0).unwrap(), 5.0);
        assert_eq!(math.ceil(3.7).unwrap(), 4.0);
        assert_eq!(math.floor(3.7).unwrap(), 3.0);
        assert_eq!(math.round(3.7).unwrap(), 4.0);
        assert_eq!(math.round(3.2).unwrap(), 3.0);
    }

    #[test]
    fn test_logarithmic_operations() {
        let math = MathModule::new();

        // Use approximation for exp function due to precision rounding
        assert!(math.exp(1.0).unwrap() - std::f64::consts::E < 0.000001);
        assert!(math.ln(std::f64::consts::E).unwrap() - 1.0 < 0.000001);
        assert!(math.log10(100.0).unwrap() - 2.0 < 0.000001);
        assert!(math.log2(8.0).unwrap() - 3.0 < 0.000001);
    }

    #[test]
    fn test_trigonometric_operations() {
        let math = MathModule::new();

        // Test with radians (default)
        assert!(math.sin(0.0).unwrap() - 0.0 < 0.000001);
        assert!(math.cos(0.0).unwrap() - 1.0 < 0.000001);
        assert!(math.tan(0.0).unwrap() - 0.0 < 0.000001);

        // Test with degrees
        let mut math_deg = MathModule::new();
        math_deg.set_angle_mode(AngleMode::Degrees);
        assert!(math_deg.sin(90.0).unwrap() - 1.0 < 0.000001);
        assert!(math_deg.cos(0.0).unwrap() - 1.0 < 0.000001);
    }

    #[test]
    fn test_constants() {
        let math = MathModule::new();

        assert_eq!(math.constant("PI").unwrap(), std::f64::consts::PI);
        assert_eq!(math.constant("E").unwrap(), std::f64::consts::E);
        assert!(math.constant("UNKNOWN").is_err());
    }

    #[test]
    fn test_comparisons() {
        let math = MathModule::new();

        assert_eq!(math.min(3.0, 5.0).unwrap(), 3.0);
        assert_eq!(math.max(3.0, 5.0).unwrap(), 5.0);
        assert_eq!(math.clamp(7.0, 5.0, 10.0).unwrap(), 7.0);
        assert_eq!(math.clamp(3.0, 5.0, 10.0).unwrap(), 5.0);
        assert_eq!(math.clamp(12.0, 5.0, 10.0).unwrap(), 10.0);
    }

    #[test]
    fn test_chinese_formatting() {
        let mut math = MathModule::new();
        math.set_chinese_formatting(true);

        assert_eq!(math.format_chinese(0).unwrap(), "零");
        assert_eq!(math.format_chinese(5).unwrap(), "五");
        assert_eq!(math.format_chinese(10).unwrap(), "十");
        assert_eq!(math.format_chinese(15).unwrap(), "十五");
        assert_eq!(math.format_chinese(100).unwrap(), "一百");
        assert_eq!(math.format_chinese(123).unwrap(), "一百二十三");
        assert_eq!(math.format_chinese(-5).unwrap(), "负五");
    }

    #[test]
    fn test_error_handling() {
        let math = MathModule::new();

        // Division by zero
        assert!(math.divide(5.0, 0.0).is_err());
        assert!(math.modulus(5.0, 0.0).is_err());

        // Square root of negative number
        assert!(math.sqrt(-1.0).is_err());

        // Logarithm of non-positive number
        assert!(math.ln(0.0).is_err());
        assert!(math.ln(-1.0).is_err());

        // Arc sine/cosine out of range
        assert!(math.asin(2.0).is_err());
        assert!(math.acos(2.0).is_err());
    }

    #[test]
    fn test_precision() {
        let mut math = MathModule::new();
        math.set_precision(2);

        let result = math.divide(1.0, 3.0).unwrap();
        // Result should be close to 0.33 (rounded to 2 decimal places)
        assert!(result - 0.33 < 0.001);
        // Result should be significantly different from full precision 0.333333
        let diff_from_full_precision = (result - 0.333333).abs();
        assert!(diff_from_full_precision > 0.001);
    }
}
