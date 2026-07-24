//! Qi Basic Runtime Environment
//!
//! This module provides the foundational runtime environment for executing compiled Qi programs.
//! It includes memory management, I/O operations, standard library functions, and comprehensive
//! Chinese language support.
//!
//! # Features
//!
//! - **Memory Management**: Stack/heap hybrid allocation with tracing GC support
//! - **I/O Operations**: Synchronous file and network operations with Chinese keyword support
//! - **Standard Library**: Built-in functions for strings, math, and system operations
//! - **Error Handling**: Comprehensive Chinese error message system
//! - **Cross-Platform**: Support for Linux, Windows, and macOS
//!
//! # Usage
//!
//! ```rust,ignore
//! use qi_compiler::runtime::{RuntimeEnvironment, RuntimeConfig};
//!
//! let config = RuntimeConfig::default();
//! let mut runtime = RuntimeEnvironment::new(config).unwrap();
//! runtime.initialize().unwrap();
//! // runtime.execute_program(program_data).unwrap();
//! ```

// GUI 特性开启时把 qi-gui 拉入链接图，使其 #[no_mangle] qi_gui_*_impl 符号
// 编入 libqi_runtime.a（供 stdlib::gui_ffi 的包装转调，含老 tao API 与 egui 控件层）。
#[cfg(feature = "gui")]
extern crate qi_gui;

pub mod async_runtime;
pub mod debug;
pub mod environment;
pub mod error;
pub mod executor;
pub mod io;
pub mod memory;
pub mod runtime_worker;
pub mod stdlib;
pub mod tool_control;

// Legacy modules for backward compatibility
pub mod strings;

// Runtime library with C FFI exports (temporarily disabled due to duplicate symbols)
// pub mod lib;

// Re-export core components for convenience
pub use async_runtime::{
    Runtime as AsyncRuntime, RuntimeConfig as AsyncRuntimeConfig, RuntimeStats as AsyncRuntimeStats,
};
pub use debug::{create_debug_system, DebugSystem, DebugSystemConfig};
pub use environment::{RuntimeConfig, RuntimeEnvironment, RuntimeState};
pub use error::{ChineseErrorMessages, ErrorHandler};
pub use io::{FileSystemInterface, NetworkManager};
pub use memory::{AllocationStrategy, MemoryManager};
pub use runtime_worker::{WorkerMessage, WORKER_PROTOCOL_VERSION};
pub use stdlib::{MathModule, StandardLibrary, StringModule};
pub use tool_control::{FinishRecord, WaitResult};
// Re-export async runtime FFI functions
pub use async_runtime::ffi::{qi_runtime_await, qi_runtime_create_task, qi_runtime_spawn_task};

/// Runtime version information
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Runtime build timestamp
pub const BUILD_TIMESTAMP: &str = "2025-01-22";

/// Result type for runtime operations
pub type RuntimeResult<T> = Result<T, RuntimeError>;

/// Core runtime error type - unified with error module
pub type RuntimeError = error::Error;

/// Runtime library interface
pub struct RuntimeLibrary {
    memory_interface: memory::MemoryInterface,
    string_interface: strings::StringInterface,
    io_interface: io::IoInterface,
}

impl RuntimeLibrary {
    /// Create a new runtime library interface
    pub fn new() -> Result<Self, RuntimeError> {
        Ok(Self {
            memory_interface: memory::MemoryInterface::new()?,
            string_interface: strings::StringInterface::new(),
            io_interface: io::IoInterface::new()?,
        })
    }

    /// Initialize the runtime library
    pub fn initialize(&mut self) -> Result<(), RuntimeError> {
        self.memory_interface.initialize()?;
        self.string_interface.initialize()?;
        self.io_interface.initialize()?;
        Ok(())
    }

    /// Get memory management interface
    pub fn memory(&self) -> &memory::MemoryInterface {
        &self.memory_interface
    }

    /// Get mutable memory management interface
    pub fn memory_mut(&mut self) -> &mut memory::MemoryInterface {
        &mut self.memory_interface
    }

    /// Get string operations interface
    pub fn strings(&self) -> &strings::StringInterface {
        &self.string_interface
    }

    /// Get I/O operations interface
    pub fn io(&self) -> &io::IoInterface {
        &self.io_interface
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_info() {
        assert!(!VERSION.is_empty());
        assert!(!BUILD_TIMESTAMP.is_empty());
    }

    #[test]
    fn test_runtime_error_display() {
        let error = RuntimeError::program_execution_error("测试错误消息", "测试错误消息");
        assert!(error.to_string().contains("测试错误消息"));
    }

    #[test]
    fn test_runtime_library_initialization() {
        let mut runtime = RuntimeLibrary::new().unwrap();
        assert!(runtime.initialize().is_ok());
    }

    #[test]
    fn test_memory_operations() {
        let mut runtime = RuntimeLibrary::new().unwrap();
        runtime.initialize().unwrap();

        let memory = runtime.memory();
        assert_eq!(memory.get_allocated_bytes(), 0);
    }

    #[test]
    fn test_string_operations() {
        let mut runtime = RuntimeLibrary::new().unwrap();
        runtime.initialize().unwrap();

        let strings = runtime.strings();

        // Test string length
        assert_eq!(strings.length("你好").unwrap(), 2);
        assert_eq!(strings.length("Hello").unwrap(), 5);

        // Test string concatenation
        assert_eq!(
            strings
                .concat(&[String::from("你好"), String::from("世界")])
                .unwrap(),
            "你好世界"
        );

        // Test string comparison
        assert_eq!(strings.compare("你好", "你好").unwrap(), 0);
        let result = strings.compare("你好", "世界").unwrap();
        assert!(result != 0, "Comparison should not be equal");
    }

    #[test]
    fn test_io_operations() {
        let mut runtime = RuntimeLibrary::new().unwrap();
        runtime.initialize().unwrap();

        let io = runtime.io();

        // Test printing (should not panic)
        assert!(io.print("Hello").is_ok());
        assert!(io.println_int(42).is_ok());
        assert!(io.println_float(3.14).is_ok());
    }

    #[test]
    fn test_memory_allocation() {
        let mut runtime = RuntimeLibrary::new().unwrap();
        runtime.initialize().unwrap();

        let memory = runtime.memory_mut();

        // Test allocation (using unsafe for testing)
        let ptr = memory.allocate(1024);
        assert!(ptr.is_ok());

        if let Ok(allocated_ptr) = ptr {
            assert_eq!(memory.get_allocated_bytes(), 1024);

            // Test deallocation
            assert!(memory.deallocate(allocated_ptr, 1024).is_ok());
            assert_eq!(memory.get_allocated_bytes(), 0);
        }
    }
}
