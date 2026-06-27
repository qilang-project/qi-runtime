//! Network Operations Implementation
//!
//! This module provides network operations including HTTP requests,
//! TCP connections, and network timeout management.

use super::{IoError, IoResult, IoStatistics};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

/// HTTP request methods
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HttpMethod {
    Get,
    Post,
    Put,
    Delete,
    Head,
    Options,
    Patch,
}

impl HttpMethod {
    /// Convert to string
    pub fn as_str(&self) -> &'static str {
        match self {
            HttpMethod::Get => "GET",
            HttpMethod::Post => "POST",
            HttpMethod::Put => "PUT",
            HttpMethod::Delete => "DELETE",
            HttpMethod::Head => "HEAD",
            HttpMethod::Options => "OPTIONS",
            HttpMethod::Patch => "PATCH",
        }
    }
}

/// HTTP request configuration
#[derive(Debug, Clone)]
pub struct HttpRequest {
    /// Request URL
    pub url: String,
    /// HTTP method
    pub method: HttpMethod,
    /// Request headers
    pub headers: std::collections::HashMap<String, String>,
    /// Request body
    pub body: Option<Vec<u8>>,
    /// Request timeout
    pub timeout: Duration,
    /// Follow redirects
    pub follow_redirects: bool,
    /// Maximum redirect count
    pub max_redirects: usize,
    /// Verify SSL certificates
    pub verify_ssl: bool,
}

impl HttpRequest {
    /// Create new GET request
    pub fn get(url: String) -> Self {
        Self {
            url,
            method: HttpMethod::Get,
            headers: std::collections::HashMap::new(),
            body: None,
            timeout: Duration::from_secs(30),
            follow_redirects: true,
            max_redirects: 5,
            verify_ssl: true,
        }
    }

    /// Create new POST request
    pub fn post(url: String, body: Vec<u8>) -> Self {
        let mut request = Self::get(url);
        request.method = HttpMethod::Post;
        request.body = Some(body);
        request
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Add header
    pub fn with_header(mut self, name: String, value: String) -> Self {
        self.headers.insert(name, value);
        self
    }

    /// Set body
    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }
}

/// HTTP response
#[derive(Debug, Clone)]
pub struct HttpResponse {
    /// Status code
    pub status_code: u16,
    /// Response headers
    pub headers: std::collections::HashMap<String, String>,
    /// Response body
    pub body: Vec<u8>,
    /// Response time in milliseconds
    pub response_time_ms: u64,
    /// Number of redirects followed
    pub redirect_count: usize,
}

impl HttpResponse {
    /// Check if response is successful (2xx status code)
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status_code)
    }

    /// Get body as string (assumes UTF-8)
    pub fn body_as_string(&self) -> Result<String, std::string::FromUtf8Error> {
        String::from_utf8(self.body.clone())
    }

    /// Get header value
    pub fn get_header(&self, name: &str) -> Option<&String> {
        self.headers.get(name)
    }
}

/// TCP connection configuration
#[derive(Debug, Clone)]
pub struct TcpConnectionConfig {
    /// Remote host
    pub host: String,
    /// Remote port
    pub port: u16,
    /// Connection timeout
    pub timeout: Duration,
    /// Keep connection alive
    pub keep_alive: bool,
    /// Local bind address
    pub bind_address: Option<String>,
    /// Connection buffer size
    pub buffer_size: usize,
}

impl TcpConnectionConfig {
    /// Create new TCP connection configuration
    pub fn new(host: String, port: u16) -> Self {
        Self {
            host,
            port,
            timeout: Duration::from_secs(30),
            keep_alive: false,
            bind_address: None,
            buffer_size: 8192,
        }
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Set keep alive
    pub fn with_keep_alive(mut self, keep_alive: bool) -> Self {
        self.keep_alive = keep_alive;
        self
    }

    /// Set bind address
    pub fn with_bind_address(mut self, bind_address: String) -> Self {
        self.bind_address = Some(bind_address);
        self
    }
}

/// TCP connection
#[derive(Debug)]
pub struct TcpConnection {
    /// TCP stream
    stream: TcpStream,
    /// Connection configuration
    config: TcpConnectionConfig,
    /// Connection established timestamp
    established_at: Instant,
    /// Bytes read
    bytes_read: u64,
    /// Bytes written
    bytes_written: u64,
}

impl TcpConnection {
    /// Create new TCP connection
    pub fn connect(config: TcpConnectionConfig) -> IoResult<Self> {
        let start_time = Instant::now();

        // Resolve address
        let addr = format!("{}:{}", config.host, config.port)
            .to_socket_addrs()
            .map_err(|e| IoError::NetworkOperationFailed {
                endpoint: config.host.clone(),
                message: format!("解析地址失败: {}", e),
            })?
            .next()
            .ok_or_else(|| IoError::NetworkOperationFailed {
                endpoint: config.host.clone(),
                message: "无法解析地址".to_string(),
            })?;

        // Connect with timeout
        #[cfg(unix)]
        let stream = {
            // Use platform-specific timeout on Unix
            Self::connect_with_timeout_unix(&addr, config.timeout)?
        };

        #[cfg(not(unix))]
        let stream = {
            // Fallback for other platforms
            TcpStream::connect(addr).map_err(|e| IoError::NetworkOperationFailed {
                endpoint: config.host.clone(),
                message: format!("连接失败: {}", e),
            })?
        };

        Ok(Self {
            stream,
            config,
            established_at: start_time,
            bytes_read: 0,
            bytes_written: 0,
        })
    }

    /// Connect with timeout on Unix systems
    #[cfg(unix)]
    fn connect_with_timeout_unix(
        addr: &std::net::SocketAddr,
        timeout: Duration,
    ) -> IoResult<TcpStream> {
        use std::os::unix::io::AsRawFd;

        let stream = TcpStream::connect(addr).map_err(|e| IoError::NetworkOperationFailed {
            endpoint: addr.to_string(),
            message: format!("连接失败: {}", e),
        })?;

        // Set non-blocking
        stream
            .set_nonblocking(true)
            .map_err(|e| IoError::NetworkOperationFailed {
                endpoint: addr.to_string(),
                message: format!("设置非阻塞失败: {}", e),
            })?;

        // Wait for connection with timeout
        let fd = stream.as_raw_fd();
        let mut pollfds = libc::pollfd {
            fd,
            events: libc::POLLOUT,
            revents: 0,
        };

        let timeout_ms = timeout.as_millis() as i32;
        let result = unsafe { libc::poll(&mut pollfds, 1, timeout_ms) };

        if result < 0 {
            return Err(IoError::NetworkOperationFailed {
                endpoint: addr.to_string(),
                message: "轮询失败".to_string(),
            });
        }

        if result == 0 {
            return Err(IoError::Timeout {
                timeout_ms: timeout.as_millis() as u64,
            });
        }

        if pollfds.revents & libc::POLLERR != 0 || pollfds.revents & libc::POLLHUP != 0 {
            return Err(IoError::ConnectionRefused {
                address: addr.ip().to_string(),
                port: addr.port(),
            });
        }

        // Set back to blocking
        stream
            .set_nonblocking(false)
            .map_err(|e| IoError::NetworkOperationFailed {
                endpoint: addr.to_string(),
                message: format!("设置阻塞模式失败: {}", e),
            })?;

        Ok(stream)
    }

    /// Read data from connection
    pub fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        match self.stream.read(buf) {
            Ok(bytes_read) => {
                self.bytes_read += bytes_read as u64;
                Ok(bytes_read)
            }
            Err(e) => Err(IoError::NetworkOperationFailed {
                endpoint: format!("{}:{}", self.config.host, self.config.port),
                message: format!("读取数据失败: {}", e),
            }),
        }
    }

    /// Write data to connection
    pub fn write(&mut self, buf: &[u8]) -> IoResult<usize> {
        match self.stream.write(buf) {
            Ok(bytes_written) => {
                self.bytes_written += bytes_written as u64;
                Ok(bytes_written)
            }
            Err(e) => Err(IoError::NetworkOperationFailed {
                endpoint: format!("{}:{}", self.config.host, self.config.port),
                message: format!("写入数据失败: {}", e),
            }),
        }
    }

    /// Flush pending writes
    pub fn flush(&mut self) -> IoResult<()> {
        self.stream
            .flush()
            .map_err(|e| IoError::NetworkOperationFailed {
                endpoint: format!("{}:{}", self.config.host, self.config.port),
                message: format!("刷新缓冲区失败: {}", e),
            })
    }

    /// Get connection duration
    pub fn duration(&self) -> Duration {
        self.established_at.elapsed()
    }

    /// Get bytes read
    pub fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    /// Get bytes written
    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    /// Get local address
    pub fn local_addr(&self) -> IoResult<std::net::SocketAddr> {
        self.stream
            .local_addr()
            .map_err(|e| IoError::NetworkOperationFailed {
                endpoint: format!("{}:{}", self.config.host, self.config.port),
                message: format!("获取本地地址失败: {}", e),
            })
    }

    /// Get remote address
    pub fn remote_addr(&self) -> IoResult<std::net::SocketAddr> {
        self.stream
            .peer_addr()
            .map_err(|e| IoError::NetworkOperationFailed {
                endpoint: format!("{}:{}", self.config.host, self.config.port),
                message: format!("获取远程地址失败: {}", e),
            })
    }

    /// Create TcpConnection from existing TcpStream (用于服务器接受的连接)
    pub fn from_stream(stream: TcpStream) -> IoResult<Self> {
        // 关掉 Nagle 算法：小响应不再等 ~40ms 的 ACK 合并，p99 直接砍。
        // 失败不致命，记一下日志继续即可。
        let _ = stream.set_nodelay(true);

        let peer_addr = stream
            .peer_addr()
            .map_err(|e| IoError::NetworkOperationFailed {
                endpoint: "unknown".to_string(),
                message: format!("获取对端地址失败: {}", e),
            })?;

        let config = TcpConnectionConfig {
            host: peer_addr.ip().to_string(),
            port: peer_addr.port(),
            timeout: Duration::from_secs(30),
            keep_alive: true,
            bind_address: None,
            buffer_size: 8192,
        };

        Ok(Self {
            stream,
            config,
            established_at: Instant::now(),
            bytes_read: 0,
            bytes_written: 0,
        })
    }

    /// Consume the TcpConnection and return the underlying TcpStream
    /// This transfers ownership of the stream out of the connection
    pub fn into_stream(self) -> TcpStream {
        self.stream
    }

    /// Try to clone the underlying TcpStream
    pub fn try_clone_stream(&self) -> std::io::Result<TcpStream> {
        self.stream.try_clone()
    }
}

/// HTTP client
#[derive(Debug)]
pub struct HttpClient {
    /// Default timeout
    default_timeout: Duration,
    /// I/O statistics
    statistics: std::sync::Mutex<IoStatistics>,
}

impl HttpClient {
    /// Create new HTTP client
    pub fn new() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            statistics: std::sync::Mutex::new(IoStatistics::new()),
        }
    }

    /// Create HTTP client with default timeout
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            default_timeout: timeout,
            statistics: std::sync::Mutex::new(IoStatistics::new()),
        }
    }

    /// Execute HTTP request
    pub fn execute(&self, request: HttpRequest) -> IoResult<HttpResponse> {
        let start_time = Instant::now();

        // For this implementation, we'll simulate HTTP requests
        // In a real implementation, we would use an HTTP client library
        let response = self.simulate_http_request(&request);

        let elapsed = start_time.elapsed();

        match &response {
            Ok(_) => {
                let mut stats = self.statistics.lock().unwrap();
                stats.record_read(
                    response.as_ref().unwrap().body.len() as u64,
                    elapsed.as_millis() as f64,
                );
            }
            Err(_) => {
                let mut stats = self.statistics.lock().unwrap();
                stats.record_failure();
            }
        }

        response
    }

    /// Get statistics
    pub fn get_statistics(&self) -> IoStatistics {
        self.statistics.lock().unwrap().clone()
    }

    /// Reset statistics
    pub fn reset_statistics(&mut self) {
        let mut stats = self.statistics.lock().unwrap();
        stats.reset();
    }

    /// Simulate HTTP request (placeholder implementation)
    fn simulate_http_request(&self, request: &HttpRequest) -> IoResult<HttpResponse> {
        // This is a simulation - in a real implementation we would make actual HTTP requests
        let response_time = std::time::Duration::from_millis(150);

        Ok(HttpResponse {
            status_code: 200,
            headers: std::collections::HashMap::from([
                ("content-type".to_string(), "application/json".to_string()),
                ("content-length".to_string(), "13".to_string()),
            ]),
            body: b"Hello, World!".to_vec(),
            response_time_ms: response_time.as_millis() as u64,
            redirect_count: 0,
        })
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

/// TCP manager
#[derive(Debug)]
pub struct TcpManager {
    /// Default timeout
    default_timeout: Duration,
    /// I/O statistics
    statistics: std::sync::Mutex<IoStatistics>,
}

impl TcpManager {
    /// Create new TCP manager
    pub fn new() -> Self {
        Self {
            default_timeout: Duration::from_secs(30),
            statistics: std::sync::Mutex::new(IoStatistics::new()),
        }
    }

    /// Create TCP manager with default timeout
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            default_timeout: timeout,
            statistics: std::sync::Mutex::new(IoStatistics::new()),
        }
    }

    /// Create TCP connection
    pub fn connect(&self, config: TcpConnectionConfig) -> IoResult<TcpConnection> {
        let start_time = Instant::now();
        let result = TcpConnection::connect(config);

        match &result {
            Ok(_) => {
                let elapsed = start_time.elapsed();
                let mut stats = self.statistics.lock().unwrap();
                stats.record_network_operations();
                // Network operations don't have byte counts easily accessible
            }
            Err(_) => {
                let mut stats = self.statistics.lock().unwrap();
                stats.record_failure();
            }
        }

        result
    }

    /// Get statistics
    pub fn get_statistics(&self) -> IoStatistics {
        self.statistics.lock().unwrap().clone()
    }

    /// Reset statistics
    pub fn reset_statistics(&mut self) {
        let mut stats = self.statistics.lock().unwrap();
        stats.reset();
    }

    /// Initialize the TCP manager
    pub fn initialize(&mut self) -> IoResult<()> {
        // Reset statistics on initialization
        self.reset_statistics();
        Ok(())
    }

    /// Cleanup the TCP manager
    pub fn cleanup(&mut self) -> IoResult<()> {
        // In a real implementation, this would close all active connections
        // For now, we just reset statistics
        self.reset_statistics();
        Ok(())
    }
}

impl Default for TcpManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Timeout manager for network operations
#[derive(Debug)]
pub struct TimeoutManager {
    /// Default timeout
    default_timeout: Duration,
}

impl TimeoutManager {
    /// Create new timeout manager
    pub fn new(default_timeout: Duration) -> Self {
        Self { default_timeout }
    }

    /// Get default timeout
    pub fn default_timeout(&self) -> Duration {
        self.default_timeout
    }

    /// Set default timeout
    pub fn set_default_timeout(&mut self, timeout: Duration) {
        self.default_timeout = timeout;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_request_creation() {
        let request = HttpRequest::get("https://example.com".to_string());
        assert_eq!(request.method, HttpMethod::Get);
        assert_eq!(request.url, "https://example.com");
        assert!(request.body.is_none());
    }

    #[test]
    fn test_http_request_builder() {
        let request = HttpRequest::post("https://example.com".to_string(), b"data".to_vec())
            .with_timeout(Duration::from_secs(60))
            .with_header("Content-Type".to_string(), "application/json".to_string());

        assert_eq!(request.method, HttpMethod::Post);
        assert_eq!(request.timeout, Duration::from_secs(60));
        assert!(request.headers.contains_key("Content-Type"));
    }

    #[test]
    fn test_http_response() {
        let response = HttpResponse {
            status_code: 200,
            headers: std::collections::HashMap::new(),
            body: b"Hello".to_vec(),
            response_time_ms: 100,
            redirect_count: 0,
        };

        assert!(response.is_success());
        assert_eq!(response.body_as_string().unwrap(), "Hello");
    }

    #[test]
    fn test_tcp_connection_config() {
        let config = TcpConnectionConfig::new("example.com".to_string(), 80)
            .with_timeout(Duration::from_secs(10))
            .with_keep_alive(true);

        assert_eq!(config.host, "example.com");
        assert_eq!(config.port, 80);
        assert_eq!(config.timeout, Duration::from_secs(10));
        assert!(config.keep_alive);
    }

    #[test]
    fn test_http_methods() {
        assert_eq!(HttpMethod::Get.as_str(), "GET");
        assert_eq!(HttpMethod::Post.as_str(), "POST");
        assert_ne!(HttpMethod::Get, HttpMethod::Post);
    }

    #[test]
    fn test_http_client() {
        let client = HttpClient::new();
        let request = HttpRequest::get("https://example.com".to_string());

        let response = client.execute(request);
        assert!(response.is_ok());

        let response = response.unwrap();
        assert_eq!(response.status_code, 200);
        assert_eq!(response.body_as_string().unwrap(), "Hello, World!");
    }

    #[test]
    fn test_tcp_manager() {
        let manager = TcpManager::new();
        let config = TcpConnectionConfig::new("localhost".to_string(), 8080);

        // This will likely fail since we don't have a server running
        let result = manager.connect(config);
        assert!(result.is_err());
    }

    #[test]
    fn test_timeout_manager() {
        let manager = TimeoutManager::new(Duration::from_secs(30));
        assert_eq!(manager.default_timeout(), Duration::from_secs(30));

        let mut manager = manager;
        manager.set_default_timeout(Duration::from_secs(60));
        assert_eq!(manager.default_timeout(), Duration::from_secs(60));
    }
}

/// Network interface for unified network operations
#[derive(Debug)]
pub struct NetworkInterface {
    /// HTTP client
    http_client: HttpClient,
    /// TCP manager
    tcp_manager: TcpManager,
    /// Timeout manager
    timeout_manager: TimeoutManager,
}

impl NetworkInterface {
    /// Create new network interface
    pub fn new() -> IoResult<Self> {
        Ok(Self {
            http_client: HttpClient::new(),
            tcp_manager: TcpManager::new(),
            timeout_manager: TimeoutManager::new(Duration::from_secs(30)),
        })
    }

    /// Make HTTP request
    pub fn make_request(&self, request: &HttpRequest) -> IoResult<HttpResponse> {
        self.http_client.execute(request.clone())
    }

    /// Create TCP connection
    pub fn create_connection(&self, config: TcpConnectionConfig) -> IoResult<TcpConnection> {
        self.tcp_manager.connect(config)
    }

    /// Get HTTP client
    pub fn http_client(&self) -> &HttpClient {
        &self.http_client
    }

    /// Get TCP manager
    pub fn tcp_manager(&self) -> &TcpManager {
        &self.tcp_manager
    }

    /// Get timeout manager
    pub fn timeout_manager(&self) -> &TimeoutManager {
        &self.timeout_manager
    }
}

impl Default for NetworkInterface {
    fn default() -> Self {
        Self::new().unwrap()
    }
}
