//! Standard Library Functions
//!
//! This module provides built-in functions for common operations including
//! string manipulation, mathematical operations, system information access,
//! and type conversions with comprehensive Chinese language support.

pub mod bytes_ffi;
pub mod cli_ffi;
pub mod closure_ffi;
pub mod compress_ffi;
pub mod config_ffi;
pub mod conversion;
pub mod crypto;
pub mod crypto_ffi;
pub mod database_ffi;
pub mod datetime;
pub mod debug;
pub mod env_ffi;
pub mod exception_ffi;
pub mod gui_ffi;
pub mod hashmap;
pub mod json_ffi;
pub mod list;
pub mod llm;
pub mod llm_ffi;
pub mod math;
pub mod mcp;
pub mod mcp_client_ffi;
pub mod mcp_ffi;
pub mod multipart_ffi;
pub mod os_ffi;
pub mod path_ffi;
pub mod process_ffi;
pub mod qi_str;
pub mod qi_str_ffi;
pub mod random_ffi;
pub mod rc_obj;
pub mod regex_ffi;
pub mod signal_ffi;
pub mod string;
pub mod string_ffi;
pub mod subprocess_ffi;
pub mod sync_ffi;
pub mod system;
pub mod test_ffi;
pub mod vector;
pub mod vector_ffi;
pub mod web_ffi;

// Re-export main components
pub use conversion::{ConversionModule, TypeConversion};
pub use crypto::{加密操作, 加密模块, 编码格式};
pub use debug::{DebugInfo, DebugModule};
pub use llm::{
    大模型模块, 嵌入器, 提示模板, 智能代理, 检索增强生成, 知识库
};
pub use math::{MathModule, MathOperation};
pub use mcp::{
    MCP工具, MCP提示, MCP服务器, MCP服务器模块, MCP资源, 工具参数, 工具回调函数, 资源内容, 资源类型,
};
pub use string::{StringModule, StringOperation};
pub use system::{SystemInfo, SystemModule};
pub use vector::{向量, 向量模块};
// StandardLibrary is defined below, no need to re-export

/// Standard library result type
pub type StdlibResult<T> = Result<T, StdlibError>;

/// Standard library errors
#[derive(Debug, thiserror::Error)]
pub enum StdlibError {
    #[error("字符串操作错误: {operation} - {message}")]
    StringOperationError { operation: String, message: String },

    #[error("数学运算错误: {operation} - {message}")]
    MathError { operation: String, message: String },

    #[error("系统调用错误: {system_call} - {message}")]
    SystemError {
        system_call: String,
        message: String,
    },

    #[error("类型转换错误: {from_type} -> {to_type} - {message}")]
    ConversionError {
        from_type: String,
        to_type: String,
        message: String,
    },

    #[error("加密操作错误: {operation} - {message}")]
    CryptoError { operation: String, message: String },

    #[error("无效参数: {parameter} - {message}")]
    InvalidParameter { parameter: String, message: String },

    #[error("索引越界: 索引 {index}，长度 {length}")]
    IndexOutOfBounds { index: usize, length: usize },

    #[error("除零错误")]
    DivisionByZero,

    #[error("数值溢出: {operation}")]
    NumericOverflow { operation: String },
}

/// Standard library function signature
pub type StdlibFunction = fn(&[StdlibValue]) -> StdlibResult<StdlibValue>;

/// Standard library value types
#[derive(Debug, Clone, PartialEq)]
pub enum StdlibValue {
    /// Null/None value
    Null,
    /// Boolean value
    Boolean(bool),
    /// Integer value
    Integer(i64),
    /// Floating point value
    Float(f64),
    /// String value
    String(String),
    /// Array of values
    Array(Vec<StdlibValue>),
    /// Object/map of values
    Object(std::collections::HashMap<String, StdlibValue>),
}

impl StdlibValue {
    /// Get the type name as string
    pub fn type_name(&self) -> &'static str {
        match self {
            StdlibValue::Null => "null",
            StdlibValue::Boolean(_) => "boolean",
            StdlibValue::Integer(_) => "integer",
            StdlibValue::Float(_) => "float",
            StdlibValue::String(_) => "string",
            StdlibValue::Array(_) => "array",
            StdlibValue::Object(_) => "object",
        }
    }

    /// Check if value is null
    pub fn is_null(&self) -> bool {
        matches!(self, StdlibValue::Null)
    }

    /// Convert to string representation
    pub fn to_string(&self) -> String {
        match self {
            StdlibValue::Null => "null".to_string(),
            StdlibValue::Boolean(b) => b.to_string(),
            StdlibValue::Integer(i) => i.to_string(),
            StdlibValue::Float(f) => f.to_string(),
            StdlibValue::String(s) => s.clone(),
            StdlibValue::Array(arr) => {
                let items: Vec<String> = arr.iter().map(|v| v.to_string()).collect();
                format!("[{}]", items.join(", "))
            }
            StdlibValue::Object(obj) => {
                let items: Vec<String> = obj
                    .iter()
                    .map(|(k, v)| format!("{}: {}", k, v.to_string()))
                    .collect();
                format!("{{{}}}", items.join(", "))
            }
        }
    }

    /// Try to convert to boolean
    pub fn as_boolean(&self) -> Option<bool> {
        match self {
            StdlibValue::Boolean(b) => Some(*b),
            _ => None,
        }
    }

    /// Try to convert to integer
    pub fn as_integer(&self) -> Option<i64> {
        match self {
            StdlibValue::Integer(i) => Some(*i),
            StdlibValue::Float(f) => Some(*f as i64),
            _ => None,
        }
    }

    /// Try to convert to float
    pub fn as_float(&self) -> Option<f64> {
        match self {
            StdlibValue::Float(f) => Some(*f),
            StdlibValue::Integer(i) => Some(*i as f64),
            _ => None,
        }
    }

    /// Try to convert to string
    pub fn as_string(&self) -> Option<String> {
        match self {
            StdlibValue::String(s) => Some(s.clone()),
            _ => Some(self.to_string()),
        }
    }

    /// Try to convert to array
    pub fn as_array(&self) -> Option<&Vec<StdlibValue>> {
        match self {
            StdlibValue::Array(arr) => Some(arr),
            _ => None,
        }
    }

    /// Try to convert to object
    pub fn as_object(&self) -> Option<&std::collections::HashMap<String, StdlibValue>> {
        match self {
            StdlibValue::Object(obj) => Some(obj),
            _ => None,
        }
    }
}

impl From<bool> for StdlibValue {
    fn from(value: bool) -> Self {
        StdlibValue::Boolean(value)
    }
}

impl From<i64> for StdlibValue {
    fn from(value: i64) -> Self {
        StdlibValue::Integer(value)
    }
}

impl From<f64> for StdlibValue {
    fn from(value: f64) -> Self {
        StdlibValue::Float(value)
    }
}

impl From<String> for StdlibValue {
    fn from(value: String) -> Self {
        StdlibValue::String(value)
    }
}

impl From<&str> for StdlibValue {
    fn from(value: &str) -> Self {
        StdlibValue::String(value.to_string())
    }
}

impl<T: Into<StdlibValue>> From<Vec<T>> for StdlibValue {
    fn from(value: Vec<T>) -> Self {
        StdlibValue::Array(value.into_iter().map(|v| v.into()).collect())
    }
}

/// Standard library function registry
#[derive(Debug)]
pub struct StdlibRegistry {
    functions: std::collections::HashMap<String, StdlibFunction>,
}

impl StdlibRegistry {
    /// Create new function registry
    pub fn new() -> Self {
        Self {
            functions: std::collections::HashMap::new(),
        }
    }

    /// Register a function
    pub fn register(&mut self, name: &str, function: StdlibFunction) {
        self.functions.insert(name.to_string(), function);
    }

    /// Get a function by name
    pub fn get(&self, name: &str) -> Option<StdlibFunction> {
        self.functions.get(name).copied()
    }

    /// Check if function exists
    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_key(name)
    }

    /// Get all function names
    pub fn function_names(&self) -> Vec<&String> {
        self.functions.keys().collect()
    }

    /// Clear all functions
    pub fn clear(&mut self) {
        self.functions.clear();
    }
}

impl Default for StdlibRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Standard library container
#[derive(Debug)]
#[allow(non_snake_case)]
pub struct StandardLibrary {
    /// String module
    pub string_module: StringModule,
    /// Math module
    pub math_module: MathModule,
    /// System module
    pub system_module: SystemModule,
    /// Conversion module
    pub conversion_module: ConversionModule,
    /// Debug module
    pub debug_module: DebugModule,
    /// 加密模块
    pub 加密模块: 加密模块,
    /// 向量模块
    pub 向量模块: 向量模块,
    /// 大模型模块
    pub 大模型: 大模型模块,
    /// MCP服务器模块
    pub MCP服务器: MCP服务器模块,
    /// Function registry
    pub registry: StdlibRegistry,
}

impl StandardLibrary {
    /// Create new standard library
    pub fn new() -> StdlibResult<Self> {
        Ok(Self {
            string_module: StringModule::new(),
            math_module: MathModule::new(),
            system_module: SystemModule::new(),
            conversion_module: ConversionModule::new(),
            debug_module: DebugModule::new(),
            加密模块: 加密模块::创建(),
            向量模块: 向量模块::创建(),
            大模型: 大模型模块::创建(),
            MCP服务器: MCP服务器模块::创建(),
            registry: StdlibRegistry::new(),
        })
    }

    /// Initialize standard library with all built-in functions
    pub fn initialize(&mut self) -> StdlibResult<()> {
        self.register_built_in_functions()?;
        Ok(())
    }

    /// Register built-in functions
    fn register_built_in_functions(&mut self) -> StdlibResult<()> {
        // Register string functions
        self.registry.register("concat", |args| {
            if args.len() < 2 {
                return Err(StdlibError::InvalidParameter {
                    parameter: "args".to_string(),
                    message: "concat requires at least 2 arguments".to_string(),
                });
            }

            let mut result = String::new();
            for arg in args {
                if let Some(s) = arg.as_string() {
                    result.push_str(&s);
                } else {
                    return Err(StdlibError::StringOperationError {
                        operation: "concat".to_string(),
                        message: "all arguments must be strings".to_string(),
                    });
                }
            }

            Ok(StdlibValue::String(result))
        });

        // Register math functions
        self.registry.register("add", |args| {
            if args.len() != 2 {
                return Err(StdlibError::InvalidParameter {
                    parameter: "args".to_string(),
                    message: "add requires exactly 2 arguments".to_string(),
                });
            }

            let a = args[0].as_float().ok_or_else(|| StdlibError::MathError {
                operation: "add".to_string(),
                message: "arguments must be numbers".to_string(),
            })?;

            let b = args[1].as_float().ok_or_else(|| StdlibError::MathError {
                operation: "add".to_string(),
                message: "arguments must be numbers".to_string(),
            })?;

            // If both inputs are integers, return integer
            if let (Some(a_int), Some(b_int)) = (args[0].as_integer(), args[1].as_integer()) {
                Ok(StdlibValue::Integer(a_int + b_int))
            } else {
                Ok(StdlibValue::Float(a + b))
            }
        });

        // Register conversion functions
        self.registry.register("to_string", |args| {
            if args.len() != 1 {
                return Err(StdlibError::InvalidParameter {
                    parameter: "args".to_string(),
                    message: "to_string requires exactly 1 argument".to_string(),
                });
            }

            Ok(StdlibValue::String(args[0].to_string()))
        });

        // Register system functions
        self.registry.register("timestamp", |_args| {
            use std::time::{SystemTime, UNIX_EPOCH};
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs();
            Ok(StdlibValue::Integer(timestamp as i64))
        });

        Ok(())
    }

    /// Get function by name
    pub fn get_function(&self, name: &str) -> Option<StdlibFunction> {
        self.registry.get(name)
    }

    /// Get all function names
    pub fn get_function_names(&self) -> Vec<&String> {
        self.registry.function_names()
    }
}

impl Default for StandardLibrary {
    fn default() -> Self {
        Self::new().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stdlib_value_types() {
        let null_val = StdlibValue::Null;
        assert_eq!(null_val.type_name(), "null");
        assert!(null_val.is_null());

        let bool_val = StdlibValue::Boolean(true);
        assert_eq!(bool_val.type_name(), "boolean");
        assert_eq!(bool_val.as_boolean(), Some(true));

        let int_val = StdlibValue::Integer(42);
        assert_eq!(int_val.type_name(), "integer");
        assert_eq!(int_val.as_integer(), Some(42));
        assert_eq!(int_val.as_float(), Some(42.0));

        let float_val = StdlibValue::Float(3.14);
        assert_eq!(float_val.type_name(), "float");
        assert_eq!(float_val.as_float(), Some(3.14));

        let string_val = StdlibValue::String("测试".to_string());
        assert_eq!(string_val.type_name(), "string");
        assert_eq!(string_val.as_string(), Some("测试".to_string()));
    }

    #[test]
    fn test_stdlib_value_conversions() {
        let int_val: StdlibValue = 42.into();
        assert!(matches!(int_val, StdlibValue::Integer(42)));

        let float_val: StdlibValue = 3.14.into();
        assert!(matches!(float_val, StdlibValue::Float(3.14)));

        let string_val: StdlibValue = "测试".into();
        assert!(matches!(string_val, StdlibValue::String(s) if s == "测试"));

        let bool_val: StdlibValue = true.into();
        assert!(matches!(bool_val, StdlibValue::Boolean(true)));
    }

    #[test]
    fn test_stdlib_registry() {
        let mut registry = StdlibRegistry::new();
        assert!(!registry.contains("test_func"));

        // Register a test function
        fn test_func(_args: &[StdlibValue]) -> StdlibResult<StdlibValue> {
            Ok(StdlibValue::Integer(42))
        }

        registry.register("test_func", test_func);
        assert!(registry.contains("test_func"));

        let func = registry.get("test_func");
        assert!(func.is_some());

        let result = func.unwrap()(&[]);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), StdlibValue::Integer(42));
    }

    #[test]
    fn test_stdlib_error_display() {
        let error = StdlibError::StringOperationError {
            operation: "concat".to_string(),
            message: "无效的字符串参数".to_string(),
        };
        let message = error.to_string();
        assert!(message.contains("concat"));
        assert!(message.contains("无效的字符串参数"));
    }
}
