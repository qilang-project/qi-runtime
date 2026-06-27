//! Debug Command Processing System
//!
//! This module provides a comprehensive command processing system for debugging
//! the Qi runtime, including variable inspection, stack trace analysis, and
//! system monitoring commands with Chinese language support.

use super::{DebugSystem, VariableValue};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

/// Debug command processor
pub struct DebugCommandProcessor {
    /// Debug module for logging
    debug_module: std::sync::Arc<crate::stdlib::debug::DebugModule>,
    /// Command registry
    commands: HashMap<String, Box<dyn DebugCommandHandler>>,
    /// Command statistics
    commands_processed: AtomicU64,
    /// Command history
    history: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    /// Maximum history size
    max_history: usize,
}

/// Command execution result
#[derive(Debug, Clone)]
pub struct CommandResult {
    /// Success status
    pub success: bool,
    /// Result message
    pub message: String,
    /// Additional data (if any)
    pub data: Option<serde_json::Value>,
    /// Execution time (microseconds)
    pub execution_time_us: u64,
}

/// Trait for debug command handlers
pub trait DebugCommandHandler: Send + Sync {
    /// Execute the command
    fn execute(
        &self,
        args: &[String],
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult>;

    /// Get command help
    fn help(&self) -> &'static str;

    /// Get command description
    fn description(&self) -> &'static str;
}

/// Debug command definitions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DebugCommand {
    /// Help command
    Help,
    /// Stack trace command
    StackTrace,
    /// Variable inspection
    Inspect,
    /// List variables
    List,
    /// Register variable
    Register,
    /// Unregister variable
    Unregister,
    /// Clear debugging data
    Clear,
    /// Show statistics
    Stats,
    /// Enable/disable debugging features
    Enable,
    /// Profile commands
    Profile,
    /// Memory information
    Memory,
    /// System information
    System,
    /// Exit debugging
    Exit,
    /// Unknown command
    Unknown,
}

impl DebugCommandProcessor {
    /// Create new command processor
    pub fn new(debug_module: std::sync::Arc<crate::stdlib::debug::DebugModule>) -> Self {
        let mut processor = Self {
            debug_module,
            commands: HashMap::new(),
            commands_processed: AtomicU64::new(0),
            history: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
            max_history: 1000,
        };

        processor.register_builtin_commands();
        processor
    }

    /// Register built-in commands
    fn register_builtin_commands(&mut self) {
        self.register_command("help", Box::new(HelpCommand));
        self.register_command("trace", Box::new(StackTraceCommand));
        self.register_command("stack", Box::new(StackTraceCommand));
        self.register_command("inspect", Box::new(InspectCommand));
        self.register_command("list", Box::new(ListCommand));
        self.register_command("register", Box::new(RegisterCommand));
        self.register_command("unregister", Box::new(UnregisterCommand));
        self.register_command("clear", Box::new(ClearCommand));
        self.register_command("stats", Box::new(StatsCommand));
        self.register_command("enable", Box::new(EnableCommand));
        self.register_command("profile", Box::new(ProfileCommand));
        self.register_command("memory", Box::new(MemoryCommand));
        self.register_command("system", Box::new(SystemCommand));
        self.register_command("exit", Box::new(ExitCommand));
        self.register_command("quit", Box::new(ExitCommand));
    }

    /// Register a command
    pub fn register_command(&mut self, name: &str, handler: Box<dyn DebugCommandHandler>) {
        self.commands.insert(name.to_string(), handler);
        self.debug_module
            .debug(&format!("Registered debug command: {}", name))
            .unwrap();
    }

    /// Process a command string
    pub fn process_command(
        &self,
        command: &str,
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        let start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;

        // Add to history
        {
            let mut history = self.history.lock().unwrap();
            history.push(command.to_string());
            if history.len() > self.max_history {
                history.remove(0);
            }
        }

        // Parse command
        let parts: Vec<String> = command
            .trim()
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        if parts.is_empty() {
            return Ok(CommandResult {
                success: false,
                message: "Empty command".to_string(),
                data: None,
                execution_time_us: 0,
            });
        }

        let command_name = &parts[0].to_lowercase();
        let args = &parts[1..];

        // Execute command
        let result = if let Some(handler) = self.commands.get(command_name) {
            handler.execute(args, debug_system)?
        } else {
            CommandResult {
                success: false,
                message: format!("Unknown command: {}", command_name),
                data: None,
                execution_time_us: 0,
            }
        };

        // Update statistics
        self.commands_processed.fetch_add(1, Ordering::Relaxed);

        let end_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;

        Ok(CommandResult {
            execution_time_us: end_time - start_time,
            ..result
        })
    }

    /// Get command history
    pub fn get_history(&self) -> Vec<String> {
        self.history.lock().unwrap().clone()
    }

    /// Clear command history
    pub fn clear_history(&self) {
        self.history.lock().unwrap().clear();
    }

    /// Get number of commands processed
    pub fn get_commands_processed(&self) -> u64 {
        self.commands_processed.load(Ordering::Relaxed)
    }

    /// List all available commands
    pub fn list_commands(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }
}

// Command implementations

struct HelpCommand;

impl DebugCommandHandler for HelpCommand {
    fn execute(
        &self,
        args: &[String],
        _debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        let help_text = if args.is_empty() {
            format!(
                "Qi 运行时调试命令帮助:\n\
                \n\
                可用命令:\n\
                \n\
                help [command]     - 显示帮助信息\n\
                trace              - 显示当前堆栈跟踪\n\
                inspect <var>      - 检查变量值和类型\n\
                list               - 列出所有已注册变量\n\
                register <var> <val> - 注册变量进行跟踪\n\
                unregister <var>   - 注销变量\n\
                clear              - 清理所有调试数据\n\
                stats              - 显示调试统计信息\n\
                enable <feature>   - 启用调试功能\n\
                profile <command>  - 性能分析命令\n\
                memory             - 显示内存使用信息\n\
                system             - 显示系统信息\n\
                exit/quit          - 退出调试模式\n\
                \n\
                使用 'help <command>' 获取具体命令的详细帮助。"
            )
        } else {
            match args[0].as_str() {
                "trace" => "trace - 显示当前堆栈跟踪\n  显示当前调用堆栈，包括函数名、文件位置和地址信息。".to_string(),
                "inspect" => "inspect <var> - 检查变量\n  显示指定变量的详细信息，包括类型、值、内存地址等。".to_string(),
                "list" => "list - 列出变量\n  显示所有已注册的调试变量。".to_string(),
                "register" => "register <var> <val> - 注册变量\n  注册一个变量进行调试跟踪。值可以是数字、字符串等。".to_string(),
                "clear" => "clear - 清理数据\n  清理所有调试数据，包括变量、堆栈缓存等。".to_string(),
                "stats" => "stats - 统计信息\n  显示调试系统的统计信息，包括内存使用、命令数量等。".to_string(),
                _ => format!("未知命令: {}", args[0]),
            }
        };

        Ok(CommandResult {
            success: true,
            message: help_text,
            data: None,
            execution_time_us: 0,
        })
    }

    fn help(&self) -> &'static str {
        "help [command] - 显示帮助信息"
    }

    fn description(&self) -> &'static str {
        "显示调试命令帮助信息"
    }
}

struct StackTraceCommand;

impl DebugCommandHandler for StackTraceCommand {
    fn execute(
        &self,
        _args: &[String],
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        match debug_system.get_formatted_stack_trace() {
            Ok(trace) => Ok(CommandResult {
                success: true,
                message: "堆栈跟踪收集完成".to_string(),
                data: Some(serde_json::json!({ "stack_trace": trace })),
                execution_time_us: 0,
            }),
            Err(e) => Ok(CommandResult {
                success: false,
                message: format!("无法获取堆栈跟踪: {}", e),
                data: None,
                execution_time_us: 0,
            }),
        }
    }

    fn help(&self) -> &'static str {
        "trace - 显示当前堆栈跟踪"
    }

    fn description(&self) -> &'static str {
        "显示当前调用堆栈信息"
    }
}

struct InspectCommand;

impl DebugCommandHandler for InspectCommand {
    fn execute(
        &self,
        args: &[String],
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        if args.is_empty() {
            return Ok(CommandResult {
                success: false,
                message: "用法: inspect <variable_name>".to_string(),
                data: None,
                execution_time_us: 0,
            });
        }

        match debug_system.inspect_variable(&args[0]) {
            Ok(result) => Ok(CommandResult {
                success: true,
                message: format!("变量 '{}' 检查完成", args[0]),
                data: Some(serde_json::json!({
                    "variable": result.variable.name,
                    "type": result.variable.var_type,
                    "value": result.variable.value,
                    "display": result.display
                })),
                execution_time_us: 0,
            }),
            Err(e) => Ok(CommandResult {
                success: false,
                message: format!("无法检查变量 '{}': {}", args[0], e),
                data: None,
                execution_time_us: 0,
            }),
        }
    }

    fn help(&self) -> &'static str {
        "inspect <variable> - 检查变量"
    }

    fn description(&self) -> &'static str {
        "显示变量的详细信息"
    }
}

struct ListCommand;

impl DebugCommandHandler for ListCommand {
    fn execute(
        &self,
        _args: &[String],
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        match debug_system.list_variables() {
            Ok(variables) => {
                let count = variables.len();
                let display = if variables.is_empty() {
                    "没有已注册的变量".to_string()
                } else {
                    format!("已注册的变量 ({}):\n  {}", count, variables.join("\n  "))
                };

                Ok(CommandResult {
                    success: true,
                    message: display,
                    data: Some(serde_json::json!({ "variables": variables })),
                    execution_time_us: 0,
                })
            }
            Err(e) => Ok(CommandResult {
                success: false,
                message: format!("无法列出变量: {}", e),
                data: None,
                execution_time_us: 0,
            }),
        }
    }

    fn help(&self) -> &'static str {
        "list - 列出所有已注册变量"
    }

    fn description(&self) -> &'static str {
        "显示所有已注册的调试变量"
    }
}

struct RegisterCommand;

impl DebugCommandHandler for RegisterCommand {
    fn execute(
        &self,
        args: &[String],
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        if args.len() < 2 {
            return Ok(CommandResult {
                success: false,
                message: "用法: register <variable_name> <value>".to_string(),
                data: None,
                execution_time_us: 0,
            });
        }

        let var_name = &args[0];
        let value_str = &args[1];

        // Try to parse the value as different types
        let value: Box<dyn VariableValue> = if let Ok(int_val) = value_str.parse::<i32>() {
            Box::new(int_val)
        } else if let Ok(float_val) = value_str.parse::<f64>() {
            Box::new(float_val)
        } else if value_str == "true" {
            Box::new(true)
        } else if value_str == "false" {
            Box::new(false)
        } else {
            Box::new(value_str.clone())
        };

        match debug_system.register_variable(var_name, value.as_ref()) {
            Ok(()) => Ok(CommandResult {
                success: true,
                message: format!("变量 '{}' 已注册", var_name),
                data: Some(serde_json::json!({
                    "variable": var_name,
                    "value": value_str
                })),
                execution_time_us: 0,
            }),
            Err(e) => Ok(CommandResult {
                success: false,
                message: format!("无法注册变量 '{}': {}", var_name, e),
                data: None,
                execution_time_us: 0,
            }),
        }
    }

    fn help(&self) -> &'static str {
        "register <variable> <value> - 注册变量"
    }

    fn description(&self) -> &'static str {
        "注册一个变量进行调试跟踪"
    }
}

struct UnregisterCommand;

impl DebugCommandHandler for UnregisterCommand {
    fn execute(
        &self,
        args: &[String],
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        if args.is_empty() {
            return Ok(CommandResult {
                success: false,
                message: "用法: unregister <variable_name>".to_string(),
                data: None,
                execution_time_us: 0,
            });
        }

        match debug_system
            .variable_inspector
            .unregister_variable(&args[0])
        {
            Ok(()) => Ok(CommandResult {
                success: true,
                message: format!("变量 '{}' 已注销", args[0]),
                data: Some(serde_json::json!({ "variable": args[0] })),
                execution_time_us: 0,
            }),
            Err(e) => Ok(CommandResult {
                success: false,
                message: format!("无法注销变量 '{}': {}", args[0], e),
                data: None,
                execution_time_us: 0,
            }),
        }
    }

    fn help(&self) -> &'static str {
        "unregister <variable> - 注销变量"
    }

    fn description(&self) -> &'static str {
        "注销调试变量"
    }
}

struct ClearCommand;

impl DebugCommandHandler for ClearCommand {
    fn execute(
        &self,
        _args: &[String],
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        match debug_system.clear_all_data() {
            Ok(()) => Ok(CommandResult {
                success: true,
                message: "所有调试数据已清理".to_string(),
                data: None,
                execution_time_us: 0,
            }),
            Err(e) => Ok(CommandResult {
                success: false,
                message: format!("无法清理调试数据: {}", e),
                data: None,
                execution_time_us: 0,
            }),
        }
    }

    fn help(&self) -> &'static str {
        "clear - 清理所有调试数据"
    }

    fn description(&self) -> &'static str {
        "清理所有调试数据"
    }
}

struct StatsCommand;

impl DebugCommandHandler for StatsCommand {
    fn execute(
        &self,
        _args: &[String],
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        match debug_system.get_statistics() {
            Ok(stats) => {
                let display = format!(
                    "调试系统统计信息:\n\
                    \n\
                    已注册变量: {}\n\
                    缓存符号: {}\n\
                    处理命令: {}\n\
                    内存使用: {} bytes\n\
                    堆栈跟踪: {} (最大: {})\n\
                    变量检查: {} (最大深度: {})",
                    stats.variable_inspector.registered_variables,
                    stats.stack_traces.cached_symbols,
                    stats.commands_processed,
                    stats.total_memory_usage,
                    stats.stack_traces.cached_symbols,
                    stats.stack_traces.max_frames,
                    stats.variable_inspector.registered_variables,
                    stats.variable_inspector.max_depth
                );

                Ok(CommandResult {
                    success: true,
                    message: display,
                    data: Some(serde_json::json!({
                        "variables": stats.variable_inspector.registered_variables,
                        "symbols": stats.stack_traces.cached_symbols,
                        "commands": stats.commands_processed,
                        "memory_bytes": stats.total_memory_usage
                    })),
                    execution_time_us: 0,
                })
            }
            Err(e) => Ok(CommandResult {
                success: false,
                message: format!("无法获取统计信息: {}", e),
                data: None,
                execution_time_us: 0,
            }),
        }
    }

    fn help(&self) -> &'static str {
        "stats - 显示调试统计信息"
    }

    fn description(&self) -> &'static str {
        "显示调试系统统计信息"
    }
}

struct EnableCommand;

impl DebugCommandHandler for EnableCommand {
    fn execute(
        &self,
        args: &[String],
        _debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        if args.is_empty() {
            return Ok(CommandResult {
                success: false,
                message:
                    "用法: enable <feature>\n可用功能: stack_traces, variable_inspection, profiling"
                        .to_string(),
                data: None,
                execution_time_us: 0,
            });
        }

        // This is a simplified implementation
        // In a real implementation, you'd update the debug system configuration
        Ok(CommandResult {
            success: true,
            message: format!("功能 '{}' 已启用 (模拟)", args[0]),
            data: Some(serde_json::json!({ "feature": args[0] })),
            execution_time_us: 0,
        })
    }

    fn help(&self) -> &'static str {
        "enable <feature> - 启用调试功能"
    }

    fn description(&self) -> &'static str {
        "启用指定的调试功能"
    }
}

struct ProfileCommand;

impl DebugCommandHandler for ProfileCommand {
    fn execute(
        &self,
        args: &[String],
        debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        if args.is_empty() {
            return Ok(CommandResult {
                success: false,
                message: "用法: profile <start|stop|list> [name]".to_string(),
                data: None,
                execution_time_us: 0,
            });
        }

        match args[0].as_str() {
            "start" => {
                let name = if args.len() > 1 { &args[1] } else { "default" };
                match debug_system.start_profiling(name) {
                    Ok(()) => Ok(CommandResult {
                        success: true,
                        message: format!("性能分析 '{}' 已开始", name),
                        data: Some(serde_json::json!({ "profile": name, "action": "start" })),
                        execution_time_us: 0,
                    }),
                    Err(e) => Ok(CommandResult {
                        success: false,
                        message: format!("无法开始性能分析: {}", e),
                        data: None,
                        execution_time_us: 0,
                    }),
                }
            }
            "stop" => {
                let name = if args.len() > 1 { &args[1] } else { "default" };
                match debug_system.stop_profiling(name) {
                    Ok(data) => Ok(CommandResult {
                        success: true,
                        message: format!("性能分析 '{}' 已停止", name),
                        data: Some(serde_json::json!({
                            "profile": name,
                            "action": "stop",
                            "data": data
                        })),
                        execution_time_us: 0,
                    }),
                    Err(e) => Ok(CommandResult {
                        success: false,
                        message: format!("无法停止性能分析: {}", e),
                        data: None,
                        execution_time_us: 0,
                    }),
                }
            }
            "list" => match debug_system.get_profile_data() {
                Ok(profiles) => {
                    let count = profiles.len();
                    Ok(CommandResult {
                        success: true,
                        message: format!("找到 {} 个性能分析数据", count),
                        data: Some(serde_json::json!({ "profiles": count })),
                        execution_time_us: 0,
                    })
                }
                Err(e) => Ok(CommandResult {
                    success: false,
                    message: format!("无法获取性能分析数据: {}", e),
                    data: None,
                    execution_time_us: 0,
                }),
            },
            _ => Ok(CommandResult {
                success: false,
                message: "未知的性能分析命令。使用: start, stop, 或 list".to_string(),
                data: None,
                execution_time_us: 0,
            }),
        }
    }

    fn help(&self) -> &'static str {
        "profile <start|stop|list> [name] - 性能分析"
    }

    fn description(&self) -> &'static str {
        "性能分析控制"
    }
}

struct MemoryCommand;

impl DebugCommandHandler for MemoryCommand {
    fn execute(
        &self,
        _args: &[String],
        _debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        // This is a simplified implementation
        // In a real implementation, you'd gather actual memory usage information
        let memory_info = format!(
            "内存使用信息:\n\
            \n\
            调试系统内存: ~{} KB\n\
            变量存储: ~{} KB\n\
            堆栈缓存: ~{} KB\n\
            系统总内存: 检查中...",
            1024, // Mock values
            512,
            256
        );

        Ok(CommandResult {
            success: true,
            message: memory_info,
            data: Some(serde_json::json!({
                "debug_memory_kb": 1024,
                "variable_memory_kb": 512,
                "stack_memory_kb": 256
            })),
            execution_time_us: 0,
        })
    }

    fn help(&self) -> &'static str {
        "memory - 显示内存使用信息"
    }

    fn description(&self) -> &'static str {
        "显示内存使用情况"
    }
}

struct SystemCommand;

impl DebugCommandHandler for SystemCommand {
    fn execute(
        &self,
        _args: &[String],
        _debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        let system_info = format!(
            "系统信息:\n\
            \n\
            Qi 运行时版本: {}\n\
            操作系统: {}\n\
            架构: {}\n\
            启动时间: {}\n\
            运行时间: {} 秒\n\
            当前线程: {}",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            "检查中...", // Would need actual startup time tracking
            "检查中...", // Would need actual runtime tracking
            "主线程"
        );

        Ok(CommandResult {
            success: true,
            message: system_info,
            data: Some(serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "os": std::env::consts::OS,
                "arch": std::env::consts::ARCH
            })),
            execution_time_us: 0,
        })
    }

    fn help(&self) -> &'static str {
        "system - 显示系统信息"
    }

    fn description(&self) -> &'static str {
        "显示系统和运行时信息"
    }
}

struct ExitCommand;

impl DebugCommandHandler for ExitCommand {
    fn execute(
        &self,
        _args: &[String],
        _debug_system: &DebugSystem,
    ) -> super::RuntimeResult<CommandResult> {
        Ok(CommandResult {
            success: true,
            message: "退出调试模式".to_string(),
            data: Some(serde_json::json!({ "action": "exit" })),
            execution_time_us: 0,
        })
    }

    fn help(&self) -> &'static str {
        "exit/quit - 退出调试模式"
    }

    fn description(&self) -> &'static str {
        "退出调试模式"
    }
}

// Implement VariableValue for common types used in commands
impl VariableValue for f64 {
    fn get_type_name(&self) -> &str {
        "f64"
    }
    fn to_string(&self) -> String {
        format!("{:?}", self)
    }
    fn get_address(&self) -> Option<usize> {
        Some(self as *const f64 as usize)
    }
    fn get_size(&self) -> Option<usize> {
        Some(8)
    }
    fn get_metadata(&self) -> super::VariableMetadata {
        super::VariableMetadata::Float {
            bits: 64,
            precision: 15,
            min: Some(f64::MIN),
            max: Some(f64::MAX),
        }
    }
}

impl VariableValue for bool {
    fn get_type_name(&self) -> &str {
        "bool"
    }
    fn to_string(&self) -> String {
        format!("{:?}", self)
    }
    fn get_address(&self) -> Option<usize> {
        Some(self as *const bool as usize)
    }
    fn get_size(&self) -> Option<usize> {
        Some(1)
    }
    fn get_metadata(&self) -> super::VariableMetadata {
        super::VariableMetadata::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_command_processor_creation() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let processor = DebugCommandProcessor::new(debug_module);

        let commands = processor.list_commands();
        assert!(commands.contains(&"help".to_string()));
        assert!(commands.contains(&"trace".to_string()));
        assert!(commands.contains(&"list".to_string()));
    }

    #[test]
    fn test_help_command() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let processor = DebugCommandProcessor::new(debug_module);
        let debug_system = DebugSystem::new().unwrap();

        let result = processor.process_command("help", &debug_system).unwrap();
        assert!(result.success);
        assert!(result.message.contains("可用命令"));
    }

    #[test]
    fn test_unknown_command() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let processor = DebugCommandProcessor::new(debug_module);
        let debug_system = DebugSystem::new().unwrap();

        let result = processor
            .process_command("unknown_command", &debug_system)
            .unwrap();
        assert!(!result.success);
        assert!(result.message.contains("Unknown command"));
    }

    #[test]
    fn test_command_history() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let processor = DebugCommandProcessor::new(debug_module);
        let debug_system = DebugSystem::new().unwrap();

        processor.process_command("help", &debug_system).unwrap();
        processor.process_command("list", &debug_system).unwrap();

        let history = processor.get_history();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0], "help");
        assert_eq!(history[1], "list");
    }

    #[test]
    fn test_commands_processed() {
        let debug_module = Arc::new(crate::stdlib::debug::DebugModule::new());
        let processor = DebugCommandProcessor::new(debug_module);
        let debug_system = DebugSystem::new().unwrap();

        assert_eq!(processor.get_commands_processed(), 0);

        processor.process_command("help", &debug_system).unwrap();
        processor.process_command("list", &debug_system).unwrap();

        assert_eq!(processor.get_commands_processed(), 2);
    }
}
