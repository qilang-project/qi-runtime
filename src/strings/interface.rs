//! String Interface Module
//!
//! This module provides a unified interface for all string operations
//! including Unicode support, Chinese text processing, and encoding conversions.

use std::sync::Arc;
use std::sync::Mutex;
use std::collections::HashMap;

use crate::{RuntimeResult, RuntimeError};
use super::{StringEncoding, TextDirection, StringNormalization};

/// Unified string interface that provides access to all string functionality
#[derive(Debug)]
pub struct StringInterface {
    /// Configuration
    config: Arc<Mutex<StringConfig>>,
    /// String statistics
    stats: Arc<Mutex<StringStats>>,
    /// Cache for common operations
    cache: Arc<Mutex<StringCache>>,
}

/// String processing configuration
#[derive(Debug, Clone)]
pub struct StringConfig {
    /// Default encoding
    pub default_encoding: StringEncoding,
    /// Default text direction
    pub text_direction: TextDirection,
    /// Enable Unicode normalization
    pub enable_normalization: bool,
    /// Normalization form
    pub normalization_form: StringNormalization,
    /// Case sensitivity
    pub case_sensitive: bool,
    /// Locale for operations
    pub locale: String,
    /// Maximum string length
    pub max_string_length: usize,
    /// Enable caching
    pub enable_caching: bool,
    /// Cache size limit
    pub cache_size_limit: usize,
}

impl Default for StringConfig {
    fn default() -> Self {
        Self {
            default_encoding: StringEncoding::Utf8,
            text_direction: TextDirection::LeftToRight,
            enable_normalization: true,
            normalization_form: StringNormalization::NFC,
            case_sensitive: true,
            locale: "zh-CN".to_string(),
            max_string_length: 10 * 1024 * 1024, // 10MB
            enable_caching: true,
            cache_size_limit: 1000,
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
    /// Encoding conversions
    pub encoding_conversions: u64,
    /// Unicode normalizations
    pub normalizations: u64,
    /// Cache hits
    pub cache_hits: u64,
    /// Cache misses
    pub cache_misses: u64,
    /// Total characters processed
    pub total_characters: u64,
    /// Total bytes processed
    pub total_bytes: u64,
}

/// String operation cache
#[derive(Debug, Default)]
pub struct StringCache {
    /// Cached concatenation results
    concatenations: HashMap<String, String>,
    /// Cached substring results
    substrings: HashMap<String, String>,
    /// Cached comparison results
    comparisons: HashMap<String, bool>,
    /// Cached case conversion results
    case_conversions: HashMap<String, String>,
    /// Total cache size (entries)
    size: usize,
}

impl StringInterface {
    /// Create new string interface
    pub fn new() -> Self {
        let config = StringConfig::default();

        Self {
            config: Arc::new(Mutex::new(config)),
            stats: Arc::new(Mutex::new(StringStats::default())),
            cache: Arc::new(Mutex::new(StringCache::default())),
        }
    }

    /// Create string interface with custom configuration
    pub fn with_config(config: StringConfig) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            stats: Arc::new(Mutex::new(StringStats::default())),
            cache: Arc::new(Mutex::new(StringCache::default())),
        }
    }

    /// Concatenate strings
    pub fn concat(&self, strings: &[String]) -> RuntimeResult<String> {
        self.check_string_lengths(strings)?;

        // Check cache if enabled
        let cache_key = self.create_concat_cache_key(strings);
        if self.is_caching_enabled() {
            if let Some(cached) = self.get_cached_concat(&cache_key) {
                self.record_cache_hit();
                return Ok(cached);
            }
            self.record_cache_miss();
        }

        let mut result = String::new();
        let total_chars = strings.iter().map(|s| s.chars().count()).sum();

        for string in strings {
            result.push_str(string);
        }

        // Cache result if enabled
        if self.is_caching_enabled() {
            self.cache_concat_result(&cache_key, &result);
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
                &format!("起始位置 {} 超出字符串长度 {}", start, text.len())
            ));
        }

        let chars: Vec<char> = text.chars().collect();
        if start >= chars.len() {
            return Err(RuntimeError::validation_error(
                "字符串操作错误",
                &format!("起始位置 {} 超出字符长度 {}", start, chars.len())
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

    /// Get string length (bytes)
    pub fn byte_length(&self, text: &str) -> RuntimeResult<usize> {
        let length = text.len();

        self.record_operation("byte_length");
        self.record_bytes_processed(length);

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

    /// Replace occurrences in string
    pub fn replace(&self, text: &str, from: &str, to: &str) -> RuntimeResult<String> {
        if from.is_empty() {
            return Err(RuntimeError::validation_error(
                "字符串操作错误",
                "替换字符串不能为空"
            ));
        }

        let result = text.replace(from, to);

        self.record_operation("replace");
        self.record_characters_processed(text.chars().count());
        self.record_bytes_processed(result.len());

        Ok(result)
    }

    /// Split string by delimiter
    pub fn split(&self, text: &str, delimiter: &str) -> RuntimeResult<Vec<String>> {
        if delimiter.is_empty() {
            return Err(RuntimeError::validation_error(
                "字符串操作错误",
                "分隔符不能为空"
            ));
        }

        let result: Vec<String> = text.split(delimiter).map(|s| s.to_string()).collect();

        self.record_operation("split");
        self.record_characters_processed(text.chars().count());
        self.record_bytes_processed(text.len());

        Ok(result)
    }

    /// Join strings with delimiter
    pub fn join(&self, strings: &[String], delimiter: &str) -> RuntimeResult<String> {
        self.check_string_lengths(strings)?;

        let result = strings.join(delimiter);

        self.record_operation("join");
        self.record_characters_processed(strings.iter().map(|s| s.chars().count()).sum());
        self.record_bytes_processed(result.len());

        Ok(result)
    }

    /// Trim whitespace from string
    pub fn trim(&self, text: &str) -> RuntimeResult<String> {
        let result = text.trim().to_string();

        self.record_operation("trim");
        self.record_characters_processed(result.chars().count());
        self.record_bytes_processed(result.len());

        Ok(result)
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

    /// Check if string contains another string
    pub fn contains(&self, text: &str, pattern: &str) -> RuntimeResult<bool> {
        let config = self.config.lock().unwrap();
        let result = if config.case_sensitive {
            text.contains(pattern)
        } else {
            text.to_lowercase().contains(&pattern.to_lowercase())
        };

        self.record_operation("contains");
        self.record_characters_processed(text.chars().count() + pattern.chars().count());
        self.record_bytes_processed(text.len() + pattern.len());

        Ok(result)
    }

    /// Check if string starts with prefix
    pub fn starts_with(&self, text: &str, prefix: &str) -> RuntimeResult<bool> {
        let config = self.config.lock().unwrap();
        let result = if config.case_sensitive {
            text.starts_with(prefix)
        } else {
            text.to_lowercase().starts_with(&prefix.to_lowercase())
        };

        self.record_operation("starts_with");
        self.record_characters_processed(text.chars().count() + prefix.chars().count());
        self.record_bytes_processed(text.len() + prefix.len());

        Ok(result)
    }

    /// Check if string ends with suffix
    pub fn ends_with(&self, text: &str, suffix: &str) -> RuntimeResult<bool> {
        let config = self.config.lock().unwrap();
        let result = if config.case_sensitive {
            text.ends_with(suffix)
        } else {
            text.to_lowercase().ends_with(&suffix.to_lowercase())
        };

        self.record_operation("ends_with");
        self.record_characters_processed(text.chars().count() + suffix.chars().count());
        self.record_bytes_processed(text.len() + suffix.len());

        Ok(result)
    }

    /// Find first occurrence of pattern
    pub fn find(&self, text: &str, pattern: &str) -> RuntimeResult<Option<usize>> {
        if pattern.is_empty() {
            return Ok(Some(0));
        }

        let result = text.find(pattern).map(|pos| {
            text.char_indices().take_while(|(p, _)| *p < pos).count()
        });

        self.record_operation("find");
        self.record_characters_processed(text.chars().count());
        self.record_bytes_processed(text.len());

        Ok(result)
    }

    /// Convert string encoding
    pub fn convert_encoding(&self, text: &str, from: StringEncoding, to: StringEncoding) -> RuntimeResult<String> {
        if from == to {
            return Ok(text.to_string());
        }

        // For now, we'll just return the original string
        // In a real implementation, you'd handle actual encoding conversion
        self.record_operation("convert_encoding");
        self.record_encoding_conversion();
        self.record_bytes_processed(text.len());

        Ok(text.to_string())
    }

    /// Get string statistics
    pub fn get_stats(&self) -> RuntimeResult<StringStats> {
        let stats = self.stats.lock().unwrap();
        Ok(stats.clone())
    }

    /// Reset statistics
    pub fn reset_stats(&self) -> RuntimeResult<()> {
        let mut stats = self.stats.lock().unwrap();
        *stats = StringStats::default();
        Ok(())
    }

    /// Clear cache
    pub fn clear_cache(&self) -> RuntimeResult<()> {
        let mut cache = self.cache.lock().unwrap();
        *cache = StringCache::default();
        Ok(())
    }

    /// Get configuration
    pub fn get_config(&self) -> RuntimeResult<StringConfig> {
        let config = self.config.lock().unwrap();
        Ok(config.clone())
    }

    /// Update configuration
    pub fn update_config(&self, config: StringConfig) -> RuntimeResult<()> {
        *self.config.lock().unwrap() = config;
        Ok(())
    }

    /// Set case sensitivity
    pub fn set_case_sensitive(&self, enabled: bool) -> RuntimeResult<()> {
        self.config.lock().unwrap().case_sensitive = enabled;
        Ok(())
    }

    /// Set locale
    pub fn set_locale(&self, locale: &str) -> RuntimeResult<()> {
        self.config.lock().unwrap().locale = locale.to_string();
        Ok(())
    }

    /// Enable/disable caching
    pub fn set_caching_enabled(&self, enabled: bool) -> RuntimeResult<()> {
        self.config.lock().unwrap().enable_caching = enabled;
        if !enabled {
            self.clear_cache()?;
        }
        Ok(())
    }

    /// Private helper methods

    fn check_string_lengths(&self, strings: &[String]) -> RuntimeResult<()> {
        let config = self.config.lock().unwrap();
        let total_length: usize = strings.iter().map(|s| s.len()).sum();

        if total_length > config.max_string_length {
            return Err(RuntimeError::validation_error(
                "字符串长度错误",
                &format!("字符串总长度 {} 超过最大限制 {}", total_length, config.max_string_length)
            ));
        }

        Ok(())
    }

    fn is_caching_enabled(&self) -> bool {
        self.config.lock().unwrap().enable_caching
    }

    fn create_concat_cache_key(&self, strings: &[String]) -> String {
        strings.join("|")
    }

    fn get_cached_concat(&self, key: &str) -> Option<String> {
        let cache = self.cache.lock().unwrap();
        cache.concatenations.get(key).cloned()
    }

    fn cache_concat_result(&self, key: &str, result: &str) {
        let mut cache = self.cache.lock().unwrap();

        // Check cache size limit
        if cache.size >= self.config.lock().unwrap().cache_size_limit {
            // Simple LRU: clear all concatenations when limit is reached
            cache.concatenations.clear();
            cache.size = 0;
        }

        cache.concatenations.insert(key.to_string(), result.to_string());
        cache.size += 1;
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

    fn record_cache_hit(&self) {
        let mut stats = self.stats.lock().unwrap();
        stats.cache_hits += 1;
    }

    fn record_cache_miss(&self) {
        let mut stats = self.stats.lock().unwrap();
        stats.cache_misses += 1;
    }

    fn record_case_conversion(&self) {
        let mut stats = self.stats.lock().unwrap();
        stats.case_conversions += 1;
    }

    fn record_encoding_conversion(&self) {
        let mut stats = self.stats.lock().unwrap();
        stats.encoding_conversions += 1;
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
    fn test_string_interface_creation() {
        let string_interface = StringInterface::new();
        let config = string_interface.get_config().unwrap();
        assert_eq!(config.locale, "zh-CN");
        assert!(config.enable_caching);
    }

    #[test]
    fn test_concat() {
        let string_interface = StringInterface::new();
        let strings = vec!["Hello".to_string(), " ".to_string(), "World".to_string()];
        let result = string_interface.concat(&strings).unwrap();
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_substring() {
        let string_interface = StringInterface::new();
        let result = string_interface.substring("Hello World", 6, 5).unwrap();
        assert_eq!(result, "World");
    }

    #[test]
    fn test_length() {
        let string_interface = StringInterface::new();
        let length = string_interface.length("Hello").unwrap();
        assert_eq!(length, 5);
    }

    #[test]
    fn test_compare() {
        let string_interface = StringInterface::new();
        let result = string_interface.compare("abc", "def").unwrap();
        assert_eq!(result, -1);

        let result = string_interface.compare("abc", "abc").unwrap();
        assert_eq!(result, 0);

        let result = string_interface.compare("def", "abc").unwrap();
        assert_eq!(result, 1);
    }

    #[test]
    fn test_case_sensitivity() {
        let string_interface = StringInterface::new();

        // Case sensitive (default)
        let result = string_interface.compare("abc", "ABC").unwrap();
        assert_eq!(result, -1);

        // Case insensitive
        string_interface.set_case_sensitive(false).unwrap();
        let result = string_interface.compare("abc", "ABC").unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn test_string_stats() {
        let string_interface = StringInterface::new();

        string_interface.concat(&vec!["a".to_string(), "b".to_string()]).unwrap();
        string_interface.substring("hello", 1, 2).unwrap();
        string_interface.compare("a", "b").unwrap();

        let stats = string_interface.get_stats().unwrap();
        assert_eq!(stats.total_operations, 3);
        assert_eq!(stats.concatenations, 1);
        assert_eq!(stats.substrings, 1);
        assert_eq!(stats.comparisons, 1);
    }
}