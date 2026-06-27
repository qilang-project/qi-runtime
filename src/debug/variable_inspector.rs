//! Variable Inspection System
//!
//! This module provides comprehensive variable inspection capabilities for debugging,
//! including type information, memory layout, and value visualization for Qi runtime values.

use crate::stdlib::DebugModule;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// Variable inspector for runtime debugging
#[derive(Debug)]
pub struct VariableInspector {
    /// Debug module for logging
    debug: Arc<DebugModule>,
    /// Variable registry for tracking variables
    variable_registry: Arc<Mutex<HashMap<String, VariableInfo>>>,
    /// Type information cache
    type_cache: Arc<Mutex<HashMap<String, TypeInfo>>>,
    /// Memory layout cache
    memory_cache: Arc<Mutex<HashMap<usize, MemoryLayout>>>,
    /// Configuration
    config: InspectorConfig,
}

/// Variable inspector configuration
#[derive(Debug, Clone)]
pub struct InspectorConfig {
    /// Maximum depth for nested inspection
    pub max_depth: usize,
    /// Maximum string length to display
    pub max_string_length: usize,
    /// Maximum array elements to display
    pub max_array_elements: usize,
    /// Include memory addresses in output
    pub include_memory_addresses: bool,
    /// Include type information in output
    pub include_type_info: bool,
    /// Pretty print formatted output
    pub pretty_print: bool,
    /// Enable variable change tracking
    pub enable_change_tracking: bool,
}

impl Default for InspectorConfig {
    fn default() -> Self {
        Self {
            max_depth: 5,
            max_string_length: 100,
            max_array_elements: 10,
            include_memory_addresses: true,
            include_type_info: true,
            pretty_print: true,
            enable_change_tracking: false,
        }
    }
}

/// Variable information with full debugging details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableInfo {
    /// Variable name
    pub name: String,
    /// Variable type
    pub var_type: String,
    /// Current value as string
    pub value: String,
    /// Memory address (if applicable)
    pub address: Option<usize>,
    /// Size in bytes
    pub size: Option<usize>,
    /// Type-specific metadata
    pub metadata: VariableMetadata,
    /// Variable scope
    pub scope: VariableScope,
    /// Modification history (if tracking enabled)
    pub history: Vec<VariableSnapshot>,
    /// Creation timestamp
    pub created_at: u64,
    /// Last modification timestamp
    pub modified_at: u64,
}

/// Type-specific variable metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VariableMetadata {
    /// Integer metadata
    Integer {
        signed: bool,
        bits: u8,
        min: Option<i64>,
        max: Option<u64>,
    },
    /// Float metadata
    Float {
        bits: u8,
        precision: u8,
        min: Option<f64>,
        max: Option<f64>,
    },
    /// String metadata
    String {
        length: usize,
        encoding: String,
        is_utf8: bool,
    },
    /// Array metadata
    Array {
        element_type: String,
        length: usize,
        capacity: usize,
    },
    /// Struct metadata
    Struct {
        fields: Vec<String>,
        field_types: HashMap<String, String>,
    },
    /// Function metadata
    Function {
        parameters: Vec<String>,
        return_type: String,
        is_closure: bool,
    },
    /// Pointer metadata
    Pointer {
        target_type: String,
        is_null: bool,
        target_address: Option<usize>,
    },
    /// Unknown metadata
    Unknown,
}

/// Variable scope information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VariableScope {
    /// Global variable
    Global,
    /// Local variable in function
    Local { function: String, line: u32 },
    /// Parameter in function
    Parameter { function: String, index: usize },
    /// Member of struct/object
    Member { parent: String, field: String },
    /// Unknown scope
    Unknown,
}

/// Variable snapshot for change tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableSnapshot {
    /// Value at this point in time
    pub value: String,
    /// Timestamp of snapshot
    pub timestamp: u64,
    /// Optional change description
    pub description: Option<String>,
}

/// Type information for variables
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypeInfo {
    /// Type name
    pub name: String,
    /// Type category
    pub category: TypeCategory,
    /// Size in bytes
    pub size: usize,
    /// Alignment requirements
    pub alignment: usize,
    /// Type-specific details
    pub details: TypeDetails,
}

/// Type categories
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TypeCategory {
    /// Primitive types (int, float, bool, etc.)
    Primitive,
    /// Composite types (struct, union)
    Composite,
    /// Collection types (array, list, map)
    Collection,
    /// Reference types (pointer, reference)
    Reference,
    /// Function types
    Function,
    /// User-defined types
    UserDefined,
    /// Unknown type
    Unknown,
}

/// Type-specific details
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TypeDetails {
    /// Primitive details
    Primitive { signed: bool, bits: u8 },
    /// Array details
    Array {
        element_type: String,
        fixed_length: Option<usize>,
    },
    /// Struct details
    Struct { fields: HashMap<String, String> },
    /// Function details
    Function {
        parameters: Vec<String>,
        return_type: String,
    },
    /// No specific details
    None,
}

/// Memory layout information
#[derive(Debug, Clone)]
pub struct MemoryLayout {
    /// Base address
    pub base_address: usize,
    /// Total size
    pub size: usize,
    /// Field offsets (for structs)
    pub field_offsets: HashMap<String, usize>,
    /// Alignment padding
    pub padding: Vec<(usize, usize)>, // (offset, size)
}

/// Inspection result with formatted output
#[derive(Debug, Clone)]
pub struct InspectionResult {
    /// Variable information
    pub variable: VariableInfo,
    /// Formatted display string
    pub display: String,
    /// Nested variables (for complex types)
    pub nested: HashMap<String, InspectionResult>,
    /// Inspection metadata
    pub metadata: InspectionMetadata,
}

/// Inspection metadata
#[derive(Debug, Clone)]
pub struct InspectionMetadata {
    /// Inspection depth
    pub depth: usize,
    /// Number of nested variables
    pub nested_count: usize,
    /// Total inspection time (microseconds)
    pub inspection_time_us: u64,
    /// Memory usage during inspection
    pub memory_usage_bytes: usize,
}

impl VariableInspector {
    /// Create new variable inspector
    pub fn new(debug: Arc<DebugModule>) -> Self {
        Self::with_config(debug, InspectorConfig::default())
    }

    /// Create variable inspector with configuration
    pub fn with_config(debug: Arc<DebugModule>, config: InspectorConfig) -> Self {
        Self {
            debug,
            variable_registry: Arc::new(Mutex::new(HashMap::new())),
            type_cache: Arc::new(Mutex::new(HashMap::new())),
            memory_cache: Arc::new(Mutex::new(HashMap::new())),
            config,
        }
    }

    /// Register a variable for tracking
    pub fn register_variable(&self, name: &str, value: &dyn VariableValue) -> RuntimeResult<()> {
        let info = self.create_variable_info(name, value)?;

        {
            let mut registry = self.variable_registry.lock().unwrap();
            registry.insert(name.to_string(), info);
        }

        self.debug
            .debug(&format!("Registered variable: {}", name))?;
        Ok(())
    }

    /// Inspect a variable by name
    pub fn inspect_variable(&self, name: &str) -> RuntimeResult<InspectionResult> {
        let start_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_micros() as u64;

        {
            let registry = self.variable_registry.lock().unwrap();
            if let Some(variable) = registry.get(name) {
                let result = self.create_inspection_result(variable, 0)?;

                let end_time = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_micros() as u64;

                self.debug.info(&format!("Inspected variable: {}", name))?;
                return Ok(result);
            }
        }

        Err(crate::error::Error::user_error(
            format!("Variable '{}' not found", name),
            format!("变量 '{}' 未找到", name),
        ))
    }

    /// Inspect a value directly
    pub fn inspect_value(
        &self,
        name: &str,
        value: &dyn VariableValue,
    ) -> RuntimeResult<InspectionResult> {
        let info = self.create_variable_info(name, value)?;
        self.create_inspection_result(&info, 0)
    }

    /// Update a registered variable's value
    pub fn update_variable(&self, name: &str, new_value: &dyn VariableValue) -> RuntimeResult<()> {
        let mut registry = self.variable_registry.lock().unwrap();

        if let Some(variable) = registry.get_mut(name) {
            // Create snapshot if change tracking is enabled
            if self.config.enable_change_tracking {
                let snapshot = VariableSnapshot {
                    value: variable.value.clone(),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                    description: Some("Value updated".to_string()),
                };
                variable.history.push(snapshot);
            }

            // Update variable info
            let new_info = self.create_variable_info(name, new_value)?;
            variable.value = new_info.value;
            variable.metadata = new_info.metadata;
            variable.size = new_info.size;
            variable.modified_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            self.debug.debug(&format!("Updated variable: {}", name))?;
            Ok(())
        } else {
            Err(crate::error::Error::user_error(
                format!("Variable '{}' not found for update", name),
                format!("变量 '{}' 未找到用于更新", name),
            ))
        }
    }

    /// List all registered variables
    pub fn list_variables(&self) -> RuntimeResult<Vec<String>> {
        let registry = self.variable_registry.lock().unwrap();
        let names: Vec<String> = registry.keys().cloned().collect();
        Ok(names)
    }

    /// Get variable history
    pub fn get_variable_history(&self, name: &str) -> RuntimeResult<Vec<VariableSnapshot>> {
        let registry = self.variable_registry.lock().unwrap();

        if let Some(variable) = registry.get(name) {
            Ok(variable.history.clone())
        } else {
            Err(crate::error::Error::user_error(
                format!("Variable '{}' not found", name),
                format!("变量 '{}' 未找到", name),
            ))
        }
    }

    /// Unregister a variable
    pub fn unregister_variable(&self, name: &str) -> RuntimeResult<()> {
        let mut registry = self.variable_registry.lock().unwrap();

        if registry.remove(name).is_some() {
            self.debug
                .debug(&format!("Unregistered variable: {}", name))?;
            Ok(())
        } else {
            Err(crate::error::Error::user_error(
                format!("Variable '{}' not found for unregistration", name),
                format!("变量 '{}' 未找到用于注销", name),
            ))
        }
    }

    /// Create variable information from value
    fn create_variable_info(
        &self,
        name: &str,
        value: &dyn VariableValue,
    ) -> RuntimeResult<VariableInfo> {
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();

        Ok(VariableInfo {
            name: name.to_string(),
            var_type: value.get_type_name().to_string(),
            value: value.to_string(),
            address: value.get_address(),
            size: value.get_size(),
            metadata: value.get_metadata(),
            scope: VariableScope::Unknown, // TODO: Implement scope tracking
            history: Vec::new(),
            created_at: timestamp,
            modified_at: timestamp,
        })
    }

    /// Create inspection result from variable info
    fn create_inspection_result(
        &self,
        variable: &VariableInfo,
        depth: usize,
    ) -> RuntimeResult<InspectionResult> {
        if depth > self.config.max_depth {
            return Err(crate::error::Error::user_error(
                "Maximum inspection depth exceeded",
                "超过最大检查深度",
            ));
        }

        let display = self.format_variable_display(variable, depth)?;
        let nested = self.inspect_nested_variables(variable, depth + 1)?;

        Ok(InspectionResult {
            variable: variable.clone(),
            display,
            nested: nested.clone(),
            metadata: InspectionMetadata {
                depth,
                nested_count: nested.len(),
                inspection_time_us: 0, // TODO: Implement timing
                memory_usage_bytes: 0, // TODO: Implement memory tracking
            },
        })
    }

    /// Format variable for display
    fn format_variable_display(
        &self,
        variable: &VariableInfo,
        depth: usize,
    ) -> RuntimeResult<String> {
        let indent = "  ".repeat(depth);
        let mut result = String::new();

        result.push_str(&indent);
        result.push_str(&variable.name);

        if self.config.include_type_info {
            result.push_str(&format!(": {}", variable.var_type));
        }

        result.push_str(&format!(
            " = {}",
            self.format_value(&variable.value, &variable.metadata)
        ));

        if self.config.include_memory_addresses {
            if let Some(addr) = variable.address {
                result.push_str(&format!(" @ 0x{:x}", addr));
            }
        }

        if let Some(size) = variable.size {
            result.push_str(&format!(" ({} bytes)", size));
        }

        Ok(result)
    }

    /// Format value according to type
    fn format_value(&self, value: &str, metadata: &VariableMetadata) -> String {
        match metadata {
            VariableMetadata::String { length, .. } => {
                if *length > self.config.max_string_length {
                    format!(
                        "\"{}...\"",
                        &value[..self.config.max_string_length.min(value.len())]
                    )
                } else {
                    format!("\"{}\"", value)
                }
            }
            VariableMetadata::Array {
                element_type,
                length,
                ..
            } => {
                if *length > self.config.max_array_elements {
                    format!("[{}; {} elements...]", element_type, length)
                } else {
                    value.to_string()
                }
            }
            _ => value.to_string(),
        }
    }

    /// Inspect nested variables for complex types
    fn inspect_nested_variables(
        &self,
        variable: &VariableInfo,
        depth: usize,
    ) -> RuntimeResult<HashMap<String, InspectionResult>> {
        let mut nested = HashMap::new();

        match &variable.metadata {
            VariableMetadata::Struct { fields, .. } => {
                // TODO: Implement field inspection
                for field in fields.iter().take(self.config.max_array_elements) {
                    // This would require access to the actual struct instance
                    // For now, we'll add placeholder entries
                    let placeholder = InspectionResult {
                        variable: VariableInfo {
                            name: field.clone(),
                            var_type: "unknown".to_string(),
                            value: "<field value>".to_string(),
                            address: None,
                            size: None,
                            metadata: VariableMetadata::Unknown,
                            scope: VariableScope::Member {
                                parent: variable.name.clone(),
                                field: field.clone(),
                            },
                            history: Vec::new(),
                            created_at: variable.modified_at,
                            modified_at: variable.modified_at,
                        },
                        display: format!("{}{} = <field value>", "  ".repeat(depth), field),
                        nested: HashMap::new(),
                        metadata: InspectionMetadata {
                            depth,
                            nested_count: 0,
                            inspection_time_us: 0,
                            memory_usage_bytes: 0,
                        },
                    };
                    nested.insert(field.clone(), placeholder);
                }
            }
            VariableMetadata::Array {
                element_type,
                length,
                ..
            } => {
                // TODO: Implement array element inspection
                for i in 0..(*length).min(self.config.max_array_elements) {
                    let element_name = format!("[{}]", i);
                    let placeholder = InspectionResult {
                        variable: VariableInfo {
                            name: element_name.clone(),
                            var_type: element_type.clone(),
                            value: "<element value>".to_string(),
                            address: None,
                            size: None,
                            metadata: VariableMetadata::Unknown,
                            scope: VariableScope::Unknown,
                            history: Vec::new(),
                            created_at: variable.modified_at,
                            modified_at: variable.modified_at,
                        },
                        display: format!(
                            "{}{} = <element value>",
                            "  ".repeat(depth),
                            element_name
                        ),
                        nested: HashMap::new(),
                        metadata: InspectionMetadata {
                            depth,
                            nested_count: 0,
                            inspection_time_us: 0,
                            memory_usage_bytes: 0,
                        },
                    };
                    nested.insert(element_name, placeholder);
                }
            }
            _ => {}
        }

        Ok(nested)
    }

    /// Clear all registered variables
    pub fn clear_all_variables(&self) -> RuntimeResult<()> {
        let mut registry = self.variable_registry.lock().unwrap();
        let count = registry.len();
        registry.clear();
        self.debug
            .info(&format!("Cleared {} registered variables", count))?;
        Ok(())
    }

    /// Get inspector statistics
    pub fn get_statistics(&self) -> RuntimeResult<InspectorStats> {
        let registry = self.variable_registry.lock().unwrap();
        let type_cache = self.type_cache.lock().unwrap();
        let memory_cache = self.memory_cache.lock().unwrap();

        Ok(InspectorStats {
            registered_variables: registry.len(),
            cached_types: type_cache.len(),
            cached_memory_layouts: memory_cache.len(),
            max_depth: self.config.max_depth,
            change_tracking_enabled: self.config.enable_change_tracking,
        })
    }

    /// Get configuration
    pub fn config(&self) -> &InspectorConfig {
        &self.config
    }

    /// Update configuration
    pub fn update_config(&mut self, config: InspectorConfig) {
        self.config = config;
    }
}

/// Statistics for variable inspector
#[derive(Debug, Clone)]
pub struct InspectorStats {
    /// Number of registered variables
    pub registered_variables: usize,
    /// Number of cached types
    pub cached_types: usize,
    /// Number of cached memory layouts
    pub cached_memory_layouts: usize,
    /// Maximum inspection depth
    pub max_depth: usize,
    /// Change tracking enabled
    pub change_tracking_enabled: bool,
}

/// Trait for values that can be inspected
pub trait VariableValue: fmt::Debug {
    /// Get type name
    fn get_type_name(&self) -> &str;

    /// Get string representation
    fn to_string(&self) -> String;

    /// Get memory address (if applicable)
    fn get_address(&self) -> Option<usize>;

    /// Get size in bytes (if known)
    fn get_size(&self) -> Option<usize>;

    /// Get variable metadata
    fn get_metadata(&self) -> VariableMetadata;
}

/// Type alias for runtime results
pub type RuntimeResult<T> = Result<T, crate::error::Error>;

// Implement VariableValue for common types
impl VariableValue for i32 {
    fn get_type_name(&self) -> &str {
        "i32"
    }
    fn to_string(&self) -> String {
        format!("{}", self)
    }
    fn get_address(&self) -> Option<usize> {
        Some(self as *const i32 as usize)
    }
    fn get_size(&self) -> Option<usize> {
        Some(4)
    }
    fn get_metadata(&self) -> VariableMetadata {
        VariableMetadata::Integer {
            signed: true,
            bits: 32,
            min: Some(i32::MIN as i64),
            max: Some(i32::MAX as u64),
        }
    }
}

impl VariableValue for String {
    fn get_type_name(&self) -> &str {
        "String"
    }
    fn to_string(&self) -> String {
        self.clone()
    }
    fn get_address(&self) -> Option<usize> {
        Some(self.as_ptr() as usize)
    }
    fn get_size(&self) -> Option<usize> {
        Some(self.len())
    }
    fn get_metadata(&self) -> VariableMetadata {
        VariableMetadata::String {
            length: self.len(),
            encoding: "UTF-8".to_string(),
            is_utf8: true, // String in Rust is always valid UTF-8
        }
    }
}

impl VariableValue for Vec<String> {
    fn get_type_name(&self) -> &str {
        "Vec<String>"
    }
    fn to_string(&self) -> String {
        format!("{:?}", self)
    }
    fn get_address(&self) -> Option<usize> {
        Some(self.as_ptr() as usize)
    }
    fn get_size(&self) -> Option<usize> {
        Some(std::mem::size_of::<Self>())
    }
    fn get_metadata(&self) -> VariableMetadata {
        VariableMetadata::Array {
            element_type: "String".to_string(),
            length: self.len(),
            capacity: self.capacity(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn test_variable_inspector_creation() {
        let debug = Arc::new(DebugModule::new());
        let inspector = VariableInspector::new(debug);

        let stats = inspector.get_statistics().unwrap();
        assert_eq!(stats.registered_variables, 0);
        assert_eq!(stats.max_depth, 5);
    }

    #[test]
    fn test_variable_registration() {
        let debug = Arc::new(DebugModule::new());
        let inspector = VariableInspector::new(debug);

        let value = 42i32;
        inspector.register_variable("test_var", &value).unwrap();

        let variables = inspector.list_variables().unwrap();
        assert_eq!(variables.len(), 1);
        assert_eq!(variables[0], "test_var");
    }

    #[test]
    fn test_variable_inspection() {
        let debug = Arc::new(DebugModule::new());
        let inspector = VariableInspector::new(debug);

        let value = 42i32;
        inspector.register_variable("test_var", &value).unwrap();

        let result = inspector.inspect_variable("test_var").unwrap();
        assert_eq!(result.variable.name, "test_var");
        assert_eq!(result.variable.var_type, "i32");
        assert!(result.display.contains("test_var"));
        assert!(result.display.contains("42"));
    }

    #[test]
    fn test_direct_value_inspection() {
        let debug = Arc::new(DebugModule::new());
        let inspector = VariableInspector::new(debug);

        let value = "Hello, World!".to_string();
        let result = inspector.inspect_value("test_string", &value).unwrap();

        assert_eq!(result.variable.name, "test_string");
        assert_eq!(result.variable.var_type, "String");
        assert!(result.display.contains("Hello, World"));
    }

    #[test]
    fn test_variable_update() {
        let debug = Arc::new(DebugModule::new());
        let mut config = InspectorConfig::default();
        config.enable_change_tracking = true;
        let inspector = VariableInspector::with_config(debug, config);

        let value = 42i32;
        inspector.register_variable("test_var", &value).unwrap();

        let new_value = 100i32;
        inspector.update_variable("test_var", &new_value).unwrap();

        let result = inspector.inspect_variable("test_var").unwrap();
        assert!(result.display.contains("100"));

        let history = inspector.get_variable_history("test_var").unwrap();
        assert_eq!(history.len(), 1);
        assert!(history[0].value.contains("42"));
    }

    #[test]
    fn test_array_inspection() {
        let debug = Arc::new(DebugModule::new());
        let inspector = VariableInspector::new(debug);

        let value = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let result = inspector.inspect_value("test_array", &value).unwrap();

        assert_eq!(result.variable.var_type, "Vec<String>");
        assert!(matches!(
            result.variable.metadata,
            VariableMetadata::Array { .. }
        ));
    }

    #[test]
    fn test_variable_unregistration() {
        let debug = Arc::new(DebugModule::new());
        let inspector = VariableInspector::new(debug);

        let value = 42i32;
        inspector.register_variable("test_var", &value).unwrap();
        assert_eq!(inspector.list_variables().unwrap().len(), 1);

        inspector.unregister_variable("test_var").unwrap();
        assert_eq!(inspector.list_variables().unwrap().len(), 0);
    }

    #[test]
    fn test_error_handling() {
        let debug = Arc::new(DebugModule::new());
        let inspector = VariableInspector::new(debug);

        // Try to inspect non-existent variable
        let result = inspector.inspect_variable("nonexistent");
        assert!(result.is_err());

        // Try to update non-existent variable
        let value = 42i32;
        let result = inspector.update_variable("nonexistent", &value);
        assert!(result.is_err());
    }
}
