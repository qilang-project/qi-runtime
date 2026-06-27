//! I/O Interface Module
//!
//! This module provides a unified interface for all I/O operations
//! including file system, network, and standard I/O with comprehensive
//! Chinese language support and cross-platform compatibility.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::filesystem::FileSystemInterface;
use super::http::NetworkInterface;
use super::stdio::{ConsoleInterface, StandardIo};
use crate::{RuntimeError, RuntimeResult};

/// Unified I/O interface that provides access to all I/O functionality
#[derive(Debug)]
pub struct IoInterface {
    /// File system interface
    filesystem: Arc<Mutex<FileSystemInterface>>,
    /// Network interface
    network: Arc<Mutex<NetworkInterface>>,
    /// Standard I/O interface
    stdio: Arc<Mutex<StandardIo>>,
    /// Console interface
    console: Arc<Mutex<ConsoleInterface>>,
    /// I/O statistics
    stats: Arc<Mutex<IoStats>>,
    /// Configuration
    config: Arc<Mutex<IoConfig>>,
}

/// I/O configuration
#[derive(Debug, Clone)]
pub struct IoConfig {
    /// Default buffer size for I/O operations
    pub default_buffer_size: usize,
    /// Default timeout for operations
    pub default_timeout: Duration,
    /// Enable caching
    pub enable_caching: bool,
    /// Cache size limit
    pub cache_size_limit: usize,
    /// Enable Chinese language support
    pub chinese_support: bool,
    /// Enable colored output
    pub colored_output: bool,
    /// Default file encoding
    pub default_encoding: String,
    /// Network configuration
    pub network_config: NetworkConfig,
    /// Enable async operations
    pub enable_async: bool,
}

/// Network configuration
#[derive(Debug, Clone)]
pub struct NetworkConfig {
    /// Default request timeout
    pub request_timeout: Duration,
    /// Maximum concurrent connections
    pub max_connections: usize,
    /// Enable connection pooling
    pub connection_pooling: bool,
    /// Default user agent
    pub user_agent: String,
    /// Enable HTTPS verification
    pub verify_https: bool,
}

impl Default for IoConfig {
    fn default() -> Self {
        Self {
            default_buffer_size: 8192,
            default_timeout: Duration::from_secs(30),
            enable_caching: true,
            cache_size_limit: 1000,
            chinese_support: true,
            colored_output: true,
            default_encoding: "utf-8".to_string(),
            network_config: NetworkConfig::default(),
            enable_async: false,
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            request_timeout: Duration::from_secs(30),
            max_connections: 10,
            connection_pooling: true,
            user_agent: "qi-runtime/1.0".to_string(),
            verify_https: true,
        }
    }
}

/// Extended I/O statistics
#[derive(Debug, Clone, Default)]
pub struct IoStats {
    /// Total I/O operations
    pub total_operations: u64,
    /// File operations
    pub file_operations: u64,
    /// Network operations
    pub network_operations: u64,
    /// Standard I/O operations
    pub stdio_operations: u64,
    /// Total bytes read
    pub total_bytes_read: u64,
    /// Total bytes written
    pub total_bytes_written: u64,
    /// Cache hits
    pub cache_hits: u64,
    /// Cache misses
    pub cache_misses: u64,
    /// Operations by type
    pub operations_by_type: HashMap<String, u64>,
    /// Average operation time (milliseconds)
    pub avg_operation_time_ms: f64,
    /// Last operation timestamp
    pub last_operation_timestamp: Option<u64>,
}

/// I/O operation context
#[derive(Debug, Clone)]
pub struct IoOperation {
    /// Operation ID
    pub id: String,
    /// Operation type
    pub operation_type: String,
    /// Resource path/URL
    pub resource: String,
    /// Timestamp
    pub timestamp: u64,
    /// Duration in milliseconds
    pub duration_ms: f64,
    /// Bytes transferred
    pub bytes_transferred: u64,
    /// Success status
    pub success: bool,
    /// Error message if failed
    pub error_message: Option<String>,
    /// Additional metadata
    pub metadata: HashMap<String, String>,
}

impl IoInterface {
    /// Create new I/O interface
    pub fn new() -> RuntimeResult<Self> {
        let config = IoConfig::default();

        let filesystem = FileSystemInterface::new(config.default_buffer_size)?;
        let network = NetworkInterface::new()?;
        let stdio = StandardIo::new();
        let console = ConsoleInterface::new();

        Ok(Self {
            filesystem: Arc::new(Mutex::new(filesystem)),
            network: Arc::new(Mutex::new(network)),
            stdio: Arc::new(Mutex::new(stdio)),
            console: Arc::new(Mutex::new(console)),
            stats: Arc::new(Mutex::new(IoStats::default())),
            config: Arc::new(Mutex::new(config)),
        })
    }

    /// Create I/O interface with custom configuration
    pub fn with_config(config: IoConfig) -> RuntimeResult<Self> {
        let filesystem = FileSystemInterface::new(config.default_buffer_size)?;
        let network = NetworkInterface::new()?;
        let stdio = StandardIo::new();
        let console = ConsoleInterface::new();

        Ok(Self {
            filesystem: Arc::new(Mutex::new(filesystem)),
            network: Arc::new(Mutex::new(network)),
            stdio: Arc::new(Mutex::new(stdio)),
            console: Arc::new(Mutex::new(console)),
            stats: Arc::new(Mutex::new(IoStats::default())),
            config: Arc::new(Mutex::new(config)),
        })
    }

    /// File system operations

    /// Read file contents as string
    pub fn read_file(&self, path: &str) -> RuntimeResult<String> {
        let start_time = Instant::now();
        let operation_id = self.generate_operation_id("read_file");

        let result = {
            let fs = self.filesystem.lock().unwrap();
            fs.read_file_string(path)
        };

        let duration = start_time.elapsed();
        self.record_operation(&IoOperation {
            id: operation_id,
            operation_type: "read_file".to_string(),
            resource: path.to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_ms: duration.as_millis() as f64,
            bytes_transferred: result.as_ref().map(|s| s.len() as u64).unwrap_or(0),
            success: result.is_ok(),
            error_message: result.as_ref().err().map(|e| e.to_string()),
            metadata: HashMap::new(),
        });

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "文件读取失败".to_string()))
    }

    /// Write string to file
    pub fn write_file(&self, path: &str, content: &str) -> RuntimeResult<()> {
        let start_time = Instant::now();
        let operation_id = self.generate_operation_id("write_file");

        let result = {
            let fs = self.filesystem.lock().unwrap();
            fs.write_file_string(path, content)
        };

        let duration = start_time.elapsed();
        self.record_operation(&IoOperation {
            id: operation_id,
            operation_type: "write_file".to_string(),
            resource: path.to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_ms: duration.as_millis() as f64,
            bytes_transferred: content.len() as u64,
            success: result.is_ok(),
            error_message: result.as_ref().err().map(|e| e.to_string()),
            metadata: HashMap::new(),
        });

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "文件写入失败".to_string()))
    }

    /// Append string to file
    pub fn append_file(&self, path: &str, content: &str) -> RuntimeResult<()> {
        let start_time = Instant::now();
        let operation_id = self.generate_operation_id("append_file");

        let result = {
            let fs = self.filesystem.lock().unwrap();
            fs.append_file_string(path, content)
        };

        let duration = start_time.elapsed();
        self.record_operation(&IoOperation {
            id: operation_id,
            operation_type: "append_file".to_string(),
            resource: path.to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_ms: duration.as_millis() as f64,
            bytes_transferred: content.len() as u64,
            success: result.is_ok(),
            error_message: result.as_ref().err().map(|e| e.to_string()),
            metadata: HashMap::new(),
        });

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "文件追加失败".to_string()))
    }

    /// Delete file
    pub fn delete_file(&self, path: &str) -> RuntimeResult<()> {
        let result = {
            let fs = self.filesystem.lock().unwrap();
            fs.delete_file(path)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "文件删除失败".to_string()))
    }

    /// Check if file exists
    pub fn file_exists(&self, path: &str) -> bool {
        let fs = self.filesystem.lock().unwrap();
        fs.file_exists(path)
    }

    /// Get file size
    pub fn file_size(&self, path: &str) -> RuntimeResult<u64> {
        let result = {
            let fs = self.filesystem.lock().unwrap();
            fs.file_size(path)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "获取文件大小失败".to_string()))
    }

    /// List directory contents
    pub fn list_directory(&self, path: &str) -> RuntimeResult<Vec<String>> {
        let result = {
            let fs = self.filesystem.lock().unwrap();
            fs.list_directory(path).map(|paths| {
                paths
                    .into_iter()
                    .filter_map(|p| p.to_str().map(|s| s.to_string()))
                    .collect()
            })
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "列出目录内容失败".to_string()))
    }

    /// Network operations

    /// Make HTTP GET request
    pub fn http_get(&self, url: &str) -> RuntimeResult<String> {
        let start_time = Instant::now();
        let operation_id = self.generate_operation_id("http_get");

        let config = self.config.lock().unwrap();
        let request = super::http::HttpRequest::get(url.to_string())
            .with_timeout(config.network_config.request_timeout);

        let result = {
            let network = self.network.lock().unwrap();
            network.make_request(&request).and_then(|response| {
                String::from_utf8(response.body).map_err(|e| super::IoError::EncodingError {
                    message: format!("HTTP响应编码错误: {}", e),
                })
            })
        };

        let duration = start_time.elapsed();
        self.record_operation(&IoOperation {
            id: operation_id,
            operation_type: "http_get".to_string(),
            resource: url.to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_ms: duration.as_millis() as f64,
            bytes_transferred: result.as_ref().map(|s| s.len() as u64).unwrap_or(0),
            success: result.is_ok(),
            error_message: result.as_ref().err().map(|e| e.to_string()),
            metadata: HashMap::new(),
        });

        result.map_err(|e| RuntimeError::network_error(e.to_string(), "HTTP请求失败".to_string()))
    }

    /// Make HTTP POST request
    pub fn http_post(&self, url: &str, body: &str) -> RuntimeResult<String> {
        let start_time = Instant::now();
        let operation_id = self.generate_operation_id("http_post");

        let config = self.config.lock().unwrap();
        let request = super::http::HttpRequest::post(url.to_string(), body.as_bytes().to_vec())
            .with_timeout(config.network_config.request_timeout);

        let result = {
            let network = self.network.lock().unwrap();
            network.make_request(&request).and_then(|response| {
                String::from_utf8(response.body).map_err(|e| super::IoError::EncodingError {
                    message: format!("HTTP响应编码错误: {}", e),
                })
            })
        };

        let duration = start_time.elapsed();
        self.record_operation(&IoOperation {
            id: operation_id,
            operation_type: "http_post".to_string(),
            resource: url.to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_ms: duration.as_millis() as f64,
            bytes_transferred: result.as_ref().map(|s| s.len() as u64).unwrap_or(0),
            success: result.is_ok(),
            error_message: result.as_ref().err().map(|e| e.to_string()),
            metadata: HashMap::new(),
        });

        result.map_err(|e| RuntimeError::network_error(e.to_string(), "HTTP请求失败".to_string()))
    }

    /// Standard I/O operations

    /// Print to standard output
    pub fn print(&self, text: &str) -> RuntimeResult<()> {
        let start_time = Instant::now();
        let operation_id = self.generate_operation_id("print");

        let result = {
            let mut stdio = self.stdio.lock().unwrap();
            stdio.print(text)
        };

        let duration = start_time.elapsed();
        self.record_operation(&IoOperation {
            id: operation_id,
            operation_type: "print".to_string(),
            resource: "stdout".to_string(),
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            duration_ms: duration.as_millis() as f64,
            bytes_transferred: text.len() as u64,
            success: result.is_ok(),
            error_message: result.as_ref().err().map(|e| e.to_string()),
            metadata: HashMap::new(),
        });

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "标准输出失败".to_string()))
    }

    /// Print to standard output with newline
    pub fn println(&self, text: &str) -> RuntimeResult<()> {
        let result = {
            let mut stdio = self.stdio.lock().unwrap();
            stdio.println(text)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "标准输出失败".to_string()))
    }

    /// Print integer to standard output with newline
    pub fn println_int(&self, value: i64) -> RuntimeResult<()> {
        let result = {
            let mut stdio = self.stdio.lock().unwrap();
            stdio.println_int(value)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "标准输出失败".to_string()))
    }

    /// Print float to standard output with newline
    pub fn println_float(&self, value: f64) -> RuntimeResult<()> {
        let result = {
            let mut stdio = self.stdio.lock().unwrap();
            stdio.println_float(value)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "标准输出失败".to_string()))
    }

    /// Read line from standard input
    pub fn read_line(&self) -> RuntimeResult<String> {
        let result = {
            let mut stdio = self.stdio.lock().unwrap();
            stdio.read_line()
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "标准输入失败".to_string()))
    }

    /// Print to standard error
    pub fn eprint(&self, text: &str) -> RuntimeResult<()> {
        let result = {
            let mut stdio = self.stdio.lock().unwrap();
            stdio.eprint(text)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "标准错误输出失败".to_string()))
    }

    /// Print to standard error with newline
    pub fn eprintln(&self, text: &str) -> RuntimeResult<()> {
        let result = {
            let mut stdio = self.stdio.lock().unwrap();
            stdio.eprintln(text)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "标准错误输出失败".to_string()))
    }

    /// Console operations

    /// Print colored text
    pub fn print_color(&self, text: &str, color: &str) -> RuntimeResult<()> {
        let result = {
            let mut console = self.console.lock().unwrap();
            // Map string color to console color enum
            let console_color = self.map_string_to_color(color)?;
            console.print_color(text, console_color)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "控制台输出失败".to_string()))
    }

    /// Print success message
    pub fn print_success(&self, text: &str) -> RuntimeResult<()> {
        let result = {
            let mut console = self.console.lock().unwrap();
            console.print_success(text)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "控制台输出失败".to_string()))
    }

    /// Print error message
    pub fn print_error(&self, text: &str) -> RuntimeResult<()> {
        let result = {
            let mut console = self.console.lock().unwrap();
            console.print_error(text)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "控制台输出失败".to_string()))
    }

    /// Print warning message
    pub fn print_warning(&self, text: &str) -> RuntimeResult<()> {
        let result = {
            let mut console = self.console.lock().unwrap();
            console.print_warning(text)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "控制台输出失败".to_string()))
    }

    /// Print info message
    pub fn print_info(&self, text: &str) -> RuntimeResult<()> {
        let result = {
            let mut console = self.console.lock().unwrap();
            console.print_info(text)
        };

        result.map_err(|e| RuntimeError::io_error(e.to_string(), "控制台输出失败".to_string()))
    }

    /// Get I/O statistics
    pub fn get_io_stats(&self) -> RuntimeResult<IoStats> {
        let stats = self.stats.lock().unwrap();
        Ok(stats.clone())
    }

    /// Get configuration
    pub fn get_config(&self) -> RuntimeResult<IoConfig> {
        let config = self.config.lock().unwrap();
        Ok(config.clone())
    }

    /// Update configuration
    pub fn update_config(&self, config: IoConfig) -> RuntimeResult<()> {
        *self.config.lock().unwrap() = config;
        Ok(())
    }

    /// Reset statistics
    pub fn reset_stats(&self) -> RuntimeResult<()> {
        let mut stats = self.stats.lock().unwrap();
        *stats = IoStats::default();
        Ok(())
    }

    /// Flush all buffers
    pub fn flush_all(&self) -> RuntimeResult<()> {
        {
            let mut stdio = self.stdio.lock().unwrap();
            stdio.flush_all()?;
        }

        // Console interface typically doesn't need separate flushing

        Ok(())
    }

    /// Initialize the I/O interface
    pub fn initialize(&self) -> RuntimeResult<()> {
        // Initialize file system interface
        {
            let mut fs = self.filesystem.lock().unwrap();
            fs.initialize()?;
        }

        // Reset statistics
        let mut stats = self.stats.lock().unwrap();
        *stats = IoStats::default();

        Ok(())
    }

    /// Private helper methods

    fn generate_operation_id(&self, operation_type: &str) -> String {
        format!(
            "{}_{}",
            operation_type,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        )
    }

    fn record_operation(&self, operation: &IoOperation) {
        let mut stats = self.stats.lock().unwrap();

        stats.total_operations += 1;
        stats.last_operation_timestamp = Some(operation.timestamp);

        // Update operation type counts
        match operation.operation_type.as_str() {
            op if op.starts_with("read_file")
                || op.starts_with("write_file")
                || op.starts_with("append_file") =>
            {
                stats.file_operations += 1;
            }
            op if op.starts_with("http_") => {
                stats.network_operations += 1;
            }
            op if op.starts_with("print") || op.starts_with("read_line") => {
                stats.stdio_operations += 1;
            }
            _ => {}
        }

        *stats
            .operations_by_type
            .entry(operation.operation_type.clone())
            .or_insert(0) += 1;

        // Update byte counts
        stats.total_bytes_read += if operation.operation_type.contains("read")
            || operation.operation_type.contains("get")
        {
            operation.bytes_transferred
        } else {
            0
        };
        stats.total_bytes_written += if operation.operation_type.contains("write")
            || operation.operation_type.contains("post")
            || operation.operation_type.contains("print")
        {
            operation.bytes_transferred
        } else {
            0
        };

        // Update average operation time
        if stats.total_operations > 0 {
            let total_time = stats.avg_operation_time_ms * (stats.total_operations - 1) as f64
                + operation.duration_ms;
            stats.avg_operation_time_ms = total_time / stats.total_operations as f64;
        }
    }

    fn map_string_to_color(&self, color_str: &str) -> RuntimeResult<super::stdio::ConsoleColor> {
        match color_str.to_lowercase().as_str() {
            "red" => Ok(super::stdio::ConsoleColor::Red),
            "green" => Ok(super::stdio::ConsoleColor::Green),
            "yellow" => Ok(super::stdio::ConsoleColor::Yellow),
            "blue" => Ok(super::stdio::ConsoleColor::Blue),
            "magenta" | "purple" => Ok(super::stdio::ConsoleColor::Magenta),
            "cyan" => Ok(super::stdio::ConsoleColor::Cyan),
            "white" => Ok(super::stdio::ConsoleColor::White),
            "black" => Ok(super::stdio::ConsoleColor::Black),
            _ => Ok(super::stdio::ConsoleColor::White), // Default to white
        }
    }
}

impl Default for IoInterface {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_interface_creation() {
        let io_interface = IoInterface::new();
        assert!(io_interface.is_ok());
    }

    #[test]
    fn test_io_interface_config() {
        let io_interface = IoInterface::new().unwrap();
        let config = io_interface.get_config().unwrap();
        assert_eq!(config.default_buffer_size, 8192);
        assert!(config.chinese_support);
        assert!(config.colored_output);
    }

    #[test]
    fn test_io_stats() {
        let io_interface = IoInterface::new().unwrap();
        let stats = io_interface.get_io_stats().unwrap();
        assert_eq!(stats.total_operations, 0);
        assert_eq!(stats.file_operations, 0);
        assert_eq!(stats.network_operations, 0);
        assert_eq!(stats.stdio_operations, 0);
    }

    #[test]
    fn test_print_operations() {
        let io_interface = IoInterface::new().unwrap();

        // These should work without panicking
        let _ = io_interface.print("Hello");
        let _ = io_interface.println("Hello World");
        let _ = io_interface.print_success("Success message");
        let _ = io_interface.print_error("Error message");
        let _ = io_interface.print_warning("Warning message");
        let _ = io_interface.print_info("Info message");
    }

    #[test]
    fn test_colored_print() {
        let io_interface = IoInterface::new().unwrap();

        // Test different colors
        let _ = io_interface.print_color("Red text", "red");
        let _ = io_interface.print_color("Green text", "green");
        let _ = io_interface.print_color("Blue text", "blue");
        let _ = io_interface.print_color("Unknown color", "unknown"); // Should default to white
    }

    #[test]
    fn test_config_update() {
        let io_interface = IoInterface::new().unwrap();

        let mut new_config = IoConfig::default();
        new_config.default_buffer_size = 4096;
        new_config.chinese_support = false;

        let result = io_interface.update_config(new_config);
        assert!(result.is_ok());

        let updated_config = io_interface.get_config().unwrap();
        assert_eq!(updated_config.default_buffer_size, 4096);
        assert!(!updated_config.chinese_support);
    }

    #[test]
    fn test_stats_reset() {
        let io_interface = IoInterface::new().unwrap();

        // Perform some operation to generate stats
        let _ = io_interface.print("Test");

        let stats = io_interface.get_io_stats().unwrap();
        assert!(stats.total_operations > 0);

        // Reset stats
        let _ = io_interface.reset_stats();

        let reset_stats = io_interface.get_io_stats().unwrap();
        assert_eq!(reset_stats.total_operations, 0);
    }
}
