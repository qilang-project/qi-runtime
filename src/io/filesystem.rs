//! File System Interface Implementation
//!
//! This module provides file system operations with UTF-8 support,
//! Chinese language integration, and cross-platform compatibility.

use super::{IoError, IoResult, IoStatistics};
use std::fs::{File, OpenOptions};
use std::io::{BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// File character encoding
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileEncoding {
    /// UTF-8 encoding (default)
    Utf8,
    /// UTF-16 with little endian
    Utf16Le,
    /// UTF-16 with big endian
    Utf16Be,
    /// ASCII encoding
    Ascii,
    /// System default encoding
    System,
}

impl Default for FileEncoding {
    fn default() -> Self {
        Self::Utf8
    }
}

/// File operation type
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOperationType {
    /// Read operation
    Read,
    /// Write operation
    Write,
    /// Create file
    Create,
    /// Delete file
    Delete,
    /// Append to file
    Append,
    /// Read metadata
    ReadMetadata,
}

/// File operation configuration
#[derive(Debug, Clone)]
pub struct FileOperation {
    /// File path
    pub path: PathBuf,
    /// Operation type
    pub operation_type: FileOperationType,
    /// File encoding
    pub encoding: FileEncoding,
    /// Buffer size
    pub buffer_size: usize,
    /// Operation timeout
    pub timeout: Option<Duration>,
    /// Create parent directories if needed
    pub create_directories: bool,
    /// Force operation (override permissions if needed)
    pub force: bool,
}

impl FileOperation {
    /// Create new file operation
    pub fn new<P: AsRef<Path>>(path: P, operation_type: FileOperationType) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            operation_type,
            encoding: FileEncoding::default(),
            buffer_size: 8192,
            timeout: None,
            create_directories: true,
            force: false,
        }
    }

    /// Set encoding
    pub fn with_encoding(mut self, encoding: FileEncoding) -> Self {
        self.encoding = encoding;
        self
    }

    /// Set buffer size
    pub fn with_buffer_size(mut self, buffer_size: usize) -> Self {
        self.buffer_size = buffer_size;
        self
    }

    /// Set timeout
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = Some(timeout);
        self
    }

    /// Set create directories flag
    pub fn with_create_directories(mut self, create_directories: bool) -> Self {
        self.create_directories = create_directories;
        self
    }

    /// Set force flag
    pub fn with_force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }
}

/// File system interface
#[derive(Debug)]
pub struct FileSystemInterface {
    /// Default buffer size for file operations
    default_buffer_size: usize,
    /// I/O statistics
    statistics: std::sync::Mutex<IoStatistics>,
    /// Default timeout for operations
    default_timeout: Option<Duration>,
}

impl FileSystemInterface {
    /// Create new file system interface
    pub fn new(buffer_size: usize) -> IoResult<Self> {
        Ok(Self {
            default_buffer_size: buffer_size,
            statistics: std::sync::Mutex::new(IoStatistics::new()),
            default_timeout: Some(Duration::from_secs(30)),
        })
    }

    /// Initialize the file system interface
    pub fn initialize(&mut self) -> IoResult<()> {
        let mut stats = self.statistics.lock().unwrap();
        stats.reset();
        Ok(())
    }

    /// Read file contents as string with encoding support
    pub fn read_file_string<P: AsRef<Path>>(&self, path: P) -> IoResult<String> {
        let start_time = Instant::now();
        let operation = FileOperation::new(path, FileOperationType::Read)
            .with_buffer_size(self.default_buffer_size);

        let result = self.read_file_string_impl(&operation);

        match &result {
            Ok(content) => {
                let elapsed = start_time.elapsed();
                let mut stats = self.statistics.lock().unwrap();
                stats.record_read(content.len() as u64, elapsed.as_millis() as f64);
            }
            Err(_) => {
                let mut stats = self.statistics.lock().unwrap();
                stats.record_failure();
            }
        }

        result
    }

    /// Write string to file with encoding support
    pub fn write_file_string<P: AsRef<Path>>(&self, path: P, content: &str) -> IoResult<()> {
        let start_time = Instant::now();
        let operation = FileOperation::new(path, FileOperationType::Write)
            .with_buffer_size(self.default_buffer_size);

        let result = self.write_file_string_impl(&operation, content);

        match &result {
            Ok(()) => {
                let elapsed = start_time.elapsed();
                let mut stats = self.statistics.lock().unwrap();
                stats.record_write(content.len() as u64, elapsed.as_millis() as f64);
            }
            Err(_) => {
                let mut stats = self.statistics.lock().unwrap();
                stats.record_failure();
            }
        }

        result
    }

    /// Append string to file
    pub fn append_file_string<P: AsRef<Path>>(&self, path: P, content: &str) -> IoResult<()> {
        let start_time = Instant::now();
        let operation = FileOperation::new(path, FileOperationType::Append)
            .with_buffer_size(self.default_buffer_size);

        let result = self.append_file_string_impl(&operation, content);

        match &result {
            Ok(()) => {
                let elapsed = start_time.elapsed();
                let mut stats = self.statistics.lock().unwrap();
                stats.record_write(content.len() as u64, elapsed.as_millis() as f64);
            }
            Err(_) => {
                let mut stats = self.statistics.lock().unwrap();
                stats.record_failure();
            }
        }

        result
    }

    /// Delete file
    pub fn delete_file<P: AsRef<Path>>(&self, path: P) -> IoResult<()> {
        let path = path.as_ref();

        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) => Err(IoError::FileOperationFailed {
                path: path.to_string_lossy().to_string(),
                message: format!("删除文件失败: {}", e),
            }),
        }
    }

    /// Create directory
    pub fn create_directory<P: AsRef<Path>>(&self, path: P) -> IoResult<()> {
        let path = path.as_ref();

        match std::fs::create_dir_all(path) {
            Ok(()) => Ok(()),
            Err(e) => Err(IoError::FileOperationFailed {
                path: path.to_string_lossy().to_string(),
                message: format!("创建目录失败: {}", e),
            }),
        }
    }

    /// Check if file exists
    pub fn file_exists<P: AsRef<Path>>(&self, path: P) -> bool {
        path.as_ref().exists()
    }

    /// Get file metadata
    pub fn file_metadata<P: AsRef<Path>>(&self, path: P) -> IoResult<std::fs::Metadata> {
        let path = path.as_ref();

        match std::fs::metadata(path) {
            Ok(metadata) => Ok(metadata),
            Err(e) => Err(IoError::FileOperationFailed {
                path: path.to_string_lossy().to_string(),
                message: format!("获取文件元数据失败: {}", e),
            }),
        }
    }

    /// Get file size in bytes
    pub fn file_size<P: AsRef<Path>>(&self, path: P) -> IoResult<u64> {
        let metadata = self.file_metadata(path)?;
        Ok(metadata.len())
    }

    /// List directory contents
    pub fn list_directory<P: AsRef<Path>>(&self, path: P) -> IoResult<Vec<PathBuf>> {
        let path = path.as_ref();

        match std::fs::read_dir(path) {
            Ok(entries) => {
                let mut paths = Vec::new();
                for entry in entries {
                    match entry {
                        Ok(entry) => paths.push(entry.path()),
                        Err(e) => {
                            return Err(IoError::FileOperationFailed {
                                path: path.to_string_lossy().to_string(),
                                message: format!("读取目录条目失败: {}", e),
                            });
                        }
                    }
                }
                Ok(paths)
            }
            Err(e) => Err(IoError::FileOperationFailed {
                path: path.to_string_lossy().to_string(),
                message: format!("读取目录失败: {}", e),
            }),
        }
    }

    /// Get file system statistics
    pub fn get_statistics(&self) -> IoStatistics {
        self.statistics.lock().unwrap().clone()
    }

    /// Reset statistics
    pub fn reset_statistics(&mut self) {
        let mut stats = self.statistics.lock().unwrap();
        stats.reset();
    }

    /// Cleanup resources
    pub fn cleanup(&mut self) -> IoResult<()> {
        self.reset_statistics();
        Ok(())
    }

    /// Implementation of read file string
    fn read_file_string_impl(&self, operation: &FileOperation) -> IoResult<String> {
        // Create parent directories if needed
        if let Some(parent) = operation.path.parent() {
            if !parent.exists() && operation.create_directories {
                std::fs::create_dir_all(parent).map_err(|e| IoError::FileOperationFailed {
                    path: parent.to_string_lossy().to_string(),
                    message: format!("创建父目录失败: {}", e),
                })?;
            }
        }

        // Open file for reading
        let file = File::open(&operation.path).map_err(|e| IoError::FileOperationFailed {
            path: operation.path.to_string_lossy().to_string(),
            message: format!("打开文件读取失败: {}", e),
        })?;

        let mut reader = BufReader::new(file);
        let mut content = String::new();

        reader
            .read_to_string(&mut content)
            .map_err(|e| IoError::FileOperationFailed {
                path: operation.path.to_string_lossy().to_string(),
                message: format!("读取文件内容失败: {}", e),
            })?;

        // Handle encoding conversion if needed
        match operation.encoding {
            FileEncoding::Utf8 => {
                // Validate UTF-8 by checking if content can be converted to UTF-8 bytes and back
                match String::from_utf8(content.as_bytes().to_vec()) {
                    Ok(validated_content) => Ok(validated_content),
                    Err(_) => Err(IoError::EncodingError {
                        message: "文件内容不是有效的UTF-8编码".to_string(),
                    }),
                }
            }
            _ => {
                // For other encodings, return as-is for now
                // In a real implementation, we'd do proper encoding conversion
                Ok(content)
            }
        }
    }

    /// Implementation of write file string
    fn write_file_string_impl(&self, operation: &FileOperation, content: &str) -> IoResult<()> {
        // Create parent directories if needed
        if let Some(parent) = operation.path.parent() {
            if !parent.exists() && operation.create_directories {
                std::fs::create_dir_all(parent).map_err(|e| IoError::FileOperationFailed {
                    path: parent.to_string_lossy().to_string(),
                    message: format!("创建父目录失败: {}", e),
                })?;
            }
        }

        // Open file for writing
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&operation.path)
            .map_err(|e| IoError::FileOperationFailed {
                path: operation.path.to_string_lossy().to_string(),
                message: format!("打开文件写入失败: {}", e),
            })?;

        let mut writer = BufWriter::new(file);
        writer
            .write_all(content.as_bytes())
            .map_err(|e| IoError::FileOperationFailed {
                path: operation.path.to_string_lossy().to_string(),
                message: format!("写入文件内容失败: {}", e),
            })?;

        writer.flush().map_err(|e| IoError::FileOperationFailed {
            path: operation.path.to_string_lossy().to_string(),
            message: format!("刷新文件缓冲区失败: {}", e),
        })?;

        Ok(())
    }

    /// Implementation of append file string
    fn append_file_string_impl(&self, operation: &FileOperation, content: &str) -> IoResult<()> {
        // Create parent directories if needed
        if let Some(parent) = operation.path.parent() {
            if !parent.exists() && operation.create_directories {
                std::fs::create_dir_all(parent).map_err(|e| IoError::FileOperationFailed {
                    path: parent.to_string_lossy().to_string(),
                    message: format!("创建父目录失败: {}", e),
                })?;
            }
        }

        // Open file for appending
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .append(true)
            .open(&operation.path)
            .map_err(|e| IoError::FileOperationFailed {
                path: operation.path.to_string_lossy().to_string(),
                message: format!("打开文件追加失败: {}", e),
            })?;

        let mut writer = BufWriter::new(file);
        writer
            .write_all(content.as_bytes())
            .map_err(|e| IoError::FileOperationFailed {
                path: operation.path.to_string_lossy().to_string(),
                message: format!("追加文件内容失败: {}", e),
            })?;

        writer.flush().map_err(|e| IoError::FileOperationFailed {
            path: operation.path.to_string_lossy().to_string(),
            message: format!("刷新文件缓冲区失败: {}", e),
        })?;

        Ok(())
    }
}

impl Default for FileSystemInterface {
    fn default() -> Self {
        Self::new(8192).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_filesystem_creation() {
        let fs = FileSystemInterface::new(4096);
        assert!(fs.is_ok());
    }

    #[test]
    fn test_file_operation_creation() {
        let path = PathBuf::from("/test/file.txt");
        let operation = FileOperation::new(&path, FileOperationType::Read);
        assert_eq!(operation.operation_type, FileOperationType::Read);
        assert_eq!(operation.buffer_size, 8192);
    }

    #[test]
    fn test_file_operation_builder() {
        let path = PathBuf::from("/test/file.txt");
        let operation = FileOperation::new(&path, FileOperationType::Write)
            .with_encoding(FileEncoding::Utf16Le)
            .with_buffer_size(4096)
            .with_force(true);

        assert_eq!(operation.encoding, FileEncoding::Utf16Le);
        assert_eq!(operation.buffer_size, 4096);
        assert!(operation.force);
    }

    #[test]
    fn test_file_encoding() {
        assert_eq!(FileEncoding::default(), FileEncoding::Utf8);
        assert_eq!(FileEncoding::Utf8, FileEncoding::Utf8);
        assert_ne!(FileEncoding::Utf8, FileEncoding::Ascii);
    }

    #[test]
    fn test_filesystem_with_temp_dir() {
        let temp_dir = tempdir().unwrap();
        let file_path = temp_dir.path().join("test.txt");
        let test_content = "测试内容";

        let mut fs = FileSystemInterface::new(1024).unwrap();
        fs.initialize().unwrap();

        // Write file
        fs.write_file_string(&file_path, test_content).unwrap();
        assert!(fs.file_exists(&file_path));

        // Read file
        let read_content = fs.read_file_string(&file_path).unwrap();
        assert_eq!(read_content, test_content);

        // Get file size
        let size = fs.file_size(&file_path).unwrap();
        assert_eq!(size, test_content.len() as u64);

        // Append to file
        fs.append_file_string(&file_path, "追加内容").unwrap();
        let appended_content = fs.read_file_string(&file_path).unwrap();
        assert_eq!(appended_content, "测试内容追加内容");

        // Delete file
        fs.delete_file(&file_path).unwrap();
        assert!(!fs.file_exists(&file_path));
    }

    #[test]
    fn test_filesystem_statistics() {
        let mut fs = FileSystemInterface::new(1024).unwrap();
        fs.initialize().unwrap();

        let stats = fs.get_statistics();
        assert_eq!(stats.total_operations(), 0);
        assert_eq!(stats.success_rate(), 1.0);

        // Simulate some operations
        {
            let mut stats = fs.statistics.lock().unwrap();
            stats.record_read(1024, 10.5);
            stats.record_write(512, 5.0);
        }

        let updated_stats = fs.get_statistics();
        assert_eq!(updated_stats.total_operations(), 2);
        assert_eq!(updated_stats.success_rate(), 1.0);
        assert_eq!(updated_stats.bytes_read, 1024);
        assert_eq!(updated_stats.bytes_written, 512);
    }
}
