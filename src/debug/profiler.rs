//! Performance Profiler for Qi Runtime
//!
//! This module provides comprehensive performance profiling capabilities including
//! function timing, memory usage tracking, and performance analysis with Chinese
//! language support for reports.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

/// Performance profiler
#[derive(Debug)]
pub struct Profiler {
    /// Debug module for logging
    debug_module: Arc<crate::stdlib::debug::DebugModule>,
    /// Active profiling sessions
    active_sessions: Arc<Mutex<HashMap<String, ProfileSession>>>,
    /// Completed profile data
    completed_data: Arc<Mutex<Vec<ProfileData>>>,
    /// Configuration
    config: ProfileConfig,
    /// Global statistics
    global_stats: Arc<Mutex<GlobalStats>>,
}

/// Profiler configuration
#[derive(Debug, Clone)]
pub struct ProfileConfig {
    /// Maximum number of active sessions
    pub max_active_sessions: usize,
    /// Maximum number of completed profiles to keep
    pub max_completed_profiles: usize,
    /// Enable memory profiling
    pub enable_memory_profiling: bool,
    /// Enable call stack sampling
    pub enable_call_stack_sampling: bool,
    /// Sampling interval for call stacks (milliseconds)
    pub sampling_interval_ms: u64,
    /// Auto-save profiles
    pub auto_save: bool,
    /// Auto-save directory
    pub auto_save_dir: Option<String>,
}

impl Default for ProfileConfig {
    fn default() -> Self {
        Self {
            max_active_sessions: 10,
            max_completed_profiles: 100,
            enable_memory_profiling: true,
            enable_call_stack_sampling: false, // Disabled by default for performance
            sampling_interval_ms: 10,
            auto_save: false,
            auto_save_dir: None,
        }
    }
}

/// Active profiling session
#[derive(Debug)]
pub struct ProfileSession {
    /// Session name
    pub name: String,
    /// Start time
    pub start_time: SystemTime,
    /// Current function call stack
    pub call_stack: Vec<FunctionCall>,
    /// Memory snapshots
    pub memory_snapshots: Vec<MemorySnapshot>,
    /// Function timing data
    pub function_times: HashMap<String, Vec<FunctionTiming>>,
    /// Session metadata
    pub metadata: SessionMetadata,
}

/// Function call information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    /// Function name
    pub name: String,
    /// Entry timestamp (microseconds)
    pub entry_time_us: u64,
    /// Memory usage at entry (bytes)
    pub memory_at_entry: usize,
    /// Call depth
    pub depth: usize,
}

/// Memory usage snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemorySnapshot {
    /// Timestamp (microseconds)
    pub timestamp_us: u64,
    /// Total memory usage (bytes)
    pub total_memory: usize,
    /// Heap memory (bytes)
    pub heap_memory: usize,
    /// Stack memory (bytes)
    pub stack_memory: usize,
    /// Number of allocations
    pub allocation_count: usize,
}

/// Function timing information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionTiming {
    /// Function name
    pub function_name: String,
    /// Start time (microseconds)
    pub start_time_us: u64,
    /// End time (microseconds)
    pub end_time_us: u64,
    /// Duration (microseconds)
    pub duration_us: u64,
    /// Memory allocated during call (bytes)
    pub memory_allocated: usize,
    /// Memory deallocated during call (bytes)
    pub memory_deallocated: usize,
    /// Call depth
    pub depth: usize,
    /// Number of sub-calls
    pub sub_call_count: usize,
}

/// Session metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionMetadata {
    /// Session type
    pub session_type: SessionType,
    /// Program being profiled
    pub program_name: Option<String>,
    /// Command line arguments
    pub arguments: Vec<String>,
    /// Environment variables
    pub environment: HashMap<String, String>,
    /// System information
    pub system_info: SystemInfo,
}

/// Session types
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SessionType {
    /// Function-level profiling
    Function,
    /// Memory profiling
    Memory,
    /// CPU profiling
    Cpu,
    /// Mixed profiling
    Mixed,
    /// Custom profiling
    Custom(String),
}

/// System information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemInfo {
    /// Operating system
    pub os: String,
    /// Architecture
    pub arch: String,
    /// CPU cores
    pub cpu_cores: usize,
    /// Total memory (bytes)
    pub total_memory: usize,
    /// Available memory (bytes)
    pub available_memory: usize,
}

/// Completed profile data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileData {
    /// Profile name
    pub name: String,
    /// Session metadata
    pub metadata: SessionMetadata,
    /// Total duration (microseconds)
    pub total_duration_us: u64,
    /// Function timing data
    pub function_timings: Vec<FunctionTiming>,
    /// Memory snapshots
    pub memory_snapshots: Vec<MemorySnapshot>,
    /// Performance summary
    pub summary: PerformanceSummary,
    /// Profile creation timestamp
    pub created_at: u64,
}

/// Performance summary
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceSummary {
    /// Total function calls
    pub total_function_calls: usize,
    /// Unique functions called
    pub unique_functions: usize,
    /// Maximum call depth
    pub max_call_depth: usize,
    /// Average call duration (microseconds)
    pub average_call_duration_us: f64,
    /// Total memory allocated (bytes)
    pub total_memory_allocated: usize,
    /// Peak memory usage (bytes)
    pub peak_memory_usage: usize,
    /// Function call distribution
    pub call_distribution: HashMap<String, usize>,
    /// Memory usage distribution
    pub memory_distribution: HashMap<String, usize>,
}

/// Global profiler statistics
#[derive(Debug, Clone, Default)]
pub struct GlobalStats {
    /// Total sessions created
    pub total_sessions: usize,
    /// Total profiles completed
    pub total_profiles: usize,
    /// Total profiling time (microseconds)
    pub total_profiling_time_us: u64,
    /// Memory used by profiler (bytes)
    pub profiler_memory_usage: usize,
}

/// Profiler statistics
#[derive(Debug, Clone)]
pub struct ProfilerStats {
    /// Number of active sessions
    pub active_sessions: usize,
    /// Number of completed profiles
    pub completed_profiles: usize,
    /// Global statistics
    pub global_stats: GlobalStats,
}

impl Profiler {
    /// Create new profiler
    pub fn new(debug_module: Arc<crate::stdlib::debug::DebugModule>) -> Self {
        Self::with_config(debug_module, ProfileConfig::default())
    }

    /// Create profiler with configuration
    pub fn with_config(
        debug_module: Arc<crate::stdlib::debug::DebugModule>,
        config: ProfileConfig,
    ) -> Self {
        Self {
            debug_module,
            active_sessions: Arc::new(Mutex::new(HashMap::new())),
            completed_data: Arc::new(Mutex::new(Vec::new())),
            config,
            global_stats: Arc::new(Mutex::new(GlobalStats::default())),
        }
    }

    /// Initialize the profiler
    pub fn initialize(&self) -> super::RuntimeResult<()> {
        self.debug_module.info("初始化性能分析器")?;

        if self.config.enable_memory_profiling {
            self.debug_module.debug("启用内存性能分析")?;
        }

        if self.config.enable_call_stack_sampling {
            self.debug_module.debug("启用调用栈采样")?;
        }

        self.debug_module.info("性能分析器初始化完成")?;
        Ok(())
    }

    /// Start a new profiling session
    pub fn start_profiling(&self, name: &str) -> super::RuntimeResult<()> {
        let mut sessions = self.active_sessions.lock().unwrap();

        if sessions.len() >= self.config.max_active_sessions {
            return Err(super::RuntimeError::debug_error(
                "Maximum number of active profiling sessions reached".to_string(),
                "达到最大活动性能分析会话数".to_string(),
            ));
        }

        if sessions.contains_key(name) {
            return Err(super::RuntimeError::debug_error(
                format!("Profiling session '{}' already exists", name),
                format!("性能分析会话 '{}' 已存在", name),
            ));
        }

        let start_time = SystemTime::now();
        let session = ProfileSession {
            name: name.to_string(),
            start_time,
            call_stack: Vec::new(),
            memory_snapshots: Vec::new(),
            function_times: HashMap::new(),
            metadata: SessionMetadata {
                session_type: SessionType::Mixed,
                program_name: None,
                arguments: Vec::new(),
                environment: HashMap::new(),
                system_info: self.gather_system_info(),
            },
        };

        sessions.insert(name.to_string(), session);

        // Update global stats
        {
            let mut stats = self.global_stats.lock().unwrap();
            stats.total_sessions += 1;
        }

        self.debug_module
            .info(&format!("开始性能分析会话: {}", name))?;
        Ok(())
    }

    /// Stop a profiling session and generate profile data
    pub fn stop_profiling(&self, name: &str) -> super::RuntimeResult<ProfileData> {
        let mut sessions = self.active_sessions.lock().unwrap();

        let session = sessions.remove(name).ok_or_else(|| {
            super::RuntimeError::debug_error(
                format!("Profiling session '{}' not found", name),
                format!("性能分析会话 '{}' 未找到", name),
            )
        })?;

        let end_time = SystemTime::now();
        let total_duration = end_time
            .duration_since(session.start_time)
            .map_err(|e| {
                super::RuntimeError::debug_error(
                    format!("Invalid duration: {}", e),
                    "无效持续时间".to_string(),
                )
            })?
            .as_micros() as u64;

        // Collect final memory snapshot
        let final_memory = self.get_memory_snapshot()?;

        // Generate performance summary
        let summary = self.generate_performance_summary(&session, total_duration);

        // Create profile data
        let profile_data = ProfileData {
            name: name.to_string(),
            metadata: session.metadata,
            total_duration_us: total_duration,
            function_timings: session.function_times.values().flatten().cloned().collect(),
            memory_snapshots: session.memory_snapshots,
            summary,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        };

        // Store completed profile
        {
            let mut completed = self.completed_data.lock().unwrap();
            completed.push(profile_data.clone());

            // Limit completed profiles
            if completed.len() > self.config.max_completed_profiles {
                completed.remove(0);
            }
        }

        // Update global stats
        {
            let mut stats = self.global_stats.lock().unwrap();
            stats.total_profiles += 1;
            stats.total_profiling_time_us += total_duration;
        }

        // Auto-save if enabled
        if self.config.auto_save {
            if let Err(e) = self.auto_save_profile(&profile_data) {
                self.debug_module
                    .warning(&format!("自动保存性能分析失败: {}", e))?;
            }
        }

        self.debug_module.info(&format!(
            "性能分析会话完成: {} (耗时: {}μs)",
            name, total_duration
        ))?;
        Ok(profile_data)
    }

    /// Enter a function call
    pub fn enter_function(
        &self,
        session_name: &str,
        function_name: &str,
    ) -> super::RuntimeResult<()> {
        let mut sessions = self.active_sessions.lock().unwrap();

        if let Some(session) = sessions.get_mut(session_name) {
            let entry_time = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_micros() as u64;

            let memory_usage = self.get_current_memory_usage()?;

            let function_call = FunctionCall {
                name: function_name.to_string(),
                entry_time_us: entry_time,
                memory_at_entry: memory_usage,
                depth: session.call_stack.len(),
            };

            session.call_stack.push(function_call);
            Ok(())
        } else {
            Err(super::RuntimeError::debug_error(
                format!("Profiling session '{}' not found", session_name),
                format!("性能分析会话 '{}' 未找到", session_name),
            ))
        }
    }

    /// Exit a function call
    pub fn exit_function(
        &self,
        session_name: &str,
        function_name: &str,
    ) -> super::RuntimeResult<()> {
        let mut sessions = self.active_sessions.lock().unwrap();

        if let Some(session) = sessions.get_mut(session_name) {
            if let Some(function_call) = session.call_stack.pop() {
                if function_call.name != function_name {
                    self.debug_module.warning(&format!(
                        "函数退出不匹配: 期望 '{}', 实际 '{}'",
                        function_name, function_call.name
                    ))?;
                }

                let exit_time = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_micros() as u64;

                let memory_usage = self.get_current_memory_usage()?;

                let timing = FunctionTiming {
                    function_name: function_name.to_string(),
                    start_time_us: function_call.entry_time_us,
                    end_time_us: exit_time,
                    duration_us: exit_time - function_call.entry_time_us,
                    memory_allocated: memory_usage.saturating_sub(function_call.memory_at_entry),
                    memory_deallocated: 0, // TODO: Track deallocations
                    depth: function_call.depth,
                    sub_call_count: 0, // TODO: Calculate sub-calls
                };

                session
                    .function_times
                    .entry(function_name.to_string())
                    .or_insert_with(Vec::new)
                    .push(timing);
            } else {
                self.debug_module
                    .warning(&format!("空调用栈时尝试退出函数: {}", function_name))?;
            }
            Ok(())
        } else {
            Err(super::RuntimeError::debug_error(
                format!("Profiling session '{}' not found", session_name),
                format!("性能分析会话 '{}' 未找到", session_name),
            ))
        }
    }

    /// Get all completed profile data
    pub fn get_all_data(&self) -> Vec<ProfileData> {
        self.completed_data.lock().unwrap().clone()
    }

    /// Get profile data by name
    pub fn get_profile_by_name(&self, name: &str) -> Option<ProfileData> {
        self.completed_data
            .lock()
            .unwrap()
            .iter()
            .find(|profile| profile.name == name)
            .cloned()
    }

    /// Clear all profiling data
    pub fn clear_all_data(&self) -> super::RuntimeResult<()> {
        {
            let mut sessions = self.active_sessions.lock().unwrap();
            sessions.clear();
        }

        {
            let mut completed = self.completed_data.lock().unwrap();
            completed.clear();
        }

        {
            let mut stats = self.global_stats.lock().unwrap();
            *stats = GlobalStats::default();
        }

        self.debug_module.info("所有性能分析数据已清理")?;
        Ok(())
    }

    /// Get profiler statistics
    pub fn get_statistics(&self) -> super::RuntimeResult<ProfilerStats> {
        let sessions = self.active_sessions.lock().unwrap();
        let completed = self.completed_data.lock().unwrap();
        let global_stats = self.global_stats.lock().unwrap();

        Ok(ProfilerStats {
            active_sessions: sessions.len(),
            completed_profiles: completed.len(),
            global_stats: global_stats.clone(),
        })
    }

    /// Generate performance summary
    fn generate_performance_summary(
        &self,
        session: &ProfileSession,
        total_duration: u64,
    ) -> PerformanceSummary {
        let all_timings: Vec<FunctionTiming> =
            session.function_times.values().flatten().cloned().collect();

        let total_calls = all_timings.len();
        let unique_functions = session.function_times.len();
        let max_depth = session.call_stack.len();

        let average_duration = if total_calls > 0 {
            all_timings.iter().map(|t| t.duration_us).sum::<u64>() as f64 / total_calls as f64
        } else {
            0.0
        };

        let total_memory_allocated = all_timings.iter().map(|t| t.memory_allocated).sum();
        let peak_memory = session
            .memory_snapshots
            .iter()
            .map(|s| s.total_memory)
            .max()
            .unwrap_or(0);

        let mut call_distribution = HashMap::new();
        for timing in &all_timings {
            *call_distribution
                .entry(timing.function_name.clone())
                .or_insert(0) += 1;
        }

        let mut memory_distribution = HashMap::new();
        for timing in &all_timings {
            *memory_distribution
                .entry(timing.function_name.clone())
                .or_insert(0) += timing.memory_allocated;
        }

        PerformanceSummary {
            total_function_calls: total_calls,
            unique_functions,
            max_call_depth: max_depth,
            average_call_duration_us: average_duration,
            total_memory_allocated,
            peak_memory_usage: peak_memory,
            call_distribution,
            memory_distribution,
        }
    }

    /// Gather system information
    fn gather_system_info(&self) -> SystemInfo {
        SystemInfo {
            os: std::env::consts::OS.to_string(),
            arch: std::env::consts::ARCH.to_string(),
            cpu_cores: num_cpus::get(),
            total_memory: 0, // TODO: Implement actual memory detection
            available_memory: 0,
        }
    }

    /// Get current memory usage (simplified)
    fn get_current_memory_usage(&self) -> super::RuntimeResult<usize> {
        // This is a simplified implementation
        // In a real implementation, you'd use system-specific APIs
        Ok(1024 * 1024) // Mock 1MB
    }

    /// Get memory snapshot
    fn get_memory_snapshot(&self) -> super::RuntimeResult<MemorySnapshot> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;

        let total_memory = self.get_current_memory_usage()?;

        Ok(MemorySnapshot {
            timestamp_us: timestamp,
            total_memory,
            heap_memory: total_memory / 2,  // Mock split
            stack_memory: total_memory / 4, // Mock split
            allocation_count: 0,            // TODO: Track allocations
        })
    }

    /// Auto-save profile data
    fn auto_save_profile(&self, profile: &ProfileData) -> super::RuntimeResult<()> {
        if let Some(ref dir) = self.config.auto_save_dir {
            let filename = format!("{}/{}_{}.json", dir, profile.name, profile.created_at);

            // Save profile data to file
            let json = serde_json::to_string_pretty(profile).map_err(|e| {
                super::RuntimeError::debug_error(
                    format!("JSON serialization failed: {}", e),
                    "JSON序列化失败".to_string(),
                )
            })?;

            std::fs::write(&filename, json).map_err(|e| {
                super::RuntimeError::debug_error(
                    format!("Failed to write profile file: {}", e),
                    "写入性能分析文件失败".to_string(),
                )
            })?;

            self.debug_module
                .debug(&format!("性能分析已自动保存: {}", filename))?;
        }

        Ok(())
    }
}

/// Convenience function to create a profiler
pub fn create_profiler(debug_module: Arc<crate::stdlib::debug::DebugModule>) -> Profiler {
    Profiler::new(debug_module)
}

/// RAII helper for automatic function profiling
pub struct ProfileGuard {
    profiler: std::sync::Arc<Profiler>,
    session_name: String,
    function_name: String,
}

impl ProfileGuard {
    /// Create new profile guard
    pub fn new(
        profiler: std::sync::Arc<Profiler>,
        session_name: &str,
        function_name: &str,
    ) -> super::RuntimeResult<Self> {
        profiler.enter_function(session_name, function_name)?;
        Ok(Self {
            profiler,
            session_name: session_name.to_string(),
            function_name: function_name.to_string(),
        })
    }
}

impl Drop for ProfileGuard {
    fn drop(&mut self) {
        if let Err(e) = self
            .profiler
            .exit_function(&self.session_name, &self.function_name)
        {
            eprintln!("ProfileGuard drop failed: {}", e);
        }
    }
}

/// Macro for easy function profiling
#[macro_export]
macro_rules! profile_function {
    ($profiler:expr, $session:expr, $name:expr) => {
        let _guard = $crate::debug::profiler::ProfileGuard::new(
            std::sync::Arc::clone($profiler),
            $session,
            $name,
        )?;
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_profiler_creation() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let profiler = Profiler::new(debug_module);

        assert!(profiler.initialize().is_ok());
    }

    #[test]
    fn test_profiling_session() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let profiler = Profiler::new(debug_module);

        profiler.initialize().unwrap();

        // Start profiling
        profiler.start_profiling("test_session").unwrap();

        // Enter and exit function
        profiler
            .enter_function("test_session", "test_function")
            .unwrap();
        profiler
            .exit_function("test_session", "test_function")
            .unwrap();

        // Stop profiling
        let profile_data = profiler.stop_profiling("test_session").unwrap();
        assert_eq!(profile_data.name, "test_session");
        assert!(profile_data.total_duration_us > 0);
    }

    #[test]
    fn test_multiple_sessions() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let profiler = Profiler::new(debug_module);

        profiler.initialize().unwrap();

        profiler.start_profiling("session1").unwrap();
        profiler.start_profiling("session2").unwrap();

        let stats = profiler.get_statistics().unwrap();
        assert_eq!(stats.active_sessions, 2);

        profiler.stop_profiling("session1").unwrap();
        profiler.stop_profiling("session2").unwrap();

        let stats = profiler.get_statistics().unwrap();
        assert_eq!(stats.active_sessions, 0);
        assert_eq!(stats.completed_profiles, 2);
    }

    #[test]
    fn test_duplicate_session_error() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let profiler = Profiler::new(debug_module);

        profiler.initialize().unwrap();

        profiler.start_profiling("test").unwrap();

        let result = profiler.start_profiling("test");
        assert!(result.is_err());
    }

    #[test]
    fn test_nonexistent_session_error() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let profiler = Profiler::new(debug_module);

        profiler.initialize().unwrap();

        let result = profiler.stop_profiling("nonexistent");
        assert!(result.is_err());

        let result = profiler.enter_function("nonexistent", "test");
        assert!(result.is_err());
    }

    #[test]
    fn test_clear_data() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let profiler = Profiler::new(debug_module);

        profiler.initialize().unwrap();

        profiler.start_profiling("test").unwrap();
        profiler.stop_profiling("test").unwrap();

        let stats = profiler.get_statistics().unwrap();
        assert_eq!(stats.completed_profiles, 1);

        profiler.clear_all_data().unwrap();

        let stats = profiler.get_statistics().unwrap();
        assert_eq!(stats.completed_profiles, 0);
        assert_eq!(stats.global_stats.total_sessions, 0);
    }
}
