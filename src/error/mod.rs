//! Error Handling and Chinese Language Support
//!
//! This module provides comprehensive error handling with Chinese language
//! localization, stack tracing, and recovery strategies for the Qi runtime.

pub mod chinese;
pub mod handler;

// Re-export main components
pub use chinese::{ChineseErrorMessages, ChineseKeywords, MessageLocalizer};
pub use handler::{ErrorContext, ErrorHandler, RecoveryOption};

/// Error handling result type
pub type ErrorResult<T> = Result<T, Error>;

/// Core error type with Chinese language support
#[derive(Debug, Clone)]
pub enum Error {
    /// Runtime initialization errors
    InitializationFailed {
        message: String,
        chinese_message: String,
        source: Option<String>,
    },

    /// Memory management errors
    MemoryError {
        message: String,
        chinese_message: String,
        operation: Option<String>,
    },

    /// I/O operation errors
    IoError {
        message: String,
        chinese_message: String,
        operation: Option<String>,
        path: Option<String>,
    },

    /// Network operation errors
    NetworkError {
        message: String,
        chinese_message: String,
        operation: Option<String>,
        endpoint: Option<String>,
    },

    /// System call errors
    SystemError {
        message: String,
        chinese_message: String,
        system_call: Option<String>,
        error_code: Option<i32>,
    },

    /// User program errors
    UserError {
        message: String,
        chinese_message: String,
        program_location: Option<String>,
        error_type: Option<String>,
    },

    /// Internal runtime errors
    InternalError {
        message: String,
        chinese_message: String,
        component: Option<String>,
    },

    /// Async runtime task errors
    TaskError {
        message: String,
        chinese_message: String,
        task_id: Option<String>,
    },

    /// Thread-related errors
    ThreadError {
        message: String,
        chinese_message: String,
        thread_id: Option<String>,
    },

    /// Lock/synchronization errors
    LockError {
        message: String,
        chinese_message: String,
    },

    /// Configuration errors
    ConfigurationError {
        message: String,
        chinese_message: String,
    },

    /// General runtime errors
    RuntimeError {
        message: String,
        chinese_message: String,
    },
}

impl Error {
    /// Get the primary error message
    pub fn message(&self) -> &str {
        match self {
            Error::InitializationFailed { message, .. } => message,
            Error::MemoryError { message, .. } => message,
            Error::IoError { message, .. } => message,
            Error::NetworkError { message, .. } => message,
            Error::SystemError { message, .. } => message,
            Error::UserError { message, .. } => message,
            Error::InternalError { message, .. } => message,
            Error::TaskError { message, .. } => message,
            Error::ThreadError { message, .. } => message,
            Error::LockError { message, .. } => message,
            Error::ConfigurationError { message, .. } => message,
            Error::RuntimeError { message, .. } => message,
        }
    }

    /// Get the Chinese error message
    pub fn chinese_message(&self) -> &str {
        match self {
            Error::InitializationFailed {
                chinese_message, ..
            } => chinese_message,
            Error::MemoryError {
                chinese_message, ..
            } => chinese_message,
            Error::IoError {
                chinese_message, ..
            } => chinese_message,
            Error::NetworkError {
                chinese_message, ..
            } => chinese_message,
            Error::SystemError {
                chinese_message, ..
            } => chinese_message,
            Error::UserError {
                chinese_message, ..
            } => chinese_message,
            Error::InternalError {
                chinese_message, ..
            } => chinese_message,
            Error::TaskError {
                chinese_message, ..
            } => chinese_message,
            Error::ThreadError {
                chinese_message, ..
            } => chinese_message,
            Error::LockError {
                chinese_message, ..
            } => chinese_message,
            Error::ConfigurationError {
                chinese_message, ..
            } => chinese_message,
            Error::RuntimeError {
                chinese_message, ..
            } => chinese_message,
        }
    }

    /// Get error severity level
    pub fn severity(&self) -> ErrorSeverity {
        match self {
            Error::InitializationFailed { .. } => ErrorSeverity::Fatal,
            Error::MemoryError { .. } => ErrorSeverity::Warning,
            Error::IoError { .. } => ErrorSeverity::Warning,
            Error::NetworkError { .. } => ErrorSeverity::Warning,
            Error::SystemError { .. } => ErrorSeverity::Fatal,
            Error::UserError { .. } => ErrorSeverity::Info,
            Error::InternalError { .. } => ErrorSeverity::Fatal,
            Error::TaskError { .. } => ErrorSeverity::Warning,
            Error::ThreadError { .. } => ErrorSeverity::Fatal,
            Error::LockError { .. } => ErrorSeverity::Warning,
            Error::ConfigurationError { .. } => ErrorSeverity::Fatal,
            Error::RuntimeError { .. } => ErrorSeverity::Fatal,
        }
    }

    /// Get error category
    pub fn category(&self) -> &'static str {
        match self {
            Error::InitializationFailed { .. } => "初始化错误",
            Error::MemoryError { .. } => "内存错误",
            Error::IoError { .. } => "I/O错误",
            Error::NetworkError { .. } => "网络错误",
            Error::SystemError { .. } => "系统错误",
            Error::UserError { .. } => "用户程序错误",
            Error::InternalError { .. } => "内部错误",
            Error::TaskError { .. } => "任务错误",
            Error::ThreadError { .. } => "线程错误",
            Error::LockError { .. } => "锁错误",
            Error::ConfigurationError { .. } => "配置错误",
            Error::RuntimeError { .. } => "运行时错误",
        }
    }

    /// Create initialization failed error
    pub fn initialization_failed<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::InitializationFailed {
            message: message.into(),
            chinese_message: chinese_message.into(),
            source: None,
        }
    }

    /// Create memory error
    pub fn memory_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::MemoryError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            operation: None,
        }
    }

    /// Create I/O error
    pub fn io_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::IoError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            operation: None,
            path: None,
        }
    }

    /// Create network error
    pub fn network_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::NetworkError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            operation: None,
            endpoint: None,
        }
    }

    /// Create system error
    pub fn system_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::SystemError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            system_call: None,
            error_code: None,
        }
    }

    /// Create user error
    pub fn user_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::UserError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            program_location: None,
            error_type: None,
        }
    }

    /// Create internal error
    pub fn internal_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::InternalError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            component: None,
        }
    }

    /// Create program execution error
    pub fn program_execution_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::UserError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            program_location: None,
            error_type: Some("执行错误".to_string()),
        }
    }

    /// Create validation error
    pub fn validation_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::UserError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            program_location: None,
            error_type: Some("验证错误".to_string()),
        }
    }

    /// Create conversion error
    pub fn conversion_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::UserError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            program_location: None,
            error_type: Some("转换错误".to_string()),
        }
    }

    /// Create debug error
    pub fn debug_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::InternalError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            component: Some("调试".to_string()),
        }
    }

    /// Create assertion error
    pub fn assertion_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::UserError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            program_location: None,
            error_type: Some("断言错误".to_string()),
        }
    }

    /// Create security error
    pub fn security_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::InternalError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            component: Some("安全".to_string()),
        }
    }

    /// Create task error
    pub fn task_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::TaskError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            task_id: None,
        }
    }

    /// Create thread error
    pub fn thread_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::ThreadError {
            message: message.into(),
            chinese_message: chinese_message.into(),
            thread_id: None,
        }
    }

    /// Create lock error
    pub fn lock_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::LockError {
            message: message.into(),
            chinese_message: chinese_message.into(),
        }
    }

    /// Create configuration error
    pub fn configuration_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::ConfigurationError {
            message: message.into(),
            chinese_message: chinese_message.into(),
        }
    }

    /// Create runtime error
    pub fn runtime_error<S: Into<String>>(message: S, chinese_message: S) -> Self {
        Self::RuntimeError {
            message: message.into(),
            chinese_message: chinese_message.into(),
        }
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.category(), self.chinese_message())
    }
}

impl std::error::Error for Error {}

// From implementations for common error types
impl From<crate::memory::MemoryError> for Error {
    fn from(err: crate::memory::MemoryError) -> Self {
        Error::memory_error(format!("{:?}", err), "内存错误".to_string())
    }
}

impl From<crate::io::IoError> for Error {
    fn from(err: crate::io::IoError) -> Self {
        Error::io_error(format!("{:?}", err), "I/O错误".to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::io_error(format!("{:?}", err), "系统I/O错误".to_string())
    }
}

/// Error severity levels
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ErrorSeverity {
    /// Informational message
    Debug = 0,
    /// Informational message
    Info = 1,
    /// Warning that should be addressed
    Warning = 2,
    /// Fatal error that stops execution
    Fatal = 3,
}

impl ErrorSeverity {
    /// Get severity as string
    pub fn as_str(&self) -> &'static str {
        match self {
            ErrorSeverity::Debug => "调试",
            ErrorSeverity::Info => "信息",
            ErrorSeverity::Warning => "警告",
            ErrorSeverity::Fatal => "致命错误",
        }
    }

    /// Check if error is fatal
    pub fn is_fatal(&self) -> bool {
        matches!(self, ErrorSeverity::Fatal)
    }

    /// Check if error requires attention
    pub fn requires_attention(&self) -> bool {
        matches!(self, ErrorSeverity::Warning | ErrorSeverity::Fatal)
    }
}

/// Stack frame information for error tracing
#[derive(Debug, Clone)]
pub struct StackFrame {
    /// Function name
    pub function: String,
    /// File name
    pub file: String,
    /// Line number
    pub line: Option<u32>,
    /// Column number
    pub column: Option<u32>,
}

impl StackFrame {
    /// Create new stack frame
    pub fn new<S: Into<String>>(function: S, file: S) -> Self {
        Self {
            function: function.into(),
            file: file.into(),
            line: None,
            column: None,
        }
    }

    /// Set line number
    pub fn with_line(mut self, line: u32) -> Self {
        self.line = Some(line);
        self
    }

    /// Set column number
    pub fn with_column(mut self, column: u32) -> Self {
        self.column = Some(column);
        self
    }

    /// Format stack frame as string
    pub fn format(&self) -> String {
        let mut result = format!(
            "{} ({}:{}",
            self.function,
            self.file,
            self.line.unwrap_or(0)
        );
        if let Some(col) = self.column {
            result.push_str(&format!(":{}", col));
        }
        result.push(')');
        result
    }
}

/// Error statistics for monitoring
#[derive(Debug, Clone, Default)]
pub struct ErrorStatistics {
    /// Total errors encountered
    pub total_errors: u64,
    /// Fatal errors
    pub fatal_errors: u64,
    /// Warning errors
    pub warning_errors: u64,
    /// Info errors
    pub info_errors: u64,
    /// Debug errors
    pub debug_errors: u64,
    /// Errors by category
    pub errors_by_category: std::collections::HashMap<String, u64>,
}

impl ErrorStatistics {
    /// Create new error statistics
    pub fn new() -> Self {
        Self::default()
    }

    /// Record an error
    pub fn record_error(&mut self, error: &Error) {
        self.total_errors += 1;

        match error.severity() {
            ErrorSeverity::Fatal => self.fatal_errors += 1,
            ErrorSeverity::Warning => self.warning_errors += 1,
            ErrorSeverity::Info => self.info_errors += 1,
            ErrorSeverity::Debug => self.debug_errors += 1,
        }

        let category = error.category().to_string();
        *self.errors_by_category.entry(category).or_insert(0) += 1;
    }

    /// Get error rate by severity
    pub fn error_rate_by_severity(&self, severity: ErrorSeverity) -> f64 {
        if self.total_errors == 0 {
            0.0
        } else {
            let count = match severity {
                ErrorSeverity::Fatal => self.fatal_errors,
                ErrorSeverity::Warning => self.warning_errors,
                ErrorSeverity::Info => self.info_errors,
                ErrorSeverity::Debug => self.debug_errors,
            };
            count as f64 / self.total_errors as f64
        }
    }

    /// Reset statistics
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_creation() {
        let error = Error::memory_error("Out of memory", "内存不足");

        assert_eq!(error.message(), "Out of memory");
        assert_eq!(error.chinese_message(), "内存不足");
        assert_eq!(error.category(), "内存错误");
        assert_eq!(error.severity(), ErrorSeverity::Warning);
    }

    #[test]
    fn test_error_severity() {
        assert_eq!(ErrorSeverity::Debug.as_str(), "调试");
        assert_eq!(ErrorSeverity::Info.as_str(), "信息");
        assert_eq!(ErrorSeverity::Warning.as_str(), "警告");
        assert_eq!(ErrorSeverity::Fatal.as_str(), "致命错误");

        assert!(!ErrorSeverity::Info.is_fatal());
        assert!(ErrorSeverity::Fatal.is_fatal());
        assert!(ErrorSeverity::Warning.requires_attention());
        assert!(!ErrorSeverity::Debug.requires_attention());
    }

    #[test]
    fn test_stack_frame() {
        let frame = StackFrame::new("test_function", "test.rs")
            .with_line(42)
            .with_column(10);

        assert_eq!(frame.function, "test_function");
        assert_eq!(frame.file, "test.rs");
        assert_eq!(frame.line, Some(42));
        assert_eq!(frame.column, Some(10));

        let formatted = frame.format();
        assert!(formatted.contains("test_function"));
        assert!(formatted.contains("test.rs:42:10"));
    }

    #[test]
    fn test_error_statistics() {
        let mut stats = ErrorStatistics::new();

        let error1 = Error::memory_error("test1", "测试1");
        let error2 = Error::io_error("test2", "测试2");

        stats.record_error(&error1);
        stats.record_error(&error2);

        assert_eq!(stats.total_errors, 2);
        assert_eq!(stats.warning_errors, 2);
        assert_eq!(stats.error_rate_by_severity(ErrorSeverity::Warning), 1.0);
        assert_eq!(stats.errors_by_category.get("内存错误"), Some(&1));
        assert_eq!(stats.errors_by_category.get("I/O错误"), Some(&1));
    }

    #[test]
    fn test_error_display() {
        let error = Error::user_error("Invalid input", "无效输入");
        let display = format!("{}", error);
        assert!(display.contains("用户程序错误"));
        assert!(display.contains("无效输入"));
    }
}
