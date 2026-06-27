//! Error Handler Implementation
//!
//! This module provides comprehensive error handling with context tracking,
//! recovery strategies, and Chinese language support.

use super::{Error, ErrorSeverity, ErrorStatistics, StackFrame};
use std::collections::HashMap;

/// Error context information
#[derive(Debug, Clone)]
pub struct ErrorContext {
    /// Error message
    pub error: Error,
    /// Stack frames
    pub stack_trace: Vec<StackFrame>,
    /// Additional context data
    pub metadata: HashMap<String, String>,
    /// Error timestamp
    pub timestamp: std::time::SystemTime,
}

impl ErrorContext {
    /// Create new error context
    pub fn new(error: Error) -> Self {
        Self {
            error,
            stack_trace: Vec::new(),
            metadata: HashMap::new(),
            timestamp: std::time::SystemTime::now(),
        }
    }

    /// Add stack frame
    pub fn add_frame(mut self, frame: StackFrame) -> Self {
        self.stack_trace.push(frame);
        self
    }

    /// Add metadata
    pub fn with_metadata<K: Into<String>, V: Into<String>>(mut self, key: K, value: V) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    /// Format context as string
    pub fn format(&self) -> String {
        let mut result = format!("错误: {}", self.error);

        if !self.stack_trace.is_empty() {
            result.push_str("\n堆栈跟踪:");
            for frame in &self.stack_trace {
                result.push_str(&format!("\n  {}", frame.format()));
            }
        }

        if !self.metadata.is_empty() {
            result.push_str("\n上下文信息:");
            for (key, value) in &self.metadata {
                result.push_str(&format!("\n  {}: {}", key, value));
            }
        }

        result
    }
}

/// Recovery option for error handling
#[derive(Debug, Clone)]
pub enum RecoveryOption {
    /// No recovery possible
    None,
    /// Retry the operation
    Retry,
    /// Use default value
    UseDefault,
    /// Skip current operation
    Skip,
    /// Abort execution
    Abort,
    /// Custom recovery strategy
    Custom(String),
}

impl RecoveryOption {
    /// Get description in Chinese
    pub fn description(&self) -> &'static str {
        match self {
            RecoveryOption::None => "无法恢复",
            RecoveryOption::Retry => "重试操作",
            RecoveryOption::UseDefault => "使用默认值",
            RecoveryOption::Skip => "跳过操作",
            RecoveryOption::Abort => "中止执行",
            RecoveryOption::Custom(_) => "自定义恢复策略",
        }
    }
}

/// Error handler configuration
#[derive(Debug, Clone)]
pub struct ErrorHandlerConfig {
    /// Maximum error count before aborting
    pub max_errors: usize,
    /// Enable stack trace collection
    pub collect_stack_trace: bool,
    /// Enable error statistics
    pub enable_statistics: bool,
    /// Log errors to console
    pub log_to_console: bool,
    /// Log errors to file
    pub log_to_file: bool,
    /// Log file path
    pub log_file_path: Option<String>,
}

impl Default for ErrorHandlerConfig {
    fn default() -> Self {
        Self {
            max_errors: 100,
            collect_stack_trace: true,
            enable_statistics: true,
            log_to_console: true,
            log_to_file: false,
            log_file_path: None,
        }
    }
}

/// Error handler
#[derive(Debug)]
pub struct ErrorHandler {
    /// Configuration
    config: ErrorHandlerConfig,
    /// Error history
    error_history: Vec<ErrorContext>,
    /// Error statistics
    statistics: ErrorStatistics,
    /// Error count
    error_count: usize,
}

impl ErrorHandler {
    /// Create new error handler
    pub fn new() -> Self {
        Self::with_config(ErrorHandlerConfig::default())
    }

    /// Create error handler with configuration
    pub fn with_config(config: ErrorHandlerConfig) -> Self {
        Self {
            config,
            error_history: Vec::new(),
            statistics: ErrorStatistics::new(),
            error_count: 0,
        }
    }

    /// Initialize the error handler
    pub fn initialize(&mut self) -> crate::RuntimeResult<()> {
        // Reset error history and statistics on initialization
        self.clear_history();
        self.reset_statistics();
        Ok(())
    }

    /// Handle an error
    pub fn handle_error(&mut self, error: Error) -> RecoveryOption {
        // Create error context
        let mut context = ErrorContext::new(error.clone());

        // Collect stack trace if enabled
        if self.config.collect_stack_trace {
            // In a real implementation, you'd collect the actual stack trace
            // For now, we'll add a placeholder
            context = context.add_frame(StackFrame::new("unknown_function", "unknown.rs"));
        }

        // Update statistics
        if self.config.enable_statistics {
            self.statistics.record_error(&error);
        }

        // Add to history
        self.error_history.push(context.clone());
        self.error_count += 1;

        // Log error if enabled
        if self.config.log_to_console {
            eprintln!("{}", context.format());
        }

        if self.config.log_to_file {
            if let Some(ref path) = self.config.log_file_path {
                let _ = self.log_to_file(&context, path);
            }
        }

        // Check if we should abort
        if self.error_count >= self.config.max_errors {
            return RecoveryOption::Abort;
        }

        // Determine recovery strategy based on error severity
        match error.severity() {
            ErrorSeverity::Fatal => RecoveryOption::Abort,
            ErrorSeverity::Warning => RecoveryOption::Retry,
            ErrorSeverity::Info => RecoveryOption::Skip,
            ErrorSeverity::Debug => RecoveryOption::None,
        }
    }

    /// Handle error with custom context
    pub fn handle_error_with_context(&mut self, error: Error, frame: StackFrame) -> RecoveryOption {
        let context = ErrorContext::new(error.clone()).add_frame(frame);

        // Update statistics and history
        if self.config.enable_statistics {
            self.statistics.record_error(&error);
        }

        self.error_history.push(context.clone());
        self.error_count += 1;

        // Log error
        if self.config.log_to_console {
            eprintln!("{}", context.format());
        }

        // Return recovery option
        match error.severity() {
            ErrorSeverity::Fatal => RecoveryOption::Abort,
            ErrorSeverity::Warning => RecoveryOption::Retry,
            ErrorSeverity::Info => RecoveryOption::Skip,
            ErrorSeverity::Debug => RecoveryOption::None,
        }
    }

    /// Get error history
    pub fn get_error_history(&self) -> &[ErrorContext] {
        &self.error_history
    }

    /// Get error statistics
    pub fn get_statistics(&self) -> &ErrorStatistics {
        &self.statistics
    }

    /// Get error count
    pub fn error_count(&self) -> usize {
        self.error_count
    }

    /// Clear error history
    pub fn clear_history(&mut self) {
        self.error_history.clear();
        self.error_count = 0;
    }

    /// Reset statistics
    pub fn reset_statistics(&mut self) {
        self.statistics.reset();
    }

    /// Log to file
    fn log_to_file(&self, context: &ErrorContext, path: &str) -> Result<(), std::io::Error> {
        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new().create(true).append(true).open(path)?;

        let log_entry = format!(
            "{}\n{}\n",
            context.timestamp.elapsed().unwrap_or_default().as_secs(),
            context.format()
        );
        file.write_all(log_entry.as_bytes())?;
        file.flush()?;

        Ok(())
    }

    /// Check if error limit is reached
    pub fn is_error_limit_reached(&self) -> bool {
        self.error_count >= self.config.max_errors
    }

    /// Get configuration
    pub fn config(&self) -> &ErrorHandlerConfig {
        &self.config
    }

    /// Update configuration
    pub fn update_config(&mut self, config: ErrorHandlerConfig) {
        self.config = config;
    }
}

impl Default for ErrorHandler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_handler_creation() {
        let handler = ErrorHandler::new();
        assert_eq!(handler.error_count(), 0);
        assert!(handler.get_error_history().is_empty());
    }

    #[test]
    fn test_error_handling() {
        let mut handler = ErrorHandler::new();

        let error = Error::memory_error("Out of memory", "内存不足");
        let recovery = handler.handle_error(error.clone());

        assert_eq!(handler.error_count(), 1);
        assert_eq!(handler.get_error_history().len(), 1);

        // Should suggest retry for warning level errors
        assert!(matches!(recovery, RecoveryOption::Retry));
    }

    #[test]
    fn test_fatal_error_handling() {
        let mut handler = ErrorHandler::new();

        let error = Error::initialization_failed("Failed to init", "初始化失败");
        let recovery = handler.handle_error(error.clone());

        // Should abort for fatal errors
        assert!(matches!(recovery, RecoveryOption::Abort));
    }

    #[test]
    fn test_error_context() {
        let error = Error::user_error("Invalid input", "无效输入");
        let context = ErrorContext::new(error.clone())
            .add_frame(StackFrame::new("test_func", "test.rs").with_line(42))
            .with_metadata("user_id", "123");

        assert_eq!(context.stack_trace.len(), 1);
        assert_eq!(context.metadata.get("user_id"), Some(&"123".to_string()));
    }

    #[test]
    fn test_error_statistics() {
        let mut handler = ErrorHandler::new();

        let error1 = Error::memory_error("test1", "测试1");
        let error2 = Error::io_error("test2", "测试2");

        handler.handle_error(error1);
        handler.handle_error(error2);

        let stats = handler.get_statistics();
        assert_eq!(stats.total_errors, 2);
        assert_eq!(stats.warning_errors, 2);
    }

    #[test]
    fn test_error_limit() {
        let mut config = ErrorHandlerConfig::default();
        config.max_errors = 2;
        let mut handler = ErrorHandler::with_config(config);

        let error = Error::memory_error("test", "测试");

        handler.handle_error(error.clone());
        assert!(!handler.is_error_limit_reached());

        handler.handle_error(error.clone());
        assert!(handler.is_error_limit_reached());
    }

    #[test]
    fn test_recovery_options() {
        assert_eq!(RecoveryOption::Retry.description(), "重试操作");
        assert_eq!(RecoveryOption::Abort.description(), "中止执行");
        assert_eq!(RecoveryOption::Skip.description(), "跳过操作");
    }
}
