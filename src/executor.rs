//! Program Executor
//!
//! This module provides the interface for executing compiled Qi programs
//! with the Rust runtime environment.

#![allow(static_mut_refs)]

use std::ffi::{c_char, c_int, CStr};
use std::sync::{Mutex, Once, RwLock};

use crate::{RuntimeConfig, RuntimeEnvironment};

static RUNTIME_INIT: Once = Once::new();
// 改 RwLock 让 hot path（alloc / gc_*）走 concurrent read lock 而非串行 Mutex。
// memory_manager 内部用 DashMap + 原子计数器，方法已是 &self，所以读锁就够了。
// 写锁只在 initialize / terminate 时需要。
static mut RUNTIME: Option<RwLock<RuntimeEnvironment>> = None;

/// Initialize the Qi runtime
///
/// This function must be called before executing any Qi program.
/// It's safe to call multiple times - initialization only happens once.
#[no_mangle]
pub extern "C" fn qi_runtime_initialize() -> c_int {
    let mut result = 0;

    RUNTIME_INIT.call_once(|| {
        let config = RuntimeConfig::default();
        match RuntimeEnvironment::new(config) {
            Ok(mut runtime) => {
                if let Err(e) = runtime.initialize() {
                    eprintln!("Runtime 初始化失败: {}", e);
                    result = -1;
                    return;
                }
                unsafe {
                    RUNTIME = Some(RwLock::new(runtime));
                }
            }
            Err(e) => {
                eprintln!("Runtime 创建失败: {}", e);
                result = -1;
            }
        }
    });

    result
}

/// Shutdown the Qi runtime
#[no_mangle]
pub extern "C" fn qi_runtime_shutdown() -> c_int {
    unsafe {
        if let Some(runtime_mutex) = RUNTIME.take() {
            if let Ok(mut runtime) = runtime_mutex.write() {
                match runtime.terminate() {
                    Ok(_) => 0,
                    Err(e) => {
                        eprintln!("Runtime 关闭失败: {}", e);
                        -1
                    }
                }
            } else {
                eprintln!("无法获取 runtime 锁");
                -1
            }
        } else {
            0 // Already shutdown or never initialized
        }
    }
}

/// Execute a Qi program
#[no_mangle]
pub extern "C" fn qi_runtime_execute(program_data: *const u8, data_len: usize) -> c_int {
    if program_data.is_null() {
        eprintln!("程序数据指针为空");
        return -1;
    }

    unsafe {
        let data_slice = std::slice::from_raw_parts(program_data, data_len);

        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(mut runtime) = runtime_mutex.write() {
                match runtime.execute_program(data_slice) {
                    Ok(exit_code) => exit_code,
                    Err(e) => {
                        eprintln!("程序执行失败: {}", e);
                        runtime.increment_errors();
                        -1
                    }
                }
            } else {
                eprintln!("无法获取 runtime 锁");
                -1
            }
        } else {
            eprintln!("Runtime 未初始化");
            -1
        }
    }
}

/// Print a string (UTF-8)
#[no_mangle]
pub extern "C" fn qi_runtime_print(s: *const c_char) -> c_int {
    if s.is_null() {
        return -1;
    }

    unsafe {
        if let Ok(rust_str) = CStr::from_ptr(s).to_str() {
            print!("{}", rust_str);
            // Force flush to ensure output appears immediately
            std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());

            if let Some(runtime_mutex) = RUNTIME.as_ref() {
                if let Ok(runtime) = runtime_mutex.read() {
                    runtime.increment_io_operations();
                }
            }
            0
        } else {
            eprintln!("无效的 UTF-8 字符串");
            -1
        }
    }
}

/// Print a string with newline (UTF-8)
#[no_mangle]
pub extern "C" fn qi_runtime_println(s: *const c_char) -> c_int {
    if s.is_null() {
        return -1;
    }

    unsafe {
        if let Ok(rust_str) = CStr::from_ptr(s).to_str() {
            println!("{}", rust_str);
            // Ensure output is flushed (println! should flush, but let's be explicit)
            std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());

            if let Some(runtime_mutex) = RUNTIME.as_ref() {
                if let Ok(runtime) = runtime_mutex.read() {
                    runtime.increment_io_operations();
                }
            }
            0
        } else {
            eprintln!("无效的 UTF-8 字符串");
            -1
        }
    }
}

/// Print an integer
#[no_mangle]
pub extern "C" fn qi_runtime_print_int(value: i64) -> c_int {
    print!("{}", value);
    // Force flush to ensure output appears immediately
    std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());

    0
}

/// Print an integer with newline
#[no_mangle]
pub extern "C" fn qi_runtime_println_int(value: i64) -> c_int {
    println!("{}", value);

    0
}

/// Print a float
#[no_mangle]
pub extern "C" fn qi_runtime_print_float(value: f64) -> c_int {
    print!("{}", value);
    // Force flush to ensure output appears immediately
    std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());

    0
}

/// Print a float with newline
#[no_mangle]
pub extern "C" fn qi_runtime_println_float(value: f64) -> c_int {
    // Format to always show decimal point for float values
    if value.fract() == 0.0 {
        println!("{:.1}", value); // Show one decimal place for whole numbers
    } else {
        println!("{}", value); // Show normal format for fractions
    }

    0
}

/// Print a boolean value (accepts i32: 0 = false, non-zero = true)
#[no_mangle]
pub extern "C" fn qi_runtime_print_bool(value: i32) -> c_int {
    let text = if value != 0 { "真" } else { "假" };
    print!("{}", text);
    // Force flush to ensure output appears immediately
    std::io::Write::flush(&mut std::io::stdout()).unwrap_or(());

    0
}

/// Print a boolean value with newline (accepts i32: 0 = false, non-zero = true)
#[no_mangle]
pub extern "C" fn qi_runtime_println_bool(value: i32) -> c_int {
    let text = if value != 0 { "真" } else { "假" };
    println!("{}", text);

    0
}

/// Allocate memory
#[no_mangle]
pub extern "C" fn qi_runtime_alloc(size: usize) -> *mut u8 {
    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                match runtime.memory_manager.allocate(size, None) {
                    Ok(ptr) => ptr,
                    Err(e) => {
                        eprintln!("内存分配失败: {}", e);
                        std::ptr::null_mut()
                    }
                }
            } else {
                std::ptr::null_mut()
            }
        } else {
            // Fallback to standard allocation if runtime not initialized
            let layout = std::alloc::Layout::from_size_align(size, 8).unwrap();
            std::alloc::alloc(layout)
        }
    }
}

/// Deallocate memory
#[no_mangle]
pub extern "C" fn qi_runtime_dealloc(ptr: *mut u8, size: usize) -> c_int {
    if ptr.is_null() {
        return -1;
    }

    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                match runtime.memory_manager.deallocate(ptr) {
                    Ok(_) => 0,
                    Err(e) => {
                        eprintln!("内存释放失败: {}", e);
                        -1
                    }
                }
            } else {
                -1
            }
        } else {
            // Fallback to standard deallocation
            let layout = std::alloc::Layout::from_size_align(size, 8).unwrap();
            std::alloc::dealloc(ptr, layout);
            0
        }
    }
}

/// Check if garbage collection should be triggered
/// Returns 1 if GC should run, 0 otherwise
#[no_mangle]
pub extern "C" fn qi_runtime_gc_should_collect() -> i64 {
    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                if runtime.memory_manager.should_collect() {
                    return 1;
                }
            }
        }
    }
    0
}

/// Trigger garbage collection
#[no_mangle]
pub extern "C" fn qi_runtime_gc_collect() {
    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                if let Err(e) = runtime.memory_manager.collect() {
                    eprintln!("GC失败: {}", e);
                } else {
                }
            }
        }
    }
}

/// Register a heap object as a GC root.
#[no_mangle]
pub extern "C" fn qi_runtime_gc_add_root(ptr: *mut u8) -> i64 {
    if ptr.is_null() {
        return -1;
    }

    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                if runtime.memory_manager.add_root(ptr).is_ok() {
                    return 1;
                }
            }
        }
    }
    -1
}

/// Remove a heap object from the GC root set.
#[no_mangle]
pub extern "C" fn qi_runtime_gc_remove_root(ptr: *mut u8) -> i64 {
    if ptr.is_null() {
        return -1;
    }

    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                if runtime.memory_manager.remove_root(ptr).is_ok() {
                    return 1;
                }
            }
        }
    }
    -1
}

/// Add a reference edge between two heap objects.
#[no_mangle]
pub extern "C" fn qi_runtime_gc_add_reference(from: *mut u8, to: *mut u8) -> i64 {
    if from.is_null() || to.is_null() {
        return -1;
    }

    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                if runtime.memory_manager.add_reference(from, to).is_ok() {
                    return 1;
                }
            }
        }
    }
    -1
}

/// Clear all outgoing references for a heap object.
#[no_mangle]
pub extern "C" fn qi_runtime_gc_clear_references(ptr: *mut u8) -> i64 {
    if ptr.is_null() {
        return -1;
    }

    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                if runtime.memory_manager.clear_references(ptr).is_ok() {
                    return 1;
                }
            }
        }
    }
    -1
}

/// Get runtime metrics as JSON string
#[no_mangle]
pub extern "C" fn qi_runtime_get_metrics() -> *const c_char {
    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                let snap = runtime.get_metrics().snapshot();
                if let Ok(json) = serde_json::to_string(&snap) {
                    let c_string = std::ffi::CString::new(json).unwrap();
                    return c_string.into_raw();
                }
            }
        }
        std::ptr::null()
    }
}

/// Free a string allocated by the runtime
#[no_mangle]
pub extern "C" fn qi_runtime_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = std::ffi::CString::from_raw(s);
        }
    }
}

// ============================================================================
// String Operations
// ============================================================================

/// Get string length (returns number of UTF-8 characters)
#[no_mangle]
pub extern "C" fn qi_runtime_string_length(s: *const c_char) -> i64 {
    if s.is_null() {
        return 0;
    }
    unsafe {
        if let Ok(rust_str) = CStr::from_ptr(s).to_str() {
            rust_str.chars().count() as i64
        } else {
            0
        }
    }
}

/// Concatenate two strings (caller must free the result)
#[no_mangle]
pub extern "C" fn qi_runtime_string_concat(s1: *const c_char, s2: *const c_char) -> *mut c_char {
    if s1.is_null() || s2.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        if let (Ok(str1), Ok(str2)) = (CStr::from_ptr(s1).to_str(), CStr::from_ptr(s2).to_str()) {
            let result = format!("{}{}", str1, str2);
            if let Ok(c_string) = std::ffi::CString::new(result) {
                return c_string.into_raw();
            }
        }
        std::ptr::null_mut()
    }
}

/// Get substring (caller must free the result)
#[no_mangle]
pub extern "C" fn qi_runtime_string_slice(s: *const c_char, start: i64, end: i64) -> *mut c_char {
    if s.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        if let Ok(rust_str) = CStr::from_ptr(s).to_str() {
            let chars: Vec<char> = rust_str.chars().collect();
            let start_idx = start.max(0) as usize;
            let end_idx = end.min(chars.len() as i64) as usize;

            if start_idx < end_idx && end_idx <= chars.len() {
                let substring: String = chars[start_idx..end_idx].iter().collect();
                if let Ok(c_string) = std::ffi::CString::new(substring) {
                    return c_string.into_raw();
                }
            }
        }
        std::ptr::null_mut()
    }
}

/// Compare two strings (returns 0 if equal, <0 if s1<s2, >0 if s1>s2)
#[no_mangle]
pub extern "C" fn qi_runtime_string_compare(s1: *const c_char, s2: *const c_char) -> c_int {
    if s1.is_null() || s2.is_null() {
        return -1;
    }

    unsafe {
        if let (Ok(str1), Ok(str2)) = (CStr::from_ptr(s1).to_str(), CStr::from_ptr(s2).to_str()) {
            str1.cmp(str2) as c_int
        } else {
            -1
        }
    }
}

// ============================================================================
// Math Operations
// ============================================================================

/// Square root
#[no_mangle]
pub extern "C" fn qi_runtime_math_sqrt(x: f64) -> f64 {
    x.sqrt()
}

/// Power function
#[no_mangle]
pub extern "C" fn qi_runtime_math_pow(base: f64, exp: f64) -> f64 {
    base.powf(exp)
}

/// Sine
#[no_mangle]
pub extern "C" fn qi_runtime_math_sin(x: f64) -> f64 {
    x.sin()
}

/// Cosine
#[no_mangle]
pub extern "C" fn qi_runtime_math_cos(x: f64) -> f64 {
    x.cos()
}

/// Tangent
#[no_mangle]
pub extern "C" fn qi_runtime_math_tan(x: f64) -> f64 {
    x.tan()
}

/// Absolute value (integer)
#[no_mangle]
pub extern "C" fn qi_runtime_math_abs_int(x: i64) -> i64 {
    x.abs()
}

/// Absolute value (float)
#[no_mangle]
pub extern "C" fn qi_runtime_math_abs_float(x: f64) -> f64 {
    x.abs()
}

/// Floor
#[no_mangle]
pub extern "C" fn qi_runtime_math_floor(x: f64) -> f64 {
    x.floor()
}

/// Ceiling
#[no_mangle]
pub extern "C" fn qi_runtime_math_ceil(x: f64) -> f64 {
    x.ceil()
}

/// Round
#[no_mangle]
pub extern "C" fn qi_runtime_math_round(x: f64) -> f64 {
    x.round()
}

// ============================================================================
// File I/O Operations
// ============================================================================

/// Open a file (returns file handle or negative on error)
/// Simplified implementation using path hash as handle
#[no_mangle]
pub extern "C" fn qi_runtime_file_open(path: *const c_char, mode: *const c_char) -> i64 {
    if path.is_null() || mode.is_null() {
        return -1;
    }

    unsafe {
        if let (Ok(path_str), Ok(_mode_str)) =
            (CStr::from_ptr(path).to_str(), CStr::from_ptr(mode).to_str())
        {
            // Use a hash of the path as a temporary handle
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            let mut hasher = DefaultHasher::new();
            path_str.hash(&mut hasher);
            let handle = hasher.finish() as i64;

            // Update I/O operation count
            if let Some(runtime_mutex) = RUNTIME.as_ref() {
                if let Ok(runtime) = runtime_mutex.read() {
                    runtime.increment_io_operations();
                }
            }

            // Return a positive handle on success
            if handle <= 0 {
                1
            } else {
                handle
            }
        } else {
            eprintln!("文件打开失败: 无效的UTF-8字符串");
            -1
        }
    }
}

/// Read from file (returns bytes read or negative on error)
/// Simplified implementation that simulates reading data
#[no_mangle]
pub extern "C" fn qi_runtime_file_read(handle: i64, buffer: *mut u8, size: usize) -> i64 {
    if handle <= 0 || buffer.is_null() || size == 0 {
        return -1;
    }

    // For this simplified implementation, we simulate reading sample data
    let sample_data = b"sample file content from Qi runtime";
    let bytes_to_copy = std::cmp::min(size, sample_data.len());

    unsafe {
        std::ptr::copy_nonoverlapping(sample_data.as_ptr(), buffer, bytes_to_copy);
    }

    // Update I/O operation count
    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                runtime.increment_io_operations();
            }
        }
    }

    bytes_to_copy as i64
}

/// Write to file (returns bytes written or negative on error)
/// Simplified implementation that simulates writing data
#[no_mangle]
pub extern "C" fn qi_runtime_file_write(handle: i64, data: *const u8, size: usize) -> i64 {
    if handle <= 0 || data.is_null() || size == 0 {
        return -1;
    }

    // For this simplified implementation, we simulate successful writes
    // In a full implementation, we would look up the file handle and write to it

    // Update I/O operation count
    unsafe {
        if let Some(runtime_mutex) = RUNTIME.as_ref() {
            if let Ok(runtime) = runtime_mutex.read() {
                runtime.increment_io_operations();
            }
        }
    }

    size as i64
}

/// Close file
/// Simplified implementation that simulates closing
#[no_mangle]
pub extern "C" fn qi_runtime_file_close(handle: i64) -> c_int {
    if handle <= 0 {
        return -1;
    }

    // For this simplified implementation, we simulate successful closure
    // In a full implementation, we would look up and close the file handle

    0
}

/// Read entire file as string (caller must free the result)
#[no_mangle]
pub extern "C" fn qi_runtime_file_read_string(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        if let Ok(path_str) = CStr::from_ptr(path).to_str() {
            // Use standard library to read file
            match std::fs::read_to_string(path_str) {
                Ok(content) => {
                    if let Ok(c_string) = std::ffi::CString::new(content) {
                        return c_string.into_raw();
                    }
                }
                Err(e) => {
                    eprintln!("读取文件内容失败: {}", e);
                }
            }
        }
        std::ptr::null_mut()
    }
}

/// Write string to file
#[no_mangle]
pub extern "C" fn qi_runtime_file_write_string(
    path: *const c_char,
    content: *const c_char,
) -> c_int {
    if path.is_null() || content.is_null() {
        return -1;
    }

    unsafe {
        if let (Ok(path_str), Ok(content_str)) = (
            CStr::from_ptr(path).to_str(),
            CStr::from_ptr(content).to_str(),
        ) {
            // Use standard library to write file
            match std::fs::write(path_str, content_str) {
                Ok(_) => 0,
                Err(e) => {
                    eprintln!("写入文件内容失败: {}", e);
                    -1
                }
            }
        } else {
            -1
        }
    }
}

// ============================================================================
// Network Operations
// ============================================================================

/// Make HTTP GET request (caller must free the result)
#[no_mangle]
pub extern "C" fn qi_runtime_http_get(url: *const c_char) -> *mut c_char {
    if url.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        if let Ok(url_str) = CStr::from_ptr(url).to_str() {
            // For this simplified implementation, we simulate a successful HTTP response
            // In a full implementation, we would use the network interface
            let mock_response =
                r#"{"message": "Mock HTTP response from Qi runtime", "status": "success"}"#;

            if let Ok(c_string) = std::ffi::CString::new(mock_response) {
                // Update I/O operation count
                if let Some(runtime_mutex) = RUNTIME.as_ref() {
                    if let Ok(runtime) = runtime_mutex.read() {
                        runtime.increment_io_operations();
                    }
                }

                return c_string.into_raw();
            }
        } else {
            eprintln!("HTTP请求失败: 无效的UTF-8 URL字符串");
        }
    }

    std::ptr::null_mut()
}

/// Make HTTP POST request (caller must free the result)
#[no_mangle]
pub extern "C" fn qi_runtime_http_post(url: *const c_char, data: *const c_char) -> *mut c_char {
    if url.is_null() || data.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        if let (Ok(url_str), Ok(data_str)) =
            (CStr::from_ptr(url).to_str(), CStr::from_ptr(data).to_str())
        {
            // For this simplified implementation, we simulate a successful HTTP response
            let mock_response = format!(
                r#"{{"message": "Mock POST response", "received_data": "{}", "status": "success"}}"#,
                data_str
            );

            if let Ok(c_string) = std::ffi::CString::new(mock_response) {
                // Update I/O operation count
                if let Some(runtime_mutex) = RUNTIME.as_ref() {
                    if let Ok(runtime) = runtime_mutex.read() {
                        runtime.increment_io_operations();
                    }
                }

                return c_string.into_raw();
            }
        } else {
            eprintln!("HTTP POST请求失败: 无效的UTF-8字符串");
        }
    }

    std::ptr::null_mut()
}

/// Open TCP connection (returns connection handle or negative on error)
#[no_mangle]
pub extern "C" fn qi_runtime_tcp_connect(host: *const c_char, port: i32) -> i64 {
    if host.is_null() || port <= 0 {
        return -1;
    }

    unsafe {
        if let Ok(host_str) = CStr::from_ptr(host).to_str() {
            // Use a hash of the host:port as a temporary connection handle
            use std::collections::hash_map::DefaultHasher;
            use std::hash::{Hash, Hasher};

            let connection_str = format!("{}:{}", host_str, port);
            let mut hasher = DefaultHasher::new();
            connection_str.hash(&mut hasher);
            let handle = hasher.finish() as i64;

            // Update I/O operation count
            if let Some(runtime_mutex) = RUNTIME.as_ref() {
                if let Ok(runtime) = runtime_mutex.read() {
                    runtime.increment_io_operations();
                }
            }

            // Return a positive handle on success
            if handle <= 0 {
                1
            } else {
                handle
            }
        } else {
            eprintln!("TCP连接失败: 无效的UTF-8主机字符串");
            -1
        }
    }
}

/// Close TCP connection
#[no_mangle]
pub extern "C" fn qi_runtime_tcp_close(handle: i64) -> c_int {
    if handle <= 0 {
        return -1;
    }

    // For this simplified implementation, we simulate successful closure
    0
}

// ============================================================================
// Array Operations
// ============================================================================

/// Create array (returns pointer to array structure)
#[no_mangle]
pub extern "C" fn qi_runtime_array_create(size: i64, element_size: i64) -> *mut u8 {
    if size <= 0 || element_size <= 0 {
        return std::ptr::null_mut();
    }

    let total_size = (size * element_size) as usize;
    qi_runtime_alloc(total_size)
}

/// Get array length
#[no_mangle]
pub extern "C" fn qi_runtime_array_length(array: *const u8) -> i64 {
    // For now, we'll store the length in the first 8 bytes
    // This is a simplified implementation
    if array.is_null() {
        return 0;
    }

    unsafe {
        let length_ptr = array as *const i64;
        *length_ptr
    }
}

// ============================================================================
// Type Conversion
// ============================================================================

/// Convert integer to string (caller must free the result)
#[no_mangle]
pub extern "C" fn qi_runtime_int_to_string(value: i64) -> *mut c_char {
    let string = value.to_string();
    if let Ok(c_string) = std::ffi::CString::new(string) {
        c_string.into_raw()
    } else {
        std::ptr::null_mut()
    }
}

/// Convert float to string (caller must free the result)
#[no_mangle]
pub extern "C" fn qi_runtime_float_to_string(value: f64) -> *mut c_char {
    let string = value.to_string();
    if let Ok(c_string) = std::ffi::CString::new(string) {
        c_string.into_raw()
    } else {
        std::ptr::null_mut()
    }
}

/// Convert string to integer
#[no_mangle]
pub extern "C" fn qi_runtime_string_to_int(s: *const c_char) -> i64 {
    if s.is_null() {
        return 0;
    }

    unsafe {
        if let Ok(rust_str) = CStr::from_ptr(s).to_str() {
            rust_str.parse::<i64>().unwrap_or(0)
        } else {
            0
        }
    }
}

/// Convert string to float
#[no_mangle]
pub extern "C" fn qi_runtime_string_to_float(s: *const c_char) -> f64 {
    if s.is_null() {
        return 0.0;
    }

    unsafe {
        if let Ok(rust_str) = CStr::from_ptr(s).to_str() {
            rust_str.parse::<f64>().unwrap_or(0.0)
        } else {
            0.0
        }
    }
}

/// Convert integer to float
#[no_mangle]
pub extern "C" fn qi_runtime_int_to_float(value: i64) -> f64 {
    value as f64
}

/// Convert float to integer (truncate)
#[no_mangle]
pub extern "C" fn qi_runtime_float_to_int(value: f64) -> i64 {
    value as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_runtime_initialization() {
        let result = qi_runtime_initialize();
        assert_eq!(result, 0);

        let shutdown_result = qi_runtime_shutdown();
        assert_eq!(shutdown_result, 0);
    }

    #[test]
    fn test_runtime_print_functions() {
        qi_runtime_initialize();

        // These should not panic
        qi_runtime_print_int(42);
        qi_runtime_println_int(42);
        qi_runtime_print_float(3.14);
        qi_runtime_println_float(3.14);

        qi_runtime_shutdown();
    }

    #[test]
    fn test_string_operations() {
        use std::ffi::CString;

        let s1 = CString::new("Hello").unwrap();
        let s2 = CString::new("World").unwrap();

        let len = qi_runtime_string_length(s1.as_ptr());
        assert_eq!(len, 5);

        let result = qi_runtime_string_concat(s1.as_ptr(), s2.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_str().unwrap();
            assert_eq!(result_str, "HelloWorld");
            qi_runtime_free_string(result);
        }
    }

    #[test]
    fn test_math_operations() {
        let result = qi_runtime_math_sqrt(16.0);
        assert_eq!(result, 4.0);

        let result = qi_runtime_math_pow(2.0, 3.0);
        assert_eq!(result, 8.0);

        let result = qi_runtime_math_abs_int(-42);
        assert_eq!(result, 42);
    }
}

// ============================================================================
// Synchronization Primitives - WaitGroup & Mutex
// ============================================================================

use std::sync::{Arc, Condvar};

#[repr(C)]
pub struct QiWaitGroup {
    counter: Arc<Mutex<i32>>,
    condvar: Arc<Condvar>,
}

#[repr(C)]
pub struct QiMutex {
    inner: Arc<Mutex<()>>,
}

/// Create a new WaitGroup
#[no_mangle]
pub extern "C" fn qi_runtime_waitgroup_create() -> *mut QiWaitGroup {
    let wg = Box::new(QiWaitGroup {
        counter: Arc::new(Mutex::new(0)),
        condvar: Arc::new(Condvar::new()),
    });
    Box::into_raw(wg)
}

/// Add counter to WaitGroup
#[no_mangle]
pub extern "C" fn qi_runtime_waitgroup_add(wg: *mut QiWaitGroup, delta: i32) -> i32 {
    if wg.is_null() {
        return -1;
    }

    let wg = unsafe { &mut *wg };
    let mut counter = wg.counter.lock().unwrap();
    *counter += delta;
    0
}

/// Wait for WaitGroup counter to become zero
#[no_mangle]
pub extern "C" fn qi_runtime_waitgroup_wait(wg: *mut QiWaitGroup) -> i32 {
    if wg.is_null() {
        return -1;
    }

    let wg = unsafe { &mut *wg };
    let mut counter = wg.counter.lock().unwrap();
    while *counter > 0 {
        counter = wg.condvar.wait(counter).unwrap();
    }
    0
}

/// Done signals completion of one task in WaitGroup
#[no_mangle]
pub extern "C" fn qi_runtime_waitgroup_done(wg: *mut QiWaitGroup) -> i32 {
    if wg.is_null() {
        return -1;
    }

    let wg = unsafe { &mut *wg };
    let mut counter = wg.counter.lock().unwrap();
    *counter -= 1;
    if *counter == 0 {
        wg.condvar.notify_all();
    }
    0
}

/// Destroy WaitGroup
#[no_mangle]
pub extern "C" fn qi_runtime_waitgroup_destroy(wg: *mut QiWaitGroup) -> i32 {
    if wg.is_null() {
        return -1;
    }

    unsafe {
        let _ = Box::from_raw(wg);
    }
    0
}

/// Create a new mutex
#[no_mangle]
pub extern "C" fn qi_runtime_mutex_create() -> *mut QiMutex {
    let mutex = Box::new(QiMutex {
        inner: Arc::new(Mutex::new(())),
    });
    Box::into_raw(mutex)
}

/// Lock a mutex
#[no_mangle]
pub extern "C" fn qi_runtime_mutex_lock(mutex: *mut QiMutex) -> i32 {
    if mutex.is_null() {
        return -1;
    }

    let mutex = unsafe { &mut *mutex };
    let _lock = mutex.inner.lock().unwrap();
    // Note: In real implementation, we'd need to store the lock
    0
}

/// Try to lock a mutex (non-blocking)
#[no_mangle]
pub extern "C" fn qi_runtime_mutex_trylock(mutex: *mut QiMutex) -> i32 {
    if mutex.is_null() {
        return -1;
    }

    let mutex = unsafe { &mut *mutex };
    match mutex.inner.try_lock() {
        Ok(_) => 0,
        Err(_) => 1, // Would block
    }
}

/// Unlock a mutex
#[no_mangle]
pub extern "C" fn qi_runtime_mutex_unlock(mutex: *mut QiMutex) -> i32 {
    if mutex.is_null() {
        return -1;
    }

    // Note: In real implementation, we'd need to release the stored lock
    0
}

/// Destroy a mutex
#[no_mangle]
pub extern "C" fn qi_runtime_mutex_destroy(mutex: *mut QiMutex) -> i32 {
    if mutex.is_null() {
        return -1;
    }

    unsafe {
        let _ = Box::from_raw(mutex);
    }
    0
}
