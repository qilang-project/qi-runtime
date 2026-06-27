//! String Operations Module
//!
//! Simple string operations for the Qi runtime.

use crate::{RuntimeError, RuntimeResult};
use std::sync::Arc;
use std::sync::Mutex;

/// String encoding types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StringEncoding {
    /// UTF-8 encoding (default)
    Utf8,
    /// UTF-16 encoding
    Utf16,
    /// UTF-32 encoding
    Utf32,
    /// ASCII encoding
    Ascii,
}

impl Default for StringEncoding {
    fn default() -> Self {
        Self::Utf8
    }
}

/// Text direction for rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextDirection {
    /// Left to right
    LeftToRight,
    /// Right to left
    RightToLeft,
}

impl Default for TextDirection {
    fn default() -> Self {
        Self::LeftToRight
    }
}

/// String processing configuration
#[derive(Debug, Clone)]
pub struct StringConfig {
    /// Default encoding
    pub default_encoding: StringEncoding,
    /// Text direction
    pub text_direction: TextDirection,
    /// Case sensitivity
    pub case_sensitive: bool,
    /// Locale for operations
    pub locale: String,
    /// Maximum string length
    pub max_string_length: usize,
}

impl Default for StringConfig {
    fn default() -> Self {
        Self {
            default_encoding: StringEncoding::Utf8,
            text_direction: TextDirection::LeftToRight,
            case_sensitive: true,
            locale: "zh-CN".to_string(),
            max_string_length: 10 * 1024 * 1024, // 10MB
        }
    }
}

/// String operation statistics
#[derive(Debug, Clone, Default)]
pub struct StringStats {
    /// Total string operations performed
    pub total_operations: u64,
    /// Concatenations performed
    pub concatenations: u64,
    /// Substrings extracted
    pub substrings: u64,
    /// String comparisons
    pub comparisons: u64,
    /// Case conversions
    pub case_conversions: u64,
    /// Total characters processed
    pub total_characters: u64,
    /// Total bytes processed
    pub total_bytes: u64,
}

/// String interface for runtime string operations
#[derive(Debug)]
pub struct StringInterface {
    /// Configuration
    config: Arc<Mutex<StringConfig>>,
    /// String statistics
    stats: Arc<Mutex<StringStats>>,
}

impl StringInterface {
    /// Create new string interface
    pub fn new() -> Self {
        let config = StringConfig::default();

        Self {
            config: Arc::new(Mutex::new(config)),
            stats: Arc::new(Mutex::new(StringStats::default())),
        }
    }

    /// Create string interface with custom configuration
    pub fn with_config(config: StringConfig) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            stats: Arc::new(Mutex::new(StringStats::default())),
        }
    }

    /// Concatenate strings
    pub fn concat(&self, strings: &[String]) -> RuntimeResult<String> {
        self.check_string_lengths(strings)?;

        let mut result = String::new();
        let total_chars = strings.iter().map(|s| s.chars().count()).sum();

        for string in strings {
            result.push_str(string);
        }

        self.record_operation("concat");
        self.record_characters_processed(total_chars);
        self.record_bytes_processed(result.len());

        Ok(result)
    }

    /// Extract substring
    pub fn substring(&self, text: &str, start: usize, length: usize) -> RuntimeResult<String> {
        if start >= text.len() {
            return Err(RuntimeError::validation_error(
                "字符串操作错误",
                &format!("起始位置 {} 超出字符串长度 {}", start, text.len()),
            ));
        }

        let chars: Vec<char> = text.chars().collect();
        if start >= chars.len() {
            return Err(RuntimeError::validation_error(
                "字符串操作错误",
                &format!("起始位置 {} 超出字符长度 {}", start, chars.len()),
            ));
        }

        let end = std::cmp::min(start + length, chars.len());
        let substring_chars: String = chars[start..end].iter().collect();

        self.record_operation("substring");
        self.record_characters_processed(end - start);
        self.record_bytes_processed(substring_chars.len());

        Ok(substring_chars)
    }

    /// Get string length (characters)
    pub fn length(&self, text: &str) -> RuntimeResult<usize> {
        let length = text.chars().count();

        self.record_operation("length");
        self.record_characters_processed(length);
        self.record_bytes_processed(text.len());

        Ok(length)
    }

    /// Compare two strings
    pub fn compare(&self, a: &str, b: &str) -> RuntimeResult<i32> {
        let config = self.config.lock().unwrap();
        let result = if config.case_sensitive {
            a.cmp(b)
        } else {
            a.to_lowercase().cmp(&b.to_lowercase())
        };

        let order = match result {
            std::cmp::Ordering::Less => -1,
            std::cmp::Ordering::Equal => 0,
            std::cmp::Ordering::Greater => 1,
        };

        self.record_operation("compare");
        self.record_characters_processed(a.chars().count() + b.chars().count());
        self.record_bytes_processed(a.len() + b.len());

        Ok(order)
    }

    /// Convert to uppercase
    pub fn to_uppercase(&self, text: &str) -> RuntimeResult<String> {
        let result = text.to_uppercase();

        self.record_operation("to_uppercase");
        self.record_case_conversion();
        self.record_characters_processed(result.chars().count());
        self.record_bytes_processed(result.len());

        Ok(result)
    }

    /// Convert to lowercase
    pub fn to_lowercase(&self, text: &str) -> RuntimeResult<String> {
        let result = text.to_lowercase();

        self.record_operation("to_lowercase");
        self.record_case_conversion();
        self.record_characters_processed(result.chars().count());
        self.record_bytes_processed(result.len());

        Ok(result)
    }

    /// Initialize the string interface
    pub fn initialize(&self) -> RuntimeResult<()> {
        // Reset statistics
        let mut stats = self.stats.lock().unwrap();
        *stats = StringStats::default();
        Ok(())
    }

    /// Get configuration
    pub fn get_config(&self) -> RuntimeResult<StringConfig> {
        let config = self.config.lock().unwrap();
        Ok(config.clone())
    }

    /// Set case sensitivity
    pub fn set_case_sensitive(&self, enabled: bool) -> RuntimeResult<()> {
        self.config.lock().unwrap().case_sensitive = enabled;
        Ok(())
    }

    /// Private helper methods

    fn check_string_lengths(&self, strings: &[String]) -> RuntimeResult<()> {
        let config = self.config.lock().unwrap();
        let total_length: usize = strings.iter().map(|s| s.len()).sum();

        if total_length > config.max_string_length {
            return Err(RuntimeError::validation_error(
                "字符串长度错误",
                &format!(
                    "字符串总长度 {} 超过最大限制 {}",
                    total_length, config.max_string_length
                ),
            ));
        }

        Ok(())
    }

    fn record_operation(&self, operation: &str) {
        let mut stats = self.stats.lock().unwrap();
        stats.total_operations += 1;

        match operation {
            "concat" => stats.concatenations += 1,
            "substring" => stats.substrings += 1,
            "compare" => stats.comparisons += 1,
            _ => {}
        }
    }

    fn record_characters_processed(&self, count: usize) {
        let mut stats = self.stats.lock().unwrap();
        stats.total_characters += count as u64;
    }

    fn record_bytes_processed(&self, count: usize) {
        let mut stats = self.stats.lock().unwrap();
        stats.total_bytes += count as u64;
    }

    fn record_case_conversion(&self) {
        let mut stats = self.stats.lock().unwrap();
        stats.case_conversions += 1;
    }
}

impl Default for StringInterface {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_interface() {
        let interface = StringInterface::new();
        assert!(interface.initialize().is_ok());

        let result = interface.concat(&["Hello".to_string(), " ".to_string(), "World".to_string()]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "Hello World");

        let length = interface.length("你好");
        assert!(length.is_ok());
        assert_eq!(length.unwrap(), 2);

        let upper = interface.to_uppercase("hello");
        assert!(upper.is_ok());
        assert_eq!(upper.unwrap(), "HELLO");
    }

    #[test]
    fn test_string_config() {
        let config = StringConfig::default();
        assert!(matches!(config.default_encoding, StringEncoding::Utf8));
        assert!(matches!(config.text_direction, TextDirection::LeftToRight));
        assert!(config.case_sensitive);
        assert_eq!(config.locale, "zh-CN");
    }
}
