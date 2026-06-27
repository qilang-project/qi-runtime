//! Enhanced Debug Module for Qi Runtime
//!
//! This module provides comprehensive debugging support including:
//! - Stack trace collection with symbol resolution
//! - Variable inspection and tracking
//! - Performance monitoring and profiling
//! - Debug command processing
//! - Chinese language support for debugging messages

pub mod commands;
pub mod profiler;
pub mod stack_trace;
pub mod variable_inspector;

// Re-export main components (without duplicates)
pub use commands::{CommandResult, DebugCommandProcessor};
pub use profiler::{ProfileConfig, Profiler};
pub use stack_trace::{EnhancedStackFrame, FrameType, StackTraceCollector, StackTraceConfig};
pub use variable_inspector::{
    InspectorConfig, VariableInfo, VariableInspector, VariableMetadata, VariableValue,
};
// Rename to avoid conflict
pub use profiler::ProfileData as ProfilerData;

use crate::{RuntimeError, RuntimeResult};
use std::sync::{Arc, Mutex};

/// Comprehensive debugging system for Qi runtime
pub struct DebugSystem {
    /// Stack trace collector
    stack_collector: Arc<StackTraceCollector>,
    /// Variable inspector
    variable_inspector: Arc<VariableInspector>,
    /// Command processor
    command_processor: Arc<DebugCommandProcessor>,
    /// Profiler
    profiler: Arc<Mutex<Profiler>>,
    /// Debug module for logging
    debug_module: Arc<crate::stdlib::debug::DebugModule>,
    /// System configuration
    config: DebugSystemConfig,
}

/// Debug system configuration
#[derive(Debug, Clone)]
pub struct DebugSystemConfig {
    /// Enable stack trace collection
    pub enable_stack_traces: bool,
    /// Enable variable inspection
    pub enable_variable_inspection: bool,
    /// Enable command processing
    pub enable_commands: bool,
    /// Enable profiling
    pub enable_profiling: bool,
    /// Auto-capture stack traces on errors
    pub auto_capture_stack_traces: bool,
    /// Maximum memory usage for debugging data
    pub max_debug_memory_mb: usize,
}

impl Default for DebugSystemConfig {
    fn default() -> Self {
        Self {
            enable_stack_traces: true,
            enable_variable_inspection: true,
            enable_commands: true,
            enable_profiling: false, // Disabled by default for performance
            auto_capture_stack_traces: true,
            max_debug_memory_mb: 100,
        }
    }
}

/// Debug system statistics
#[derive(Debug, Clone)]
pub struct DebugSystemStats {
    /// Stack trace statistics
    pub stack_traces: stack_trace::StackTraceStats,
    /// Variable inspector statistics
    pub variable_inspector: variable_inspector::InspectorStats,
    /// Profiler statistics
    pub profiler_stats: Option<profiler::ProfilerStats>,
    /// Total debug memory usage (bytes)
    pub total_memory_usage: usize,
    /// Number of debug commands processed
    pub commands_processed: u64,
}

impl DebugSystem {
    /// Create new debug system
    pub fn new() -> RuntimeResult<Self> {
        Self::with_config(DebugSystemConfig::default())
    }

    /// Create debug system with configuration
    pub fn with_config(config: DebugSystemConfig) -> RuntimeResult<Self> {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());

        let stack_collector = Arc::new(StackTraceCollector::new(debug_module.clone()));
        let variable_inspector = Arc::new(VariableInspector::new(debug_module.clone()));
        let command_processor = Arc::new(DebugCommandProcessor::new(debug_module.clone()));
        let profiler = Arc::new(Mutex::new(Profiler::new(debug_module.clone())));

        Ok(Self {
            stack_collector,
            variable_inspector,
            command_processor,
            profiler,
            debug_module,
            config,
        })
    }

    /// Initialize the debug system
    pub fn initialize(&self) -> RuntimeResult<()> {
        self.debug_module.info("初始化调试系统")?;

        if self.config.enable_stack_traces {
            self.debug_module.debug("启用堆栈跟踪收集")?;
        }

        if self.config.enable_variable_inspection {
            self.debug_module.debug("启用变量检查")?;
        }

        if self.config.enable_commands {
            self.debug_module.debug("启用调试命令处理")?;
        }

        if self.config.enable_profiling {
            self.debug_module.debug("启用性能分析")?;
            self.profiler.lock().unwrap().initialize()?;
        }

        self.debug_module.info("调试系统初始化完成")?;
        Ok(())
    }

    /// Capture current stack trace
    pub fn capture_stack_trace(&self) -> RuntimeResult<Vec<EnhancedStackFrame>> {
        if !self.config.enable_stack_traces {
            return Err(RuntimeError::debug_error(
                "Stack trace collection is disabled".to_string(),
                "堆栈跟踪收集已禁用".to_string(),
            ));
        }

        self.stack_collector.collect_stack_trace()
    }

    /// Capture stack trace with context
    pub fn capture_stack_trace_with_context(
        &self,
        context: &str,
    ) -> RuntimeResult<Vec<EnhancedStackFrame>> {
        if !self.config.enable_stack_traces {
            return Err(RuntimeError::debug_error(
                "Stack trace collection is disabled".to_string(),
                "堆栈跟踪收集已禁用".to_string(),
            ));
        }

        self.stack_collector
            .collect_stack_trace_with_context(context)
    }

    /// Get formatted stack trace
    pub fn get_formatted_stack_trace(&self) -> RuntimeResult<String> {
        if !self.config.enable_stack_traces {
            return Err(RuntimeError::debug_error(
                "Stack trace collection is disabled".to_string(),
                "堆栈跟踪收集已禁用".to_string(),
            ));
        }

        self.stack_collector.get_formatted_stack_trace()
    }

    /// Register a variable for inspection
    pub fn register_variable(&self, name: &str, value: &dyn VariableValue) -> RuntimeResult<()> {
        if !self.config.enable_variable_inspection {
            return Err(RuntimeError::debug_error(
                "Variable inspection is disabled".to_string(),
                "变量检查已禁用".to_string(),
            ));
        }

        self.variable_inspector.register_variable(name, value)
    }

    /// Inspect a registered variable
    pub fn inspect_variable(
        &self,
        name: &str,
    ) -> RuntimeResult<variable_inspector::InspectionResult> {
        if !self.config.enable_variable_inspection {
            return Err(RuntimeError::debug_error(
                "Variable inspection is disabled".to_string(),
                "变量检查已禁用".to_string(),
            ));
        }

        self.variable_inspector.inspect_variable(name)
    }

    /// Inspect a value directly
    pub fn inspect_value(
        &self,
        name: &str,
        value: &dyn VariableValue,
    ) -> RuntimeResult<variable_inspector::InspectionResult> {
        if !self.config.enable_variable_inspection {
            return Err(RuntimeError::debug_error(
                "Variable inspection is disabled".to_string(),
                "变量检查已禁用".to_string(),
            ));
        }

        self.variable_inspector.inspect_value(name, value)
    }

    /// List all registered variables
    pub fn list_variables(&self) -> RuntimeResult<Vec<String>> {
        if !self.config.enable_variable_inspection {
            return Err(RuntimeError::debug_error(
                "Variable inspection is disabled".to_string(),
                "变量检查已禁用".to_string(),
            ));
        }

        self.variable_inspector.list_variables()
    }

    /// Process a debug command
    pub fn process_command(&self, command: &str) -> RuntimeResult<CommandResult> {
        if !self.config.enable_commands {
            return Err(RuntimeError::debug_error(
                "Debug commands are disabled".to_string(),
                "调试命令已禁用".to_string(),
            ));
        }

        self.command_processor.process_command(command, self)
    }

    /// Start profiling
    pub fn start_profiling(&self, name: &str) -> RuntimeResult<()> {
        if !self.config.enable_profiling {
            return Err(RuntimeError::debug_error(
                "Profiling is disabled".to_string(),
                "性能分析已禁用".to_string(),
            ));
        }

        self.profiler.lock().unwrap().start_profiling(name)
    }

    /// Stop profiling
    pub fn stop_profiling(&self, name: &str) -> RuntimeResult<ProfilerData> {
        if !self.config.enable_profiling {
            return Err(RuntimeError::debug_error(
                "Profiling is disabled".to_string(),
                "性能分析已禁用".to_string(),
            ));
        }

        self.profiler.lock().unwrap().stop_profiling(name)
    }

    /// Get all profile data
    pub fn get_profile_data(&self) -> RuntimeResult<Vec<ProfilerData>> {
        if !self.config.enable_profiling {
            return Err(RuntimeError::debug_error(
                "Profiling is disabled".to_string(),
                "性能分析已禁用".to_string(),
            ));
        }

        Ok(self.profiler.lock().unwrap().get_all_data())
    }

    /// Handle runtime error with debugging information
    pub fn handle_error(&self, error: &RuntimeError) -> RuntimeResult<()> {
        self.debug_module.error(&format!("运行时错误: {}", error))?;

        if self.config.auto_capture_stack_traces {
            match self.capture_stack_trace_with_context(&format!("错误: {}", error)) {
                Ok(frames) => {
                    self.debug_module.error("错误堆栈跟踪:")?;
                    for frame in frames.iter().take(5) {
                        // Limit to first 5 frames
                        self.debug_module
                            .error(&format!("  {}", frame.frame.format()))?;
                    }
                }
                Err(e) => {
                    self.debug_module
                        .warning(&format!("无法捕获堆栈跟踪: {}", e))?;
                }
            }
        }

        Ok(())
    }

    /// Get debug system statistics
    pub fn get_statistics(&self) -> RuntimeResult<DebugSystemStats> {
        let stack_stats = self.stack_collector.get_statistics()?;
        let var_stats = self.variable_inspector.get_statistics()?;
        let profiler_stats = if self.config.enable_profiling {
            Some(self.profiler.lock().unwrap().get_statistics()?)
        } else {
            None
        };

        // Calculate total memory usage (simplified)
        let total_memory_usage = stack_stats.cached_symbols * std::mem::size_of::<String>()
            + var_stats.registered_variables * std::mem::size_of::<VariableInfo>();

        Ok(DebugSystemStats {
            stack_traces: stack_stats,
            variable_inspector: var_stats,
            profiler_stats,
            total_memory_usage,
            commands_processed: self.command_processor.get_commands_processed(),
        })
    }

    /// Clear all debugging data
    pub fn clear_all_data(&self) -> RuntimeResult<()> {
        self.debug_module.info("清理所有调试数据")?;

        if self.config.enable_stack_traces {
            self.stack_collector.clear_symbol_cache()?;
            self.stack_collector.clear_source_mapping()?;
        }

        if self.config.enable_variable_inspection {
            self.variable_inspector.clear_all_variables()?;
        }

        if self.config.enable_profiling {
            self.profiler.lock().unwrap().clear_all_data()?;
        }

        self.debug_module.info("调试数据清理完成")?;
        Ok(())
    }

    /// Get debug module reference
    pub fn debug_module(&self) -> &crate::stdlib::debug::DebugModule {
        &self.debug_module
    }

    /// Get configuration
    pub fn config(&self) -> &DebugSystemConfig {
        &self.config
    }

    /// Update configuration
    pub fn update_config(&mut self, config: DebugSystemConfig) -> RuntimeResult<()> {
        self.config = config;

        // Update component configurations
        let stack_config = StackTraceConfig::default();
        // Note: We would need to update the stack collector config here
        // but it requires mutable access, so this is a simplified version

        self.debug_module.info("调试系统配置已更新")?;
        Ok(())
    }
}

impl Default for DebugSystem {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

/// Convenience function to create a debug system with default configuration
pub fn create_debug_system() -> RuntimeResult<DebugSystem> {
    DebugSystem::new()
}

/// Convenience function to capture and format stack trace
pub fn capture_stack_trace(debug_system: &DebugSystem) -> RuntimeResult<String> {
    debug_system.get_formatted_stack_trace()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_debug_system_creation() {
        let debug_system = DebugSystem::new().unwrap();
        let stats = debug_system.get_statistics().unwrap();
        assert_eq!(stats.variable_inspector.registered_variables, 0);
    }

    #[test]
    fn test_debug_system_initialization() {
        let debug_system = DebugSystem::new().unwrap();
        assert!(debug_system.initialize().is_ok());
    }

    #[test]
    fn test_stack_trace_capture() {
        let debug_system = DebugSystem::new().unwrap();
        debug_system.initialize().unwrap();

        let result = debug_system.capture_stack_trace();
        assert!(result.is_ok());

        let frames = result.unwrap();
        assert!(!frames.is_empty());
    }

    #[test]
    fn test_variable_registration() {
        let debug_system = DebugSystem::new().unwrap();
        debug_system.initialize().unwrap();

        let value = 42i32;
        assert!(debug_system.register_variable("test_var", &value).is_ok());

        let variables = debug_system.list_variables().unwrap();
        assert_eq!(variables.len(), 1);
        assert_eq!(variables[0], "test_var");
    }

    #[test]
    fn test_variable_inspection() {
        let debug_system = DebugSystem::new().unwrap();
        debug_system.initialize().unwrap();

        let value = 42i32;
        debug_system.register_variable("test_var", &value).unwrap();

        let result = debug_system.inspect_variable("test_var").unwrap();
        assert_eq!(result.variable.name, "test_var");
        assert_eq!(result.variable.var_type, "i32");
    }

    #[test]
    fn test_debug_commands() {
        let debug_system = DebugSystem::new().unwrap();
        debug_system.initialize().unwrap();

        let result = debug_system.process_command("help");
        assert!(result.is_ok());
    }

    #[test]
    fn test_error_handling() {
        let debug_system = DebugSystem::new().unwrap();
        debug_system.initialize().unwrap();

        let error = RuntimeError::user_error("Test error", "测试错误");
        assert!(debug_system.handle_error(&error).is_ok());
    }

    #[test]
    fn test_statistics() {
        let debug_system = DebugSystem::new().unwrap();
        debug_system.initialize().unwrap();

        let stats = debug_system.get_statistics().unwrap();
        assert_eq!(stats.variable_inspector.registered_variables, 0);
        assert_eq!(stats.commands_processed, 0);
    }

    #[test]
    fn test_disabled_features() {
        let mut config = DebugSystemConfig::default();
        config.enable_stack_traces = false;
        config.enable_variable_inspection = false;

        let debug_system = DebugSystem::with_config(config).unwrap();
        debug_system.initialize().unwrap();

        // These should fail because features are disabled
        assert!(debug_system.capture_stack_trace().is_err());
        assert!(debug_system.register_variable("test", &42i32).is_err());
    }
}
