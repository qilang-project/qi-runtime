//! Debug and Development Module
//!
//! This module provides debugging utilities, logging, and development
//! tools with Chinese language support.

use crate::{RuntimeError, RuntimeResult};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Log level enumeration
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LogLevel {
    /// Trace level
    Trace = 0,
    /// Debug level
    Debug = 1,
    /// Info level
    Info = 2,
    /// Warning level
    Warning = 3,
    /// Error level
    Error = 4,
    /// Fatal level
    Fatal = 5,
}

impl LogLevel {
    /// Get Chinese name for log level
    pub fn chinese_name(&self) -> &'static str {
        match self {
            LogLevel::Trace => "跟踪",
            LogLevel::Debug => "调试",
            LogLevel::Info => "信息",
            LogLevel::Warning => "警告",
            LogLevel::Error => "错误",
            LogLevel::Fatal => "致命",
        }
    }

    /// Get English name for log level
    pub fn english_name(&self) -> &'static str {
        match self {
            LogLevel::Trace => "TRACE",
            LogLevel::Debug => "DEBUG",
            LogLevel::Info => "INFO",
            LogLevel::Warning => "WARN",
            LogLevel::Error => "ERROR",
            LogLevel::Fatal => "FATAL",
        }
    }

    /// Get color code for log level
    pub fn color_code(&self) -> &'static str {
        match self {
            LogLevel::Trace => "\x1b[37m",   // White
            LogLevel::Debug => "\x1b[36m",   // Cyan
            LogLevel::Info => "\x1b[32m",    // Green
            LogLevel::Warning => "\x1b[33m", // Yellow
            LogLevel::Error => "\x1b[31m",   // Red
            LogLevel::Fatal => "\x1b[35m",   // Magenta
        }
    }
}

/// Debug configuration
#[derive(Debug, Clone)]
pub struct DebugConfig {
    /// Minimum log level
    pub min_level: LogLevel,
    /// Enable colored output
    pub colored_output: bool,
    /// Enable timestamps
    pub show_timestamps: bool,
    /// Use Chinese log level names
    pub chinese_levels: bool,
    /// Enable file and line number display
    pub show_location: bool,
    /// Log to file
    pub log_to_file: bool,
    /// Log file path
    pub log_file_path: Option<String>,
    /// Maximum log file size in bytes
    pub max_file_size: Option<u64>,
    /// Enable debug output
    pub debug_enabled: bool,
}

impl Default for DebugConfig {
    fn default() -> Self {
        Self {
            min_level: LogLevel::Info,
            colored_output: true,
            show_timestamps: true,
            chinese_levels: true,
            show_location: false,
            log_to_file: false,
            log_file_path: None,
            max_file_size: Some(10 * 1024 * 1024), // 10MB
            debug_enabled: false,
        }
    }
}

/// Log entry structure
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// Timestamp
    pub timestamp: u64,
    /// Log level
    pub level: LogLevel,
    /// Message
    pub message: String,
    /// Source file
    pub file: Option<String>,
    /// Line number
    pub line: Option<u32>,
    /// Thread ID
    pub thread_id: Option<String>,
}

/// Performance metric structure
#[derive(Debug, Clone)]
pub struct PerformanceMetric {
    /// Metric name
    pub name: String,
    /// Start time
    pub start_time: u64,
    /// End time
    pub end_time: u64,
    /// Duration in microseconds
    pub duration_us: u64,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

/// Debug and development module
#[derive(Debug)]
pub struct DebugModule {
    /// Configuration
    config: Arc<Mutex<DebugConfig>>,
    /// Log entries
    log_entries: Arc<Mutex<Vec<LogEntry>>>,
    /// Performance metrics
    metrics: Arc<Mutex<Vec<PerformanceMetric>>>,
    /// Active timers
    active_timers: Arc<Mutex<HashMap<String, u64>>>,
    /// Statistics
    statistics: Arc<Mutex<HashMap<LogLevel, usize>>>,
}

impl DebugModule {
    /// Create new debug module
    pub fn new() -> Self {
        Self::with_config(DebugConfig::default())
    }

    /// Create debug module with configuration
    pub fn with_config(config: DebugConfig) -> Self {
        Self {
            config: Arc::new(Mutex::new(config)),
            log_entries: Arc::new(Mutex::new(Vec::new())),
            metrics: Arc::new(Mutex::new(Vec::new())),
            active_timers: Arc::new(Mutex::new(HashMap::new())),
            statistics: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Log a message
    pub fn log(
        &self,
        level: LogLevel,
        message: &str,
        file: Option<&str>,
        line: Option<u32>,
    ) -> RuntimeResult<()> {
        let config = self.config.lock().unwrap();

        if level < config.min_level {
            return Ok(());
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| {
                RuntimeError::debug_error(
                    format!("获取时间戳失败: {}", e),
                    "获取时间戳失败".to_string(),
                )
            })?
            .as_secs();

        let entry = LogEntry {
            timestamp,
            level,
            message: message.to_string(),
            file: file.map(|s| s.to_string()),
            line,
            thread_id: Some("main".to_string()), // Simplified thread ID
        };

        // Update statistics
        {
            let mut stats = self.statistics.lock().unwrap();
            *stats.entry(level).or_insert(0) += 1;
        }

        // Store entry
        {
            let mut entries = self.log_entries.lock().unwrap();
            entries.push(entry.clone());

            // Limit entries to prevent memory issues
            if entries.len() > 10000 {
                entries.remove(0);
            }
        }

        // Output to console
        if config.colored_output {
            self.print_colored(&entry, &config);
        } else {
            self.print_plain(&entry, &config);
        }

        // Output to file if enabled
        if config.log_to_file {
            if let Err(e) = self.write_to_file(&entry, &config) {
                eprintln!("写入日志文件失败: {}", e);
            }
        }

        Ok(())
    }

    /// Log trace message
    pub fn trace(&self, message: &str) -> RuntimeResult<()> {
        self.log(LogLevel::Trace, message, None, None)
    }

    /// Log debug message
    pub fn debug(&self, message: &str) -> RuntimeResult<()> {
        self.log(LogLevel::Debug, message, None, None)
    }

    /// Log info message
    pub fn info(&self, message: &str) -> RuntimeResult<()> {
        self.log(LogLevel::Info, message, None, None)
    }

    /// Log warning message
    pub fn warning(&self, message: &str) -> RuntimeResult<()> {
        self.log(LogLevel::Warning, message, None, None)
    }

    /// Log error message
    pub fn error(&self, message: &str) -> RuntimeResult<()> {
        self.log(LogLevel::Error, message, None, None)
    }

    /// Log fatal message
    pub fn fatal(&self, message: &str) -> RuntimeResult<()> {
        self.log(LogLevel::Fatal, message, None, None)
    }

    /// Start a performance timer
    pub fn start_timer(&self, name: &str) -> RuntimeResult<()> {
        let start_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| {
                RuntimeError::debug_error(
                    format!("获取时间戳失败: {}", e),
                    "获取时间戳失败".to_string(),
                )
            })?
            .as_micros() as u64;

        let mut timers = self.active_timers.lock().unwrap();
        timers.insert(name.to_string(), start_time);

        Ok(())
    }

    /// End a performance timer and record metric
    pub fn end_timer(&self, name: &str) -> RuntimeResult<u64> {
        let end_time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| {
                RuntimeError::debug_error(
                    format!("获取时间戳失败: {}", e),
                    "获取时间戳失败".to_string(),
                )
            })?
            .as_micros() as u64;

        let mut timers = self.active_timers.lock().unwrap();

        let start_time = timers.remove(name).ok_or_else(|| {
            RuntimeError::debug_error(
                format!("计时器 '{}' 不存在", name),
                "计时器不存在".to_string(),
            )
        })?;

        let duration = end_time - start_time;

        let metric = PerformanceMetric {
            name: name.to_string(),
            start_time,
            end_time,
            duration_us: duration,
            metadata: HashMap::new(),
        };

        {
            let mut metrics = self.metrics.lock().unwrap();
            metrics.push(metric);

            // Limit metrics to prevent memory issues
            if metrics.len() > 1000 {
                metrics.remove(0);
            }
        }

        Ok(duration)
    }

    /// Measure execution time of a function
    pub fn measure<F, R>(&self, name: &str, f: F) -> RuntimeResult<R>
    where
        F: FnOnce() -> R,
    {
        self.start_timer(name)?;
        let result = f();
        let _duration = self.end_timer(name)?;
        Ok(result)
    }

    /// Assert a condition (debug builds only)
    pub fn assert(&self, condition: bool, message: &str) -> RuntimeResult<()> {
        let config = self.config.lock().unwrap();
        if config.debug_enabled && !condition {
            return Err(RuntimeError::assertion_error(
                format!("断言失败: {}", message),
                "断言失败".to_string(),
            ));
        }
        Ok(())
    }

    /// Print variable value for debugging
    pub fn dump<T: std::fmt::Debug>(&self, name: &str, value: &T) -> RuntimeResult<()> {
        let message = format!("{} = {:?}", name, value);
        self.debug(&message)
    }

    /// Print colored log entry
    fn print_colored(&self, entry: &LogEntry, config: &DebugConfig) {
        let level_name = if config.chinese_levels {
            entry.level.chinese_name()
        } else {
            entry.level.english_name()
        };

        let color = entry.level.color_code();
        let reset = "\x1b[0m";

        let mut output = String::new();

        if config.show_timestamps {
            let time_str = format_timestamp(entry.timestamp);
            output.push_str(&format!("{} ", time_str));
        }

        output.push_str(&format!("{}[{}]{} ", color, level_name, reset));
        output.push_str(&entry.message);

        if config.show_location {
            if let Some(file) = &entry.file {
                output.push_str(&format!(" ({}:{})", file, entry.line.unwrap_or(0)));
            }
        }

        println!("{}", output);
    }

    /// Print plain log entry
    fn print_plain(&self, entry: &LogEntry, config: &DebugConfig) {
        let level_name = if config.chinese_levels {
            entry.level.chinese_name()
        } else {
            entry.level.english_name()
        };

        let mut output = String::new();

        if config.show_timestamps {
            let time_str = format_timestamp(entry.timestamp);
            output.push_str(&format!("{} ", time_str));
        }

        output.push_str(&format!("[{}] ", level_name));
        output.push_str(&entry.message);

        if config.show_location {
            if let Some(file) = &entry.file {
                output.push_str(&format!(" ({}:{})", file, entry.line.unwrap_or(0)));
            }
        }

        println!("{}", output);
    }

    /// Write log entry to file
    fn write_to_file(&self, entry: &LogEntry, config: &DebugConfig) -> RuntimeResult<()> {
        let log_path = config.log_file_path.as_ref().ok_or_else(|| {
            RuntimeError::debug_error(
                "日志文件路径未配置".to_string(),
                "日志文件路径未配置".to_string(),
            )
        })?;

        let level_name = if config.chinese_levels {
            entry.level.chinese_name()
        } else {
            entry.level.english_name()
        };

        let time_str = format_timestamp(entry.timestamp);
        let log_line = format!("{} [{}] {}\n", time_str, level_name, entry.message);

        // For simplicity, we'll append to file
        use std::fs::OpenOptions;
        use std::io::Write;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)
            .map_err(|e| {
                RuntimeError::debug_error(
                    format!("打开日志文件失败: {}", e),
                    "打开日志文件失败".to_string(),
                )
            })?;

        file.write_all(log_line.as_bytes()).map_err(|e| {
            RuntimeError::debug_error(
                format!("写入日志文件失败: {}", e),
                "写入日志文件失败".to_string(),
            )
        })?;

        Ok(())
    }

    /// Get all log entries
    pub fn get_log_entries(&self) -> RuntimeResult<Vec<LogEntry>> {
        let entries = self.log_entries.lock().unwrap();
        Ok(entries.clone())
    }

    /// Get log entries filtered by level
    pub fn get_log_entries_by_level(&self, level: LogLevel) -> RuntimeResult<Vec<LogEntry>> {
        let entries = self.log_entries.lock().unwrap();
        let filtered: Vec<LogEntry> = entries
            .iter()
            .filter(|entry| entry.level >= level)
            .cloned()
            .collect();
        Ok(filtered)
    }

    /// Get performance metrics
    pub fn get_metrics(&self) -> RuntimeResult<Vec<PerformanceMetric>> {
        let metrics = self.metrics.lock().unwrap();
        Ok(metrics.clone())
    }

    /// Get statistics
    pub fn get_statistics(&self) -> RuntimeResult<HashMap<LogLevel, usize>> {
        let stats = self.statistics.lock().unwrap();
        Ok(stats.clone())
    }

    /// Clear all logs
    pub fn clear_logs(&self) -> RuntimeResult<()> {
        let mut entries = self.log_entries.lock().unwrap();
        entries.clear();
        Ok(())
    }

    /// Clear all metrics
    pub fn clear_metrics(&self) -> RuntimeResult<()> {
        let mut metrics = self.metrics.lock().unwrap();
        metrics.clear();
        Ok(())
    }

    /// Clear all statistics
    pub fn clear_statistics(&self) -> RuntimeResult<()> {
        let mut stats = self.statistics.lock().unwrap();
        stats.clear();
        Ok(())
    }

    /// Clear all data
    pub fn clear_all(&self) -> RuntimeResult<()> {
        self.clear_logs()?;
        self.clear_metrics()?;
        self.clear_statistics()?;
        Ok(())
    }

    /// Get configuration
    pub fn get_config(&self) -> DebugConfig {
        self.config.lock().unwrap().clone()
    }

    /// Update configuration
    pub fn update_config(&self, config: DebugConfig) -> RuntimeResult<()> {
        *self.config.lock().unwrap() = config;
        Ok(())
    }

    /// Set minimum log level
    pub fn set_min_level(&self, level: LogLevel) -> RuntimeResult<()> {
        self.config.lock().unwrap().min_level = level;
        Ok(())
    }

    /// Enable/disable colored output
    pub fn set_colored_output(&self, enabled: bool) -> RuntimeResult<()> {
        self.config.lock().unwrap().colored_output = enabled;
        Ok(())
    }

    /// Enable/disable debug mode
    pub fn set_debug_enabled(&self, enabled: bool) -> RuntimeResult<()> {
        self.config.lock().unwrap().debug_enabled = enabled;
        Ok(())
    }
}

impl Default for DebugModule {
    fn default() -> Self {
        Self::new()
    }
}

/// Debug information structure
#[derive(Debug, Clone)]
pub struct DebugInfo {
    /// Current debug configuration
    pub config: DebugConfig,
    /// Total log entries created
    pub total_entries: u64,
    /// Log entries by level
    pub entries_by_level: std::collections::HashMap<LogLevel, usize>,
    /// Memory usage statistics
    pub memory_usage: u64,
    /// Uptime in seconds
    pub uptime_seconds: u64,
    /// Last error message
    pub last_error: Option<String>,
    /// Performance metrics
    pub performance_metrics: std::collections::HashMap<String, f64>,
}

/// Format timestamp for display
fn format_timestamp(timestamp: u64) -> String {
    let datetime = std::time::UNIX_EPOCH + std::time::Duration::from_secs(timestamp);

    // Simple formatting - in a real implementation you'd use chrono or similar
    format!("{}", timestamp)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_module_creation() {
        let debug = DebugModule::new();
        let config = debug.get_config();
        assert_eq!(config.min_level, LogLevel::Info);
        assert!(config.colored_output);
        assert!(config.show_timestamps);
    }

    #[test]
    fn test_log_levels() {
        let debug = DebugModule::new();

        // These should work without panicking
        let _ = debug.trace("trace message");
        let _ = debug.debug("debug message");
        let _ = debug.info("info message");
        let _ = debug.warning("warning message");
        let _ = debug.error("error message");
        let _ = debug.fatal("fatal message");
    }

    #[test]
    fn test_performance_timing() {
        let debug = DebugModule::new();

        // Test timer
        debug.start_timer("test_timer").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        let duration = debug.end_timer("test_timer").unwrap();

        // Should be at least 10ms
        assert!(duration >= 10000); // 10ms in microseconds

        // Test measurement
        let result = debug
            .measure("test_measurement", || {
                std::thread::sleep(std::time::Duration::from_millis(5));
                42
            })
            .unwrap();

        assert_eq!(result, 42);
    }

    #[test]
    fn test_configuration() {
        let debug = DebugModule::new();

        // Update configuration
        let new_config = DebugConfig {
            min_level: LogLevel::Debug,
            colored_output: false,
            ..DebugConfig::default()
        };

        debug.update_config(new_config).unwrap();

        let config = debug.get_config();
        assert_eq!(config.min_level, LogLevel::Debug);
        assert!(!config.colored_output);
    }

    #[test]
    fn test_assertion() {
        let debug = DebugModule::new();

        // True assertion should pass
        assert!(debug.assert(true, "This should pass").is_ok());

        // False assertion with debug disabled should pass
        assert!(debug
            .assert(false, "This should pass when debug disabled")
            .is_ok());

        // Enable debug and test failing assertion
        debug.set_debug_enabled(true).unwrap();
        assert!(debug.assert(false, "This should fail").is_err());
    }

    #[test]
    fn test_dump() {
        let debug = DebugModule::new();

        // Should not panic
        let _ = debug.dump("test_var", &123);
        let _ = debug.dump("test_string", &"hello");
        let _ = debug.dump("test_vector", &vec![1, 2, 3]);
    }

    #[test]
    fn test_statistics() {
        let debug = DebugModule::new();

        // Log some messages
        let _ = debug.info("info message");
        let _ = debug.warning("warning message");
        let _ = debug.error("error message");

        // Get statistics
        let stats = debug.get_statistics().unwrap();
        assert_eq!(stats.get(&LogLevel::Info), Some(&1));
        assert_eq!(stats.get(&LogLevel::Warning), Some(&1));
        assert_eq!(stats.get(&LogLevel::Error), Some(&1));

        // Clear statistics
        debug.clear_statistics().unwrap();
        let cleared_stats = debug.get_statistics().unwrap();
        assert!(cleared_stats.is_empty());
    }

    #[test]
    fn test_log_levels_ordering() {
        assert!(LogLevel::Trace < LogLevel::Debug);
        assert!(LogLevel::Debug < LogLevel::Info);
        assert!(LogLevel::Info < LogLevel::Warning);
        assert!(LogLevel::Warning < LogLevel::Error);
        assert!(LogLevel::Error < LogLevel::Fatal);
    }

    #[test]
    fn test_log_level_names() {
        assert_eq!(LogLevel::Info.chinese_name(), "信息");
        assert_eq!(LogLevel::Info.english_name(), "INFO");
        assert_eq!(LogLevel::Error.chinese_name(), "错误");
        assert_eq!(LogLevel::Error.english_name(), "ERROR");
    }
}
