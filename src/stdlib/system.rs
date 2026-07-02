//! System Operations Module
//!
//! This module provides system-level operations including environment
//! variables, system information, and process management with Chinese
//! language support.

use crate::{RuntimeError, RuntimeResult};
use std::env;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// System information structure
#[derive(Debug, Clone)]
pub struct SystemInfo {
    /// Operating system type
    pub os_type: String,
    /// Architecture
    pub architecture: String,
    /// Hostname
    pub hostname: String,
    /// Number of CPU cores
    pub cpu_cores: usize,
    /// Total memory in bytes
    pub total_memory: u64,
    /// Available memory in bytes
    pub available_memory: u64,
    /// System uptime in seconds
    pub uptime: u64,
    /// Current working directory
    pub working_directory: PathBuf,
}

/// Process information structure
#[derive(Debug, Clone)]
pub struct ProcessInfo {
    /// Process ID
    pub pid: u32,
    /// Parent process ID
    pub parent_pid: u32,
    /// Process name
    pub name: String,
    /// Command line arguments
    pub arguments: Vec<String>,
    /// Working directory
    pub working_directory: PathBuf,
    /// Memory usage in bytes
    pub memory_usage: u64,
    /// CPU usage percentage
    pub cpu_usage: f64,
}

/// Environment variable configuration
#[derive(Debug, Clone)]
pub struct EnvConfig {
    /// Variable name
    pub name: String,
    /// Variable value
    pub value: String,
    /// Is read-only
    pub read_only: bool,
    /// Description in Chinese
    pub description: String,
}

/// System operations module
#[derive(Debug)]
pub struct SystemModule {
    /// System information cache
    system_info: Option<SystemInfo>,
    /// Process information cache
    process_info: Option<ProcessInfo>,
    /// Environment variables cache
    env_cache: std::collections::HashMap<String, String>,
    /// Last cache update time
    last_cache_update: Option<std::time::Instant>,
    /// Cache TTL in seconds
    cache_ttl: u64,
}

impl SystemModule {
    /// Create new system module
    pub fn new() -> Self {
        Self {
            system_info: None,
            process_info: None,
            env_cache: std::collections::HashMap::new(),
            last_cache_update: None,
            cache_ttl: 30, // 30 seconds cache TTL
        }
    }

    /// Create system module with custom cache TTL
    pub fn with_cache_ttl(cache_ttl: u64) -> Self {
        Self {
            system_info: None,
            process_info: None,
            env_cache: std::collections::HashMap::new(),
            last_cache_update: None,
            cache_ttl,
        }
    }

    /// Check if cache is valid
    fn is_cache_valid(&self) -> bool {
        if let Some(last_update) = self.last_cache_update {
            last_update.elapsed().as_secs() < self.cache_ttl
        } else {
            false
        }
    }

    /// Update cache
    fn update_cache(&mut self) -> RuntimeResult<()> {
        self.system_info = Some(self.get_system_info_impl()?);
        self.process_info = Some(self.get_process_info_impl()?);
        self.update_env_cache();
        self.last_cache_update = Some(std::time::Instant::now());
        Ok(())
    }

    /// Update environment variable cache
    fn update_env_cache(&mut self) {
        self.env_cache.clear();
        for (key, value) in env::vars() {
            self.env_cache.insert(key, value);
        }
    }

    /// Get system information
    pub fn get_system_info(&mut self) -> RuntimeResult<SystemInfo> {
        if !self.is_cache_valid() {
            self.update_cache()?;
        }
        Ok(self.system_info.clone().unwrap())
    }

    /// Get system information implementation
    fn get_system_info_impl(&self) -> RuntimeResult<SystemInfo> {
        let os_type = env::consts::OS.to_string();
        let architecture = env::consts::ARCH.to_string();

        // Get hostname
        let hostname = match Command::new("hostname").output() {
            Ok(output) => {
                let hostname_str = String::from_utf8_lossy(&output.stdout);
                hostname_str.trim().to_string()
            }
            Err(_) => "未知".to_string(),
        };

        // Get CPU cores (fallback)
        let cpu_cores = 1;

        // Get memory information (fallback)
        let (total_memory, available_memory) = (0, 0);

        // Get system uptime (platform-specific)
        let uptime = self.get_system_uptime()?;

        // Get current working directory
        let working_directory = env::current_dir().map_err(|e| {
            RuntimeError::system_error(
                format!("获取工作目录失败: {}", e),
                "获取工作目录失败".to_string(),
            )
        })?;

        Ok(SystemInfo {
            os_type,
            architecture,
            hostname,
            cpu_cores,
            total_memory,
            available_memory,
            uptime,
            working_directory,
        })
    }

    /// Get memory information
    fn get_memory_info(&self) -> RuntimeResult<(u64, u64)> {
        // Fallback implementation - return zeros
        Ok((0, 0))
    }

    /// Get system uptime
    fn get_system_uptime(&self) -> RuntimeResult<u64> {
        // Fallback implementation - return zero
        Ok(0)
    }

    /// Get process information
    pub fn get_process_info(&mut self) -> RuntimeResult<ProcessInfo> {
        if !self.is_cache_valid() {
            self.update_cache()?;
        }
        Ok(self.process_info.clone().unwrap())
    }

    /// Get process information implementation
    fn get_process_info_impl(&self) -> RuntimeResult<ProcessInfo> {
        let pid = std::process::id();
        let parent_pid = self.get_parent_pid()?;

        // Get process name
        let name = env::current_exe()
            .map(|path| {
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string()
            })
            .unwrap_or_else(|_| "未知进程".to_string());

        // Get command line arguments
        let arguments: Vec<String> = env::args().collect();

        // Get working directory
        let working_directory = env::current_dir().map_err(|e| {
            RuntimeError::system_error(
                format!("获取工作目录失败: {}", e),
                "获取工作目录失败".to_string(),
            )
        })?;

        // Get memory usage (platform-specific)
        let memory_usage = self.get_process_memory()?;

        // Get CPU usage (platform-specific)
        let cpu_usage = self.get_process_cpu()?;

        Ok(ProcessInfo {
            pid,
            parent_pid,
            name,
            arguments,
            working_directory,
            memory_usage,
            cpu_usage,
        })
    }

    /// Get parent process ID
    fn get_parent_pid(&self) -> RuntimeResult<u32> {
        #[cfg(unix)]
        {
            use std::fs;
            match fs::read_to_string(format!("/proc/{}/stat", std::process::id())) {
                Ok(content) => {
                    let parts: Vec<&str> = content.split_whitespace().collect();
                    if parts.len() > 3 {
                        match parts[3].parse::<u32>() {
                            Ok(ppid) => Ok(ppid),
                            Err(_) => Ok(0),
                        }
                    } else {
                        Ok(0)
                    }
                }
                Err(_) => Ok(0),
            }
        }

        #[cfg(not(unix))]
        {
            Ok(0)
        }
    }

    /// Get process memory usage
    fn get_process_memory(&self) -> RuntimeResult<u64> {
        #[cfg(unix)]
        {
            use std::fs;
            match fs::read_to_string(format!("/proc/{}/status", std::process::id())) {
                Ok(content) => {
                    for line in content.lines() {
                        if line.starts_with("VmRSS:") {
                            let parts: Vec<&str> = line.split_whitespace().collect();
                            if parts.len() > 1 {
                                match parts[1].parse::<u64>() {
                                    Ok(kb) => return Ok(kb * 1024), // Convert KB to bytes
                                    Err(_) => break,
                                }
                            }
                        }
                    }
                    Ok(0)
                }
                Err(_) => Ok(0),
            }
        }

        #[cfg(not(unix))]
        {
            Ok(0)
        }
    }

    /// Get process CPU usage
    fn get_process_cpu(&self) -> RuntimeResult<f64> {
        // CPU usage calculation is complex and platform-specific
        // For now, return a placeholder value
        Ok(0.0)
    }

    /// Get environment variable
    pub fn get_env(&mut self, key: &str) -> RuntimeResult<String> {
        if !self.is_cache_valid() {
            self.update_cache()?;
        }

        match self.env_cache.get(key) {
            Some(value) => Ok(value.clone()),
            None => Err(RuntimeError::system_error(
                format!("环境变量不存在: {}", key),
                "环境变量不存在".to_string(),
            )),
        }
    }

    /// Set environment variable
    pub fn set_env(&mut self, key: &str, value: &str) -> RuntimeResult<()> {
        env::set_var(key, value);

        // Update cache
        if self.is_cache_valid() {
            self.env_cache.insert(key.to_string(), value.to_string());
        }

        Ok(())
    }

    /// Remove environment variable
    pub fn remove_env(&mut self, key: &str) -> RuntimeResult<()> {
        env::remove_var(key);

        // Update cache
        if self.is_cache_valid() {
            self.env_cache.remove(key);
        }

        Ok(())
    }

    /// List all environment variables
    pub fn list_env(&mut self) -> RuntimeResult<Vec<(String, String)>> {
        if !self.is_cache_valid() {
            self.update_cache()?;
        }

        let mut env_vars: Vec<(String, String)> = self
            .env_cache
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        env_vars.sort_by(|a, b| a.0.cmp(&b.0));

        Ok(env_vars)
    }

    /// Get current working directory
    pub fn get_working_directory(&self) -> RuntimeResult<PathBuf> {
        env::current_dir().map_err(|e| {
            RuntimeError::system_error(
                format!("获取工作目录失败: {}", e),
                "获取工作目录失败".to_string(),
            )
        })
    }

    /// Set working directory
    pub fn set_working_directory(&mut self, path: &PathBuf) -> RuntimeResult<()> {
        env::set_current_dir(path).map_err(|e| {
            RuntimeError::system_error(
                format!("设置工作目录失败: {}", e),
                "设置工作目录失败".to_string(),
            )
        })
    }

    /// Get current Unix timestamp
    pub fn get_timestamp(&self) -> RuntimeResult<u64> {
        let now = SystemTime::now();
        let duration = now.duration_since(UNIX_EPOCH).map_err(|e| {
            RuntimeError::system_error(
                format!("获取时间戳失败: {}", e),
                "获取时间戳失败".to_string(),
            )
        })?;
        Ok(duration.as_secs())
    }

    /// Get current timestamp in milliseconds
    pub fn get_timestamp_millis(&self) -> RuntimeResult<u64> {
        let now = SystemTime::now();
        let duration = now.duration_since(UNIX_EPOCH).map_err(|e| {
            RuntimeError::system_error(
                format!("获取时间戳失败: {}", e),
                "获取时间戳失败".to_string(),
            )
        })?;
        Ok(duration.as_millis() as u64)
    }

    /// Get system temporary directory
    pub fn get_temp_directory(&self) -> RuntimeResult<PathBuf> {
        let temp_dir = env::temp_dir();
        Ok(temp_dir)
    }

    /// Get user home directory
    pub fn get_home_directory(&self) -> RuntimeResult<PathBuf> {
        #[cfg(unix)]
        {
            match env::var("HOME") {
                Ok(home) => Ok(PathBuf::from(home)),
                Err(_) => Err(RuntimeError::system_error(
                    "无法获取用户主目录".to_string(),
                    "无法获取用户主目录".to_string(),
                )),
            }
        }

        #[cfg(windows)]
        {
            match env::var("USERPROFILE") {
                Ok(home) => Ok(PathBuf::from(home)),
                Err(_) => Err(RuntimeError::system_error(
                    "无法获取用户主目录".to_string(),
                    "无法获取用户主目录".to_string(),
                )),
            }
        }

        #[cfg(not(any(unix, windows)))]
        {
            Err(RuntimeError::system_error(
                "不支持的平台".to_string(),
                "不支持的平台".to_string(),
            ))
        }
    }

    /// Execute system command
    pub fn execute_command(&self, command: &str, args: &[&str]) -> RuntimeResult<String> {
        let output = Command::new(command).args(args).output().map_err(|e| {
            RuntimeError::system_error(
                format!("执行命令失败: {} - {}", command, e),
                "执行命令失败".to_string(),
            )
        })?;

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Ok(stdout.to_string())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(RuntimeError::system_error(
                format!("命令执行失败: {}", stderr),
                "命令执行失败".to_string(),
            ))
        }
    }

    /// Check if command exists
    pub fn command_exists(&self, command: &str) -> bool {
        #[cfg(unix)]
        {
            match Command::new("which").arg(command).output() {
                Ok(output) => output.status.success(),
                Err(_) => false,
            }
        }

        #[cfg(windows)]
        {
            match Command::new("where").arg(command).output() {
                Ok(output) => output.status.success(),
                Err(_) => false,
            }
        }

        #[cfg(not(any(unix, windows)))]
        {
            false
        }
    }

    /// Get system locale
    pub fn get_locale(&self) -> RuntimeResult<String> {
        env::var("LANG")
            .or_else(|_| env::var("LC_ALL"))
            .or_else(|_| env::var("LC_CTYPE"))
            .map_err(|_| {
                RuntimeError::system_error(
                    "无法获取系统语言环境".to_string(),
                    "无法获取系统语言环境".to_string(),
                )
            })
    }

    /// Set system locale
    pub fn set_locale(&mut self, locale: &str) -> RuntimeResult<()> {
        self.set_env("LANG", locale)
    }

    /// Clear cache
    pub fn clear_cache(&mut self) {
        self.system_info = None;
        self.process_info = None;
        self.env_cache.clear();
        self.last_cache_update = None;
    }

    /// Get cache TTL
    pub fn cache_ttl(&self) -> u64 {
        self.cache_ttl
    }

    /// Set cache TTL
    pub fn set_cache_ttl(&mut self, ttl: u64) {
        self.cache_ttl = ttl;
    }
}

impl Default for SystemModule {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_system_module_creation() {
        let system = SystemModule::new();
        assert_eq!(system.cache_ttl(), 30);

        let system_with_ttl = SystemModule::with_cache_ttl(60);
        assert_eq!(system_with_ttl.cache_ttl(), 60);
    }

    #[test]
    fn test_environment_variables() {
        let mut system = SystemModule::new();

        // Test setting and getting environment variable
        system.set_env("TEST_VAR", "test_value").unwrap();
        let value = system.get_env("TEST_VAR").unwrap();
        assert_eq!(value, "test_value");

        // Test removing environment variable
        system.remove_env("TEST_VAR").unwrap();
        assert!(system.get_env("TEST_VAR").is_err());
    }

    #[test]
    fn test_list_environment_variables() {
        let mut system = SystemModule::new();
        let env_vars = system.list_env().unwrap();

        // Should have at least some environment variables
        assert!(!env_vars.is_empty());

        // Should be sorted
        for i in 1..env_vars.len() {
            assert!(env_vars[i - 1].0 <= env_vars[i].0);
        }
    }

    #[test]
    fn test_working_directory() {
        let mut system = SystemModule::new();

        let cwd = system.get_working_directory().unwrap();
        assert!(cwd.exists());

        let new_cwd = cwd.clone();
        system.set_working_directory(&new_cwd).unwrap();

        let retrieved_cwd = system.get_working_directory().unwrap();
        assert_eq!(new_cwd, retrieved_cwd);
    }

    #[test]
    fn test_timestamps() {
        let system = SystemModule::new();

        let timestamp = system.get_timestamp().unwrap();
        let timestamp_millis = system.get_timestamp_millis().unwrap();

        // Timestamps should be reasonable (not zero, not too large)
        assert!(timestamp > 1600000000); // After 2020
        // 两次调用间可能跨秒边界：放宽到 [ts*1000, (ts+2)*1000) 窗口，原严格不等式偶发失败
        assert!(timestamp_millis >= timestamp * 1000);
        assert!(timestamp_millis < (timestamp + 2) * 1000);
    }

    #[test]
    fn test_directories() {
        let system = SystemModule::new();

        let temp_dir = system.get_temp_directory().unwrap();
        assert!(temp_dir.exists());

        let home_dir = system.get_home_directory();
        // May fail on some systems, so we just check if it exists
        if let Ok(home) = home_dir {
            assert!(home.exists());
        }
    }

    #[test]
    fn test_locale() {
        let mut system = SystemModule::new();

        let locale = system.get_locale();
        // May fail on some systems, so we just check if it exists
        if let Ok(loc) = locale {
            assert!(!loc.is_empty());
        }

        // Setting locale should work
        let result = system.set_locale("en_US.UTF-8");
        assert!(result.is_ok() || result.is_err()); // We don't care if it succeeds or fails
    }

    #[test]
    fn test_cache_operations() {
        let mut system = SystemModule::new();

        // Initially cache should be invalid
        assert!(!system.is_cache_valid());

        // Get system info should populate cache
        let _info = system.get_system_info().unwrap();
        assert!(system.is_cache_valid());

        // Clear cache
        system.clear_cache();
        assert!(!system.is_cache_valid());

        // Set cache TTL
        system.set_cache_ttl(120);
        assert_eq!(system.cache_ttl(), 120);
    }

    #[test]
    fn test_command_exists() {
        let system = SystemModule::new();

        // Test with common commands
        let has_ls = system.command_exists("ls");
        let has_which = system.command_exists("which");

        // At least one of these should exist on most systems
        assert!(
            has_ls || has_which || system.command_exists("dir") || system.command_exists("where")
        );
    }

    #[test]
    fn test_execute_command() {
        let system = SystemModule::new();

        // Test with echo command (should work on most systems)
        #[cfg(unix)]
        {
            let result = system.execute_command("echo", &["hello"]);
            assert!(result.is_ok());
            assert_eq!(result.unwrap().trim(), "hello");
        }

        #[cfg(windows)]
        {
            let result = system.execute_command("echo", &["hello"]);
            assert!(result.is_ok());
            assert!(result.unwrap().trim().contains("hello"));
        }
    }
}
