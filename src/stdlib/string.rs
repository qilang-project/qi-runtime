//! String Operations Module
//!
//! This module provides comprehensive string manipulation functions
//! with full Unicode and Chinese language support.

use super::{StdlibError, StdlibResult, StdlibValue};

/// String operation types
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StringOperation {
    Concat,
    Substring,
    Length,
    Compare,
    Replace,
    Split,
    Join,
    Trim,
    Uppercase,
    Lowercase,
    Contains,
    StartsWith,
    EndsWith,
}

/// String module for string operations
#[derive(Debug)]
pub struct StringModule {
    /// Case sensitivity for comparisons
    case_sensitive: bool,
    /// Locale for sorting and comparison
    locale: String,
    /// Unicode normalization mode
    normalization_mode: UnicodeNormalization,
}

/// Unicode normalization modes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnicodeNormalization {
    /// No normalization
    None,
    /// Canonical decomposition
    NFD,
    /// Canonical composition
    NFC,
    /// Compatibility decomposition
    NFKD,
    /// Compatibility composition
    NFKC,
}

impl StringModule {
    /// Create new string module
    pub fn new() -> Self {
        Self {
            case_sensitive: true,
            locale: "zh-CN".to_string(),
            normalization_mode: UnicodeNormalization::NFC,
        }
    }

    /// Create string module with specific locale
    pub fn with_locale(locale: String) -> Self {
        Self {
            case_sensitive: true,
            locale,
            normalization_mode: UnicodeNormalization::NFC,
        }
    }

    /// Initialize the string module
    pub fn initialize(&mut self) -> StdlibResult<()> {
        Ok(())
    }

    /// Execute string operation
    pub fn execute_operation(
        &self,
        operation: StringOperation,
        args: &[StdlibValue],
    ) -> StdlibResult<StdlibValue> {
        match operation {
            StringOperation::Concat => self.concat(args),
            StringOperation::Substring => self.substring(args),
            StringOperation::Length => self.length(args),
            StringOperation::Compare => self.compare(args),
            StringOperation::Replace => self.replace(args),
            StringOperation::Split => self.split(args),
            StringOperation::Join => self.join(args),
            StringOperation::Trim => self.trim(args),
            StringOperation::Uppercase => self.uppercase(args),
            StringOperation::Lowercase => self.lowercase(args),
            StringOperation::Contains => self.contains(args),
            StringOperation::StartsWith => self.starts_with(args),
            StringOperation::EndsWith => self.ends_with(args),
        }
    }

    /// Concatenate strings
    fn concat(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.is_empty() {
            return Ok(StdlibValue::String("".to_string()));
        }

        let mut result = String::new();
        for arg in args {
            match arg {
                StdlibValue::String(s) => result.push_str(s),
                _ => {
                    return Err(StdlibError::StringOperationError {
                        operation: "concat".to_string(),
                        message: "所有参数必须是字符串".to_string(),
                    });
                }
            }
        }

        Ok(StdlibValue::String(result))
    }

    /// Extract substring
    fn substring(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() < 2 {
            return Err(StdlibError::StringOperationError {
                operation: "substring".to_string(),
                message: "需要字符串和起始索引参数".to_string(),
            });
        }

        let string = match &args[0] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "substring".to_string(),
                    message: "第一个参数必须是字符串".to_string(),
                });
            }
        };

        let start = match &args[1] {
            StdlibValue::Integer(i) => *i as usize,
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "substring".to_string(),
                    message: "第二个参数必须是整数".to_string(),
                });
            }
        };

        let end = if args.len() > 2 {
            match &args[2] {
                StdlibValue::Integer(i) => Some(*i as usize),
                _ => {
                    return Err(StdlibError::StringOperationError {
                        operation: "substring".to_string(),
                        message: "第三个参数必须是整数".to_string(),
                    });
                }
            }
        } else {
            None
        };

        let string_chars: Vec<char> = string.chars().collect();
        let length = string_chars.len();

        if start > length {
            return Ok(StdlibValue::String("".to_string()));
        }

        let end = end.unwrap_or(length);
        let end = end.min(length);

        if start > end {
            return Ok(StdlibValue::String("".to_string()));
        }

        let substring: String = string_chars[start..end].iter().collect();
        Ok(StdlibValue::String(substring))
    }

    /// Get string length
    fn length(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() != 1 {
            return Err(StdlibError::StringOperationError {
                operation: "length".to_string(),
                message: "需要一个字符串参数".to_string(),
            });
        }

        match &args[0] {
            StdlibValue::String(s) => Ok(StdlibValue::Integer(s.chars().count() as i64)),
            _ => Err(StdlibError::StringOperationError {
                operation: "length".to_string(),
                message: "参数必须是字符串".to_string(),
            }),
        }
    }

    /// Compare strings
    fn compare(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() < 2 {
            return Err(StdlibError::StringOperationError {
                operation: "compare".to_string(),
                message: "需要两个字符串参数".to_string(),
            });
        }

        let s1 = match &args[0] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "compare".to_string(),
                    message: "第一个参数必须是字符串".to_string(),
                });
            }
        };

        let s2 = match &args[1] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "compare".to_string(),
                    message: "第二个参数必须是字符串".to_string(),
                });
            }
        };

        let result = if self.case_sensitive {
            s1.cmp(&s2)
        } else {
            s1.to_lowercase().cmp(&s2.to_lowercase())
        };

        Ok(StdlibValue::Integer(result as i64))
    }

    /// Replace text in string
    fn replace(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() < 3 {
            return Err(StdlibError::StringOperationError {
                operation: "replace".to_string(),
                message: "需要字符串、搜索字符串和替换字符串参数".to_string(),
            });
        }

        let string = match &args[0] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "replace".to_string(),
                    message: "第一个参数必须是字符串".to_string(),
                });
            }
        };

        let search = match &args[1] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "replace".to_string(),
                    message: "第二个参数必须是字符串".to_string(),
                });
            }
        };

        let replacement = match &args[2] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "replace".to_string(),
                    message: "第三个参数必须是字符串".to_string(),
                });
            }
        };

        let result = string.replace(&search, &replacement);
        Ok(StdlibValue::String(result))
    }

    /// Split string
    fn split(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() < 2 {
            return Err(StdlibError::StringOperationError {
                operation: "split".to_string(),
                message: "需要字符串和分隔符参数".to_string(),
            });
        }

        let string = match &args[0] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "split".to_string(),
                    message: "第一个参数必须是字符串".to_string(),
                });
            }
        };

        let separator = match &args[1] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "split".to_string(),
                    message: "第二个参数必须是字符串".to_string(),
                });
            }
        };

        let parts: Vec<String> = string.split(&separator).map(|s| s.to_string()).collect();
        let array_parts: Vec<StdlibValue> = parts.into_iter().map(StdlibValue::String).collect();

        Ok(StdlibValue::Array(array_parts))
    }

    /// Join array of strings
    fn join(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() < 1 {
            return Err(StdlibError::StringOperationError {
                operation: "join".to_string(),
                message: "需要至少一个参数".to_string(),
            });
        }

        let separator = if args.len() > 1 {
            match &args[0] {
                StdlibValue::String(s) => s.clone(),
                _ => {
                    return Err(StdlibError::StringOperationError {
                        operation: "join".to_string(),
                        message: "分隔符必须是字符串".to_string(),
                    });
                }
            }
        } else {
            "".to_string()
        };

        let strings: Vec<String> = if args.len() == 2
            && matches!(&args[0], StdlibValue::String(_))
            && matches!(&args[1], StdlibValue::Array(_))
        {
            // Pattern: separator + array of strings
            match &args[1] {
                StdlibValue::Array(arr) => arr
                    .iter()
                    .map(|arg| match arg {
                        StdlibValue::String(s) => Ok(s.clone()),
                        _ => Err(StdlibError::StringOperationError {
                            operation: "join".to_string(),
                            message: "数组元素必须是字符串".to_string(),
                        }),
                    })
                    .collect::<Result<Vec<String>, StdlibError>>()?,
                _ => unreachable!(), // We already checked this pattern
            }
        } else if args.len() > 1 {
            args[1..]
                .iter()
                .map(|arg| match arg {
                    StdlibValue::String(s) => Ok(s.clone()),
                    _ => Err(StdlibError::StringOperationError {
                        operation: "join".to_string(),
                        message: "数组元素必须是字符串".to_string(),
                    }),
                })
                .collect::<Result<Vec<String>, StdlibError>>()?
        } else if args.len() == 1 && matches!(&args[0], StdlibValue::Array(_)) {
            // Pattern: just array of strings
            match &args[0] {
                StdlibValue::Array(arr) => arr
                    .iter()
                    .map(|arg| match arg {
                        StdlibValue::String(s) => Ok(s.clone()),
                        _ => Err(StdlibError::StringOperationError {
                            operation: "join".to_string(),
                            message: "数组元素必须是字符串".to_string(),
                        }),
                    })
                    .collect::<Result<Vec<String>, StdlibError>>()?,
                _ => unreachable!(), // We already checked this pattern
            }
        } else if args.len() == 1 && matches!(&args[0], StdlibValue::String(_)) {
            // Pattern: single string (just return it)
            match &args[0] {
                StdlibValue::String(s) => vec![s.clone()],
                _ => unreachable!(), // We already checked this pattern
            }
        } else {
            return Err(StdlibError::StringOperationError {
                operation: "join".to_string(),
                message: "参数格式不正确".to_string(),
            });
        };

        let result = strings.join(&separator);
        Ok(StdlibValue::String(result))
    }

    /// Trim whitespace
    fn trim(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() != 1 {
            return Err(StdlibError::StringOperationError {
                operation: "trim".to_string(),
                message: "需要一个字符串参数".to_string(),
            });
        }

        match &args[0] {
            StdlibValue::String(s) => Ok(StdlibValue::String(s.trim().to_string())),
            _ => Err(StdlibError::StringOperationError {
                operation: "trim".to_string(),
                message: "参数必须是字符串".to_string(),
            }),
        }
    }

    /// Convert to uppercase
    fn uppercase(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() != 1 {
            return Err(StdlibError::StringOperationError {
                operation: "uppercase".to_string(),
                message: "需要一个字符串参数".to_string(),
            });
        }

        match &args[0] {
            StdlibValue::String(s) => Ok(StdlibValue::String(s.to_uppercase())),
            _ => Err(StdlibError::StringOperationError {
                operation: "uppercase".to_string(),
                message: "参数必须是字符串".to_string(),
            }),
        }
    }

    /// Convert to lowercase
    fn lowercase(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() != 1 {
            return Err(StdlibError::StringOperationError {
                operation: "lowercase".to_string(),
                message: "需要一个字符串参数".to_string(),
            });
        }

        match &args[0] {
            StdlibValue::String(s) => Ok(StdlibValue::String(s.to_lowercase())),
            _ => Err(StdlibError::StringOperationError {
                operation: "lowercase".to_string(),
                message: "参数必须是字符串".to_string(),
            }),
        }
    }

    /// Check if string contains substring
    fn contains(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() < 2 {
            return Err(StdlibError::StringOperationError {
                operation: "contains".to_string(),
                message: "需要字符串和搜索字符串参数".to_string(),
            });
        }

        let string = match &args[0] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "contains".to_string(),
                    message: "第一个参数必须是字符串".to_string(),
                });
            }
        };

        let search = match &args[1] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "contains".to_string(),
                    message: "第二个参数必须是字符串".to_string(),
                });
            }
        };

        let contains = string.contains(&search);
        Ok(StdlibValue::Boolean(contains))
    }

    /// Check if string starts with prefix
    fn starts_with(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() < 2 {
            return Err(StdlibError::StringOperationError {
                operation: "starts_with".to_string(),
                message: "需要字符串和前缀参数".to_string(),
            });
        }

        let string = match &args[0] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "starts_with".to_string(),
                    message: "第一个参数必须是字符串".to_string(),
                });
            }
        };

        let prefix = match &args[1] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "starts_with".to_string(),
                    message: "第二个参数必须是字符串".to_string(),
                });
            }
        };

        let starts_with = string.starts_with(&prefix);
        Ok(StdlibValue::Boolean(starts_with))
    }

    /// Check if string ends with suffix
    fn ends_with(&self, args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
        if args.len() < 2 {
            return Err(StdlibError::StringOperationError {
                operation: "ends_with".to_string(),
                message: "需要字符串和后缀参数".to_string(),
            });
        }

        let string = match &args[0] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "ends_with".to_string(),
                    message: "第一个参数必须是字符串".to_string(),
                });
            }
        };

        let suffix = match &args[1] {
            StdlibValue::String(s) => s.clone(),
            _ => {
                return Err(StdlibError::StringOperationError {
                    operation: "ends_with".to_string(),
                    message: "第二个参数必须是字符串".to_string(),
                });
            }
        };

        let ends_with = string.ends_with(&suffix);
        Ok(StdlibValue::Boolean(ends_with))
    }

    /// Set case sensitivity
    pub fn set_case_sensitive(&mut self, case_sensitive: bool) {
        self.case_sensitive = case_sensitive;
    }

    /// Get case sensitivity
    pub fn is_case_sensitive(&self) -> bool {
        self.case_sensitive
    }

    /// Set locale
    pub fn set_locale(&mut self, locale: String) {
        self.locale = locale;
    }

    /// Get locale
    pub fn get_locale(&self) -> &str {
        &self.locale
    }

    /// Set Unicode normalization mode
    pub fn set_normalization_mode(&mut self, mode: UnicodeNormalization) {
        self.normalization_mode = mode;
    }

    /// Get Unicode normalization mode
    pub fn get_normalization_mode(&self) -> UnicodeNormalization {
        self.normalization_mode
    }
}

impl Default for StringModule {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_string_module_creation() {
        let module = StringModule::new();
        assert!(module.is_case_sensitive());
        assert_eq!(module.get_locale(), "zh-CN");
        assert_eq!(module.get_normalization_mode(), UnicodeNormalization::NFC);
    }

    #[test]
    fn test_string_concat() {
        let module = StringModule::new();
        let args = vec![
            StdlibValue::String("Hello".to_string()),
            StdlibValue::String(" ".to_string()),
            StdlibValue::String("World!".to_string()),
        ];

        let result = module.concat(&args);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            StdlibValue::String("Hello World!".to_string())
        );
    }

    #[test]
    fn test_string_length() {
        let module = StringModule::new();
        let args = vec![StdlibValue::String("你好，世界！".to_string())];

        let result = module.length(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::Integer(6)); // 你、好、，、世、界、！
    }

    #[test]
    fn test_string_chinese_operations() {
        let module = StringModule::new();

        // Chinese string length
        let args = vec![StdlibValue::String("测试中文字符串".to_string())];
        let result = module.length(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::Integer(7));

        // Chinese string comparison
        let args = vec![
            StdlibValue::String("测试".to_string()),
            StdlibValue::String("测试".to_string()),
        ];
        let result = module.compare(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::Integer(0));

        // Case insensitive comparison
        let mut module = StringModule::new();
        module.set_case_sensitive(false);
        let args = vec![
            StdlibValue::String("Test".to_string()),
            StdlibValue::String("test".to_string()),
        ];
        let result = module.compare(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::Integer(0));
    }

    #[test]
    fn test_string_replace() {
        let module = StringModule::new();
        let args = vec![
            StdlibValue::String("Hello World".to_string()),
            StdlibValue::String("World".to_string()),
            StdlibValue::String("Rust".to_string()),
        ];

        let result = module.replace(&args);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            StdlibValue::String("Hello Rust".to_string())
        );
    }

    #[test]
    fn test_string_split() {
        let module = StringModule::new();
        let args = vec![
            StdlibValue::String("a,b,c,d".to_string()),
            StdlibValue::String(",".to_string()),
        ];

        let result = module.split(&args);
        assert!(result.is_ok());

        if let StdlibValue::Array(parts) = result.unwrap() {
            assert_eq!(parts.len(), 4);
            assert_eq!(parts[0], StdlibValue::String("a".to_string()));
            assert_eq!(parts[1], StdlibValue::String("b".to_string()));
            assert_eq!(parts[2], StdlibValue::String("c".to_string()));
            assert_eq!(parts[3], StdlibValue::String("d".to_string()));
        } else {
            panic!("Expected array result");
        }
    }

    #[test]
    fn test_string_join() {
        let module = StringModule::new();
        let args = vec![
            StdlibValue::String(",".to_string()),
            StdlibValue::Array(vec![
                StdlibValue::String("a".to_string()),
                StdlibValue::String("b".to_string()),
                StdlibValue::String("c".to_string()),
            ]),
        ];

        let result = module.join(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::String("a,b,c".to_string()));
    }

    #[test]
    fn test_string_case_operations() {
        let module = StringModule::new();

        let args = vec![StdlibValue::String("hello".to_string())];
        let result = module.uppercase(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::String("HELLO".to_string()));

        let args = vec![StdlibValue::String("HELLO".to_string())];
        let result = module.lowercase(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::String("hello".to_string()));
    }

    #[test]
    fn test_string_search_operations() {
        let module = StringModule::new();

        let args = vec![
            StdlibValue::String("Hello World".to_string()),
            StdlibValue::String("World".to_string()),
        ];
        let result = module.contains(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::Boolean(true));

        let args = vec![
            StdlibValue::String("Hello World".to_string()),
            StdlibValue::String("Universe".to_string()),
        ];
        let result = module.contains(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::Boolean(false));
    }

    #[test]
    fn test_string_prefix_suffix() {
        let module = StringModule::new();

        let args = vec![
            StdlibValue::String("Hello World".to_string()),
            StdlibValue::String("Hello".to_string()),
        ];
        let result = module.starts_with(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::Boolean(true));

        let args = vec![
            StdlibValue::String("Hello World".to_string()),
            StdlibValue::String("World".to_string()),
        ];
        let result = module.ends_with(&args);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::Boolean(true));
    }

    #[test]
    fn test_error_handling() {
        let module = StringModule::new();

        // Invalid argument type
        let args = vec![StdlibValue::Integer(42)];
        let result = module.length(&args);
        assert!(result.is_err());

        // Insufficient arguments
        let args = vec![];
        let result = module.compare(&args);
        assert!(result.is_err());

        // Invalid second argument type
        let args = vec![
            StdlibValue::String("test".to_string()),
            StdlibValue::Integer(42),
        ];
        let result = module.compare(&args);
        assert!(result.is_err());
    }
}
