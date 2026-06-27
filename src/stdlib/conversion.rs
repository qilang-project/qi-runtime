//! Type Conversion Module
//!
//! This module provides comprehensive type conversion operations including
//! string to number conversions, formatting, and Chinese language support.

use crate::{RuntimeError, RuntimeResult};
use std::collections::HashMap;

/// Conversion configuration
#[derive(Debug, Clone)]
pub struct ConversionConfig {
    /// Use Chinese number formatting
    pub chinese_formatting: bool,
    /// Default number base for string conversions
    pub default_base: u32,
    /// Use strict parsing (return error for invalid input)
    pub strict_parsing: bool,
    /// Trim whitespace during conversions
    pub trim_whitespace: bool,
    /// Locale for number formatting
    pub locale: String,
}

impl Default for ConversionConfig {
    fn default() -> Self {
        Self {
            chinese_formatting: false,
            default_base: 10,
            strict_parsing: true,
            trim_whitespace: true,
            locale: "zh-CN".to_string(),
        }
    }
}

/// Type conversion module
#[derive(Debug)]
pub struct ConversionModule {
    /// Configuration
    config: ConversionConfig,
    /// Cache for common conversions
    conversion_cache: HashMap<String, String>,
    /// Chinese number mappings
    chinese_numbers: HashMap<char, i64>,
    /// Chinese unit mappings
    chinese_units: HashMap<char, i64>,
}

impl ConversionModule {
    /// Create new conversion module
    pub fn new() -> Self {
        Self::with_config(ConversionConfig::default())
    }

    /// Create conversion module with configuration
    pub fn with_config(config: ConversionConfig) -> Self {
        let mut module = Self {
            conversion_cache: HashMap::new(),
            chinese_numbers: HashMap::new(),
            chinese_units: HashMap::new(),
            config,
        };

        module.initialize_chinese_mappings();
        module
    }

    /// Initialize Chinese number mappings
    fn initialize_chinese_mappings(&mut self) {
        // Numbers
        self.chinese_numbers.insert('零', 0);
        self.chinese_numbers.insert('一', 1);
        self.chinese_numbers.insert('二', 2);
        self.chinese_numbers.insert('三', 3);
        self.chinese_numbers.insert('四', 4);
        self.chinese_numbers.insert('五', 5);
        self.chinese_numbers.insert('六', 6);
        self.chinese_numbers.insert('七', 7);
        self.chinese_numbers.insert('八', 8);
        self.chinese_numbers.insert('九', 9);
        self.chinese_numbers.insert('两', 2);

        // Units
        self.chinese_units.insert('十', 10);
        self.chinese_units.insert('百', 100);
        self.chinese_units.insert('千', 1000);
        self.chinese_units.insert('万', 10000);
        self.chinese_units.insert('亿', 100000000);
    }

    /// Convert string to integer
    pub fn string_to_int(&self, input: &str) -> RuntimeResult<i64> {
        let processed_input = if self.config.trim_whitespace {
            input.trim()
        } else {
            input
        };

        if processed_input.is_empty() {
            if self.config.strict_parsing {
                return Err(RuntimeError::conversion_error(
                    "空字符串无法转换为整数".to_string(),
                    "空字符串无法转换为整数".to_string(),
                ));
            } else {
                return Ok(0);
            }
        }

        // Try Chinese number conversion first if enabled
        if self.config.chinese_formatting {
            if let Ok(chinese_result) = self.chinese_to_int(processed_input) {
                return Ok(chinese_result);
            }
        }

        // Standard numeric conversion
        match processed_input.parse::<i64>() {
            Ok(value) => Ok(value),
            Err(_) if !self.config.strict_parsing => Ok(0),
            Err(e) => Err(RuntimeError::conversion_error(
                format!("无法将字符串 '{}' 转换为整数: {}", processed_input, e),
                "字符串转整数失败".to_string(),
            )),
        }
    }

    /// Convert integer to string
    pub fn int_to_string(&self, value: i64) -> RuntimeResult<String> {
        if self.config.chinese_formatting {
            Ok(self.int_to_chinese(value))
        } else {
            Ok(value.to_string())
        }
    }

    /// Convert string to float
    pub fn string_to_float(&self, input: &str) -> RuntimeResult<f64> {
        let processed_input = if self.config.trim_whitespace {
            input.trim()
        } else {
            input
        };

        if processed_input.is_empty() {
            if self.config.strict_parsing {
                return Err(RuntimeError::conversion_error(
                    "空字符串无法转换为浮点数".to_string(),
                    "空字符串无法转换为浮点数".to_string(),
                ));
            } else {
                return Ok(0.0);
            }
        }

        // Try Chinese number conversion first if enabled
        if self.config.chinese_formatting {
            if let Ok(chinese_result) = self.chinese_to_float(processed_input) {
                return Ok(chinese_result);
            }
        }

        // Standard numeric conversion
        match processed_input.parse::<f64>() {
            Ok(value) => Ok(value),
            Err(_) if !self.config.strict_parsing => Ok(0.0),
            Err(e) => Err(RuntimeError::conversion_error(
                format!("无法将字符串 '{}' 转换为浮点数: {}", processed_input, e),
                "字符串转浮点数失败".to_string(),
            )),
        }
    }

    /// Convert float to string
    pub fn float_to_string(&self, value: f64) -> RuntimeResult<String> {
        if self.config.chinese_formatting {
            Ok(self.float_to_chinese(value))
        } else {
            Ok(value.to_string())
        }
    }

    /// Convert string to boolean
    pub fn string_to_bool(&self, input: &str) -> RuntimeResult<bool> {
        let processed_input = if self.config.trim_whitespace {
            input.trim().to_lowercase()
        } else {
            input.to_lowercase()
        };

        match processed_input.as_str() {
            "true" | "真" | "是" | "1" | "yes" | "on" => Ok(true),
            "false" | "假" | "否" | "0" | "no" | "off" => Ok(false),
            _ if !self.config.strict_parsing => Ok(processed_input != ""),
            _ => Err(RuntimeError::conversion_error(
                format!("无法将字符串 '{}' 转换为布尔值", input),
                "字符串转布尔值失败".to_string(),
            )),
        }
    }

    /// Convert boolean to string
    pub fn bool_to_string(&self, value: bool) -> RuntimeResult<String> {
        if self.config.chinese_formatting {
            Ok(if value { "真" } else { "假" }.to_string())
        } else {
            Ok(value.to_string())
        }
    }

    /// Convert string to integer with specific base
    pub fn string_to_int_with_base(&self, input: &str, base: u32) -> RuntimeResult<i64> {
        let processed_input = if self.config.trim_whitespace {
            input.trim()
        } else {
            input
        };

        if processed_input.is_empty() {
            if self.config.strict_parsing {
                return Err(RuntimeError::conversion_error(
                    "空字符串无法转换为整数".to_string(),
                    "空字符串无法转换为整数".to_string(),
                ));
            } else {
                return Ok(0);
            }
        }

        match i64::from_str_radix(processed_input, base) {
            Ok(value) => Ok(value),
            Err(_) if !self.config.strict_parsing => Ok(0),
            Err(e) => Err(RuntimeError::conversion_error(
                format!(
                    "无法将字符串 '{}' 以 {} 进制转换为整数: {}",
                    processed_input, base, e
                ),
                "进制转换失败".to_string(),
            )),
        }
    }

    /// Convert integer to string with specific base
    pub fn int_to_string_with_base(&self, value: i64, base: u32) -> RuntimeResult<String> {
        if base == 10 {
            return self.int_to_string(value);
        }

        if base < 2 || base > 36 {
            return Err(RuntimeError::conversion_error(
                format!("无效的进制: {}, 支持范围 2-36", base),
                "无效进制".to_string(),
            ));
        }

        if value == 0 {
            return Ok("0".to_string());
        }

        let mut result = String::new();
        let mut num = if value < 0 { -value } else { value };
        let chars = "0123456789abcdefghijklmnopqrstuvwxyz";

        while num > 0 {
            let digit = (num % base as i64) as usize;
            result.insert(0, chars.chars().nth(digit).unwrap());
            num /= base as i64;
        }

        if value < 0 {
            result.insert(0, '-');
        }

        Ok(result)
    }

    /// Convert Chinese number to integer
    fn chinese_to_int(&self, input: &str) -> RuntimeResult<i64> {
        let mut chars: Vec<char> = input.chars().collect();
        let mut is_negative = false;

        // Check for negative sign
        if !chars.is_empty() && chars[0] == '负' {
            is_negative = true;
            chars.remove(0);
        }

        let mut result = 0i64;
        let mut temp = 0i64;
        let mut last_unit = 1i64;

        for ch in chars {
            if let Some(&num) = self.chinese_numbers.get(&ch) {
                temp = num;
            } else if let Some(&unit) = self.chinese_units.get(&ch) {
                if unit > last_unit {
                    if temp == 0 {
                        temp = 1;
                    }
                    result += temp * unit;
                    temp = 0;
                    last_unit = unit;
                } else {
                    if temp == 0 {
                        temp = 1;
                    }
                    result += temp * unit;
                    temp = 0;
                }
            } else {
                return Err(RuntimeError::conversion_error(
                    format!("无法识别的中文字符: '{}'", ch),
                    "无法识别的中文字符".to_string(),
                ));
            }
        }

        result += temp;
        if is_negative {
            Ok(-result)
        } else {
            Ok(result)
        }
    }

    /// Convert Chinese number to float
    fn chinese_to_float(&self, input: &str) -> RuntimeResult<f64> {
        // For simplicity, convert to integer first then to float
        let int_value = self.chinese_to_int(input)?;
        Ok(int_value as f64)
    }

    /// Convert integer to Chinese
    fn int_to_chinese(&self, mut value: i64) -> String {
        if value == 0 {
            return "零".to_string();
        }

        if value < 0 {
            return format!("负{}", self.int_to_chinese(-value));
        }

        let mut result = String::new();
        let units = ["", "十", "百", "千", "万", "十", "百", "千", "亿"];

        let mut position = 0;
        while value > 0 {
            let digit = value % 10;
            if digit != 0 {
                let digit_char = match digit {
                    1 if position == 1 && result.is_empty() => "", // 特殊处理"十"
                    _ => match digit {
                        1 => "一",
                        2 => "二",
                        3 => "三",
                        4 => "四",
                        5 => "五",
                        6 => "六",
                        7 => "七",
                        8 => "八",
                        9 => "九",
                        _ => "零",
                    },
                };
                result = format!("{}{}{}", digit_char, units[position], result);
            } else if !result.is_empty() && !result.starts_with("零") {
                result = format!("零{}", result);
            }
            value /= 10;
            position += 1;
        }

        // Clean up redundant characters
        result = result.replace("零+", "零");
        if result.ends_with("零") {
            result.pop();
        }

        result
    }

    /// Convert float to Chinese
    fn float_to_chinese(&self, value: f64) -> String {
        if value.is_nan() {
            return "非数字".to_string();
        }
        if value.is_infinite() {
            return if value > 0.0 {
                "正无穷"
            } else {
                "负无穷"
            }
            .to_string();
        }

        let int_part = value as i64;
        let float_part = value - int_part as f64;

        let mut result = self.int_to_chinese(int_part);

        if float_part > 0.0 {
            result.push_str("点");
            let mut temp_float = float_part;
            for _ in 0..6 {
                // Limit to 6 decimal places
                temp_float *= 10.0;
                let digit = temp_float as i64;
                if digit > 0 {
                    result.push_str(&self.int_to_chinese(digit));
                }
                temp_float -= digit as f64;
            }
        }

        result
    }

    /// Parse hexadecimal string
    pub fn parse_hex(&self, input: &str) -> RuntimeResult<i64> {
        let processed = input.trim_start_matches("0x").trim_start_matches("0X");
        self.string_to_int_with_base(processed, 16)
    }

    /// Parse octal string
    pub fn parse_octal(&self, input: &str) -> RuntimeResult<i64> {
        let processed = input.trim_start_matches("0o").trim_start_matches("0O");
        self.string_to_int_with_base(processed, 8)
    }

    /// Parse binary string
    pub fn parse_binary(&self, input: &str) -> RuntimeResult<i64> {
        let processed = input.trim_start_matches("0b").trim_start_matches("0B");
        self.string_to_int_with_base(processed, 2)
    }

    /// Format as hexadecimal
    pub fn format_hex(&self, value: i64) -> RuntimeResult<String> {
        Ok(format!("0x{:x}", value))
    }

    /// Format as octal
    pub fn format_octal(&self, value: i64) -> RuntimeResult<String> {
        Ok(format!("0o{:o}", value))
    }

    /// Format as binary
    pub fn format_binary(&self, value: i64) -> RuntimeResult<String> {
        Ok(format!("0b{:b}", value))
    }

    /// Convert any type to string
    pub fn to_string<T: std::fmt::Display>(&self, value: T) -> String {
        value.to_string()
    }

    /// Try to parse any type from string
    pub fn from_string<T>(&self, input: &str) -> RuntimeResult<T>
    where
        T: std::str::FromStr,
        T::Err: std::fmt::Display,
    {
        let processed_input = if self.config.trim_whitespace {
            input.trim()
        } else {
            input
        };

        processed_input.parse::<T>().map_err(|e| {
            RuntimeError::conversion_error(
                format!("无法将字符串 '{}' 转换为目标类型: {}", processed_input, e),
                "类型转换失败".to_string(),
            )
        })
    }

    /// Get configuration
    pub fn config(&self) -> &ConversionConfig {
        &self.config
    }

    /// Update configuration
    pub fn update_config(&mut self, config: ConversionConfig) {
        self.config = config;
    }

    /// Set Chinese formatting
    pub fn set_chinese_formatting(&mut self, enabled: bool) {
        self.config.chinese_formatting = enabled;
    }

    /// Set strict parsing
    pub fn set_strict_parsing(&mut self, strict: bool) {
        self.config.strict_parsing = strict;
    }

    /// Clear conversion cache
    pub fn clear_cache(&mut self) {
        self.conversion_cache.clear();
    }
}

impl Default for ConversionModule {
    fn default() -> Self {
        Self::new()
    }
}

/// Type conversion operations for runtime execution
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeConversion {
    StringToInteger,
    StringToFloat,
    IntegerToString,
    FloatToString,
    StringToBoolean,
    BooleanToString,
    StringToBytes,
    BytesToString,
    HexToString,
    StringToHex,
    Base64ToString,
    StringToBase64,
    JsonToString,
    StringToJson,
    ChineseNumberToInteger,
    IntegerToChineseNumber,
}

impl TypeConversion {
    /// Get Chinese name for the conversion type
    pub fn chinese_name(&self) -> &'static str {
        match self {
            TypeConversion::StringToInteger => "字符串转整数",
            TypeConversion::StringToFloat => "字符串转浮点数",
            TypeConversion::IntegerToString => "整数转字符串",
            TypeConversion::FloatToString => "浮点数转字符串",
            TypeConversion::StringToBoolean => "字符串转布尔值",
            TypeConversion::BooleanToString => "布尔值转字符串",
            TypeConversion::StringToBytes => "字符串转字节数组",
            TypeConversion::BytesToString => "字节数组转字符串",
            TypeConversion::HexToString => "十六进制转字符串",
            TypeConversion::StringToHex => "字符串转十六进制",
            TypeConversion::Base64ToString => "Base64转字符串",
            TypeConversion::StringToBase64 => "字符串转Base64",
            TypeConversion::JsonToString => "JSON转字符串",
            TypeConversion::StringToJson => "字符串转JSON",
            TypeConversion::ChineseNumberToInteger => "中文数字转整数",
            TypeConversion::IntegerToChineseNumber => "整数转中文数字",
        }
    }

    /// Get operation symbol
    pub fn symbol(&self) -> &'static str {
        match self {
            TypeConversion::StringToInteger => "str->int",
            TypeConversion::StringToFloat => "str->float",
            TypeConversion::IntegerToString => "int->str",
            TypeConversion::FloatToString => "float->str",
            TypeConversion::StringToBoolean => "str->bool",
            TypeConversion::BooleanToString => "bool->str",
            TypeConversion::StringToBytes => "str->bytes",
            TypeConversion::BytesToString => "bytes->str",
            TypeConversion::HexToString => "hex->str",
            TypeConversion::StringToHex => "str->hex",
            TypeConversion::Base64ToString => "base64->str",
            TypeConversion::StringToBase64 => "str->base64",
            TypeConversion::JsonToString => "json->str",
            TypeConversion::StringToJson => "str->json",
            TypeConversion::ChineseNumberToInteger => "zh->int",
            TypeConversion::IntegerToChineseNumber => "int->zh",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_conversions() {
        let conversion = ConversionModule::new();

        // String to int
        assert_eq!(conversion.string_to_int("123").unwrap(), 123);
        assert_eq!(conversion.string_to_int("-456").unwrap(), -456);

        // Int to string
        assert_eq!(conversion.int_to_string(123).unwrap(), "123");
        assert_eq!(conversion.int_to_string(-456).unwrap(), "-456");

        // String to float
        assert_eq!(conversion.string_to_float("123.45").unwrap(), 123.45);
        assert_eq!(conversion.string_to_float("-67.89").unwrap(), -67.89);

        // Float to string
        assert_eq!(conversion.float_to_string(123.45).unwrap(), "123.45");

        // String to bool
        assert_eq!(conversion.string_to_bool("true").unwrap(), true);
        assert_eq!(conversion.string_to_bool("false").unwrap(), false);
        assert_eq!(conversion.string_to_bool("真").unwrap(), true);
        assert_eq!(conversion.string_to_bool("假").unwrap(), false);

        // Bool to string
        assert_eq!(conversion.bool_to_string(true).unwrap(), "true");
        assert_eq!(conversion.bool_to_string(false).unwrap(), "false");
    }

    #[test]
    fn test_chinese_formatting() {
        let mut conversion = ConversionModule::new();
        conversion.set_chinese_formatting(true);

        // Chinese to int
        assert_eq!(conversion.string_to_int("一百二十三").unwrap(), 123);
        assert_eq!(conversion.string_to_int("一千零五").unwrap(), 1005);
        assert_eq!(conversion.string_to_int("负一百").unwrap(), -100);

        // Int to Chinese
        assert_eq!(conversion.int_to_string(123).unwrap(), "一百二十三");
        assert_eq!(conversion.int_to_string(1005).unwrap(), "一千零五");
        assert_eq!(conversion.int_to_string(-100).unwrap(), "负一百");

        // Bool to Chinese
        assert_eq!(conversion.bool_to_string(true).unwrap(), "真");
        assert_eq!(conversion.bool_to_string(false).unwrap(), "假");
    }

    #[test]
    fn test_base_conversions() {
        let conversion = ConversionModule::new();

        // Hex
        assert_eq!(conversion.string_to_int_with_base("FF", 16).unwrap(), 255);
        assert_eq!(conversion.parse_hex("0xFF").unwrap(), 255);
        assert_eq!(conversion.format_hex(255).unwrap(), "0xff");

        // Octal
        assert_eq!(conversion.string_to_int_with_base("77", 8).unwrap(), 63);
        assert_eq!(conversion.parse_octal("0o77").unwrap(), 63);
        assert_eq!(conversion.format_octal(63).unwrap(), "0o77");

        // Binary
        assert_eq!(conversion.string_to_int_with_base("1010", 2).unwrap(), 10);
        assert_eq!(conversion.parse_binary("0b1010").unwrap(), 10);
        assert_eq!(conversion.format_binary(10).unwrap(), "0b1010");
    }

    #[test]
    fn test_strict_parsing() {
        let mut conversion = ConversionModule::new();

        // Strict parsing enabled (default)
        assert!(conversion.string_to_int("abc").is_err());
        assert!(conversion.string_to_float("xyz").is_err());

        // Strict parsing disabled
        conversion.set_strict_parsing(false);
        assert_eq!(conversion.string_to_int("abc").unwrap(), 0);
        assert_eq!(conversion.string_to_float("xyz").unwrap(), 0.0);
    }

    #[test]
    fn test_edge_cases() {
        let conversion = ConversionModule::new();

        // Empty string
        assert!(conversion.string_to_int("").is_err());
        assert!(conversion.string_to_float("").is_err());

        // Large numbers
        let large_int = conversion.string_to_int("9223372036854775807").unwrap();
        assert_eq!(large_int, i64::MAX);

        // Special boolean values
        assert_eq!(conversion.string_to_bool("1").unwrap(), true);
        assert_eq!(conversion.string_to_bool("0").unwrap(), false);
        assert_eq!(conversion.string_to_bool("yes").unwrap(), true);
        assert_eq!(conversion.string_to_bool("no").unwrap(), false);
        assert_eq!(conversion.string_to_bool("on").unwrap(), true);
        assert_eq!(conversion.string_to_bool("off").unwrap(), false);
        assert_eq!(conversion.string_to_bool("是").unwrap(), true);
        assert_eq!(conversion.string_to_bool("否").unwrap(), false);
    }

    #[test]
    fn test_generic_conversions() {
        let conversion = ConversionModule::new();

        // Generic from_string
        let parsed_int: i32 = conversion.from_string("42").unwrap();
        assert_eq!(parsed_int, 42);

        let parsed_float: f64 = conversion.from_string("3.14").unwrap();
        assert_eq!(parsed_float, 3.14);

        // Generic to_string
        let stringified = conversion.to_string(42);
        assert_eq!(stringified, "42");
    }

    #[test]
    fn test_configuration() {
        let mut conversion = ConversionModule::new();

        // Test default config
        assert!(!conversion.config().chinese_formatting);
        assert!(conversion.config().strict_parsing);

        // Update configuration
        conversion.set_chinese_formatting(true);
        conversion.set_strict_parsing(false);

        assert!(conversion.config().chinese_formatting);
        assert!(!conversion.config().strict_parsing);
    }
}
