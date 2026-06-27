//! I/O Operations Subsystem
//!
//! This module provides synchronous I/O operations for file system access,
//! network operations, and standard I/O with comprehensive Chinese language support.

pub mod file;
pub mod filesystem;
pub mod h2_ffi;
pub mod http;
pub mod http_ffi;
pub mod interface;
pub mod io_ffi;
pub mod network_ffi;
pub mod stdio;
pub mod tls_ffi;
pub mod websocket_ffi;

// Re-export main components
pub use file::{文件操作, 文件模块};
pub use filesystem::{FileEncoding, FileOperation, FileSystemInterface};
pub use http::{
    HttpClient, HttpRequest, HttpResponse, NetworkInterface, TcpManager, TimeoutManager,
};
pub use interface::{IoConfig, IoInterface, IoOperation, IoStats, NetworkConfig};

// Create NetworkManager as TcpManager for compatibility
pub type NetworkManager = TcpManager;
pub use stdio::{ConsoleInterface, StandardIo};

/// I/O operation result type
pub type IoResult<T> = Result<T, IoError>;

/// I/O operation errors
#[derive(Debug, thiserror::Error)]
pub enum IoError {
    #[error("文件操作失败: {path} - {message}")]
    FileOperationFailed { path: String, message: String },

    #[error("网络操作失败: {endpoint} - {message}")]
    NetworkOperationFailed { endpoint: String, message: String },

    #[error("编码错误: {message}")]
    EncodingError { message: String },

    #[error("权限被拒绝: {resource}")]
    PermissionDenied { resource: String },

    #[error("资源未找到: {resource}")]
    ResourceNotFound { resource: String },

    #[error("I/O 超时: 操作超时 ({timeout_ms}ms)")]
    Timeout { timeout_ms: u64 },

    #[error("连接被拒绝: {address}:{port}")]
    ConnectionRefused { address: String, port: u16 },

    #[error("系统I/O错误: {0}")]
    SystemIoError(#[from] std::io::Error),
}

/// I/O operation timeout configuration
#[derive(Debug, Clone, Copy)]
pub struct IoTimeout {
    /// Timeout in milliseconds
    pub timeout_ms: u64,
    /// Whether to timeout on read operations
    pub read_timeout: bool,
    /// Whether to timeout on write operations
    pub write_timeout: bool,
}

impl Default for IoTimeout {
    fn default() -> Self {
        Self {
            timeout_ms: 30000, // 30 seconds
            read_timeout: true,
            write_timeout: true,
        }
    }
}

impl IoTimeout {
    /// Create new timeout configuration
    pub fn new(timeout_ms: u64) -> Self {
        Self {
            timeout_ms,
            read_timeout: true,
            write_timeout: true,
        }
    }

    /// Get timeout as Duration
    pub fn duration(&self) -> std::time::Duration {
        std::time::Duration::from_millis(self.timeout_ms)
    }

    /// Set read timeout
    pub fn with_read_timeout(mut self, enabled: bool) -> Self {
        self.read_timeout = enabled;
        self
    }

    /// Set write timeout
    pub fn with_write_timeout(mut self, enabled: bool) -> Self {
        self.write_timeout = enabled;
        self
    }
}

/// I/O operation statistics
#[derive(Debug, Clone, Default)]
pub struct IoStatistics {
    /// Number of successful read operations
    pub successful_reads: u64,
    /// Number of successful write operations
    pub successful_writes: u64,
    /// Number of failed operations
    pub failed_operations: u64,
    /// Total bytes read
    pub bytes_read: u64,
    /// Total bytes written
    pub bytes_written: u64,
    /// Number of network operations
    pub network_operations: u64,
    /// Average read time in milliseconds
    pub avg_read_time_ms: f64,
    /// Average write time in milliseconds
    pub avg_write_time_ms: f64,
}

impl IoStatistics {
    /// Create new I/O statistics
    pub fn new() -> Self {
        Self::default()
    }

    /// Record successful read operation
    pub fn record_read(&mut self, bytes: u64, time_ms: f64) {
        self.successful_reads += 1;
        self.bytes_read += bytes;
        self.update_avg_read_time(time_ms);
    }

    /// Record successful write operation
    pub fn record_write(&mut self, bytes: u64, time_ms: f64) {
        self.successful_writes += 1;
        self.bytes_written += bytes;
        self.update_avg_write_time(time_ms);
    }

    /// Record failed operation
    pub fn record_failure(&mut self) {
        self.failed_operations += 1;
    }

    /// Get total operations
    pub fn total_operations(&self) -> u64 {
        self.successful_reads + self.successful_writes + self.failed_operations
    }

    /// Get success rate
    pub fn success_rate(&self) -> f64 {
        let total = self.total_operations();
        if total == 0 {
            1.0
        } else {
            (self.successful_reads + self.successful_writes) as f64 / total as f64
        }
    }

    /// Update average read time
    fn update_avg_read_time(&mut self, time_ms: f64) {
        if self.successful_reads == 1 {
            self.avg_read_time_ms = time_ms;
        } else {
            self.avg_read_time_ms = (self.avg_read_time_ms * (self.successful_reads - 1) as f64
                + time_ms)
                / self.successful_reads as f64;
        }
    }

    /// Update average write time
    fn update_avg_write_time(&mut self, time_ms: f64) {
        if self.successful_writes == 1 {
            self.avg_write_time_ms = time_ms;
        } else {
            self.avg_write_time_ms = (self.avg_write_time_ms * (self.successful_writes - 1) as f64
                + time_ms)
                / self.successful_writes as f64;
        }
    }

    /// Reset statistics
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Increment network operations (legacy method)
    pub fn increment_network_operations(&mut self) {
        self.network_operations += 1;
    }

    /// Record network operation (legacy method)
    pub fn record_network_operations(&mut self) {
        self.network_operations += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_timeout() {
        let timeout = IoTimeout::new(5000);
        assert_eq!(timeout.timeout_ms, 5000);
        assert!(timeout.read_timeout);
        assert!(timeout.write_timeout);

        let duration = timeout.duration();
        assert_eq!(duration.as_millis(), 5000);

        let custom_timeout = timeout.with_read_timeout(false);
        assert!(!custom_timeout.read_timeout);
        assert!(custom_timeout.write_timeout);
    }

    #[test]
    fn test_io_statistics() {
        let mut stats = IoStatistics::new();
        assert_eq!(stats.total_operations(), 0);
        assert_eq!(stats.success_rate(), 1.0);

        stats.record_read(1024, 10.5);
        assert_eq!(stats.successful_reads, 1);
        assert_eq!(stats.bytes_read, 1024);
        assert_eq!(stats.avg_read_time_ms, 10.5);

        stats.record_write(512, 5.0);
        assert_eq!(stats.successful_writes, 1);
        assert_eq!(stats.bytes_written, 512);
        assert_eq!(stats.avg_write_time_ms, 5.0);

        stats.record_failure();
        assert_eq!(stats.failed_operations, 1);
        assert_eq!(stats.total_operations(), 3);
        assert!(stats.success_rate() > 0.6);

        stats.reset();
        assert_eq!(stats.total_operations(), 0);
    }

    #[test]
    fn test_io_error_display() {
        let error = IoError::FileOperationFailed {
            path: "/test/file.txt".to_string(),
            message: "文件不存在".to_string(),
        };
        let message = error.to_string();
        assert!(message.contains("/test/file.txt"));
        assert!(message.contains("文件不存在"));
    }
}
