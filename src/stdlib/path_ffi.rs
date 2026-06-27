//! 路径处理模块 FFI
//!
//! 提供文件路径操作功能

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;

/// 连接路径
#[no_mangle]
pub extern "C" fn qi_path_join(path1: *const c_char, path2: *const c_char) -> *mut c_char {
    if path1.is_null() || path2.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let p1 = CStr::from_ptr(path1).to_string_lossy();
        let p2 = CStr::from_ptr(path2).to_string_lossy();

        let joined = Path::new(p1.as_ref()).join(p2.as_ref());

        match CString::new(joined.to_string_lossy().as_ref()) {
            Ok(c_str) => c_str.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 获取文件名
#[no_mangle]
pub extern "C" fn qi_path_filename(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();
        let p = Path::new(path_str.as_ref());

        if let Some(filename) = p.file_name() {
            match CString::new(filename.to_string_lossy().as_ref()) {
                Ok(c_str) => c_str.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        } else {
            match CString::new("") {
                Ok(c_str) => c_str.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        }
    }
}

/// 获取父目录
#[no_mangle]
pub extern "C" fn qi_path_parent(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();
        let p = Path::new(path_str.as_ref());

        if let Some(parent) = p.parent() {
            match CString::new(parent.to_string_lossy().as_ref()) {
                Ok(c_str) => c_str.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        } else {
            match CString::new("") {
                Ok(c_str) => c_str.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        }
    }
}

/// 获取扩展名
#[no_mangle]
pub extern "C" fn qi_path_extension(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();
        let p = Path::new(path_str.as_ref());

        if let Some(ext) = p.extension() {
            match CString::new(ext.to_string_lossy().as_ref()) {
                Ok(c_str) => c_str.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        } else {
            match CString::new("") {
                Ok(c_str) => c_str.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        }
    }
}

/// 获取绝对路径
#[no_mangle]
pub extern "C" fn qi_path_absolute(path: *const c_char) -> *mut c_char {
    if path.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();
        let p = Path::new(path_str.as_ref());

        if let Ok(abs_path) = p.canonicalize() {
            match CString::new(abs_path.to_string_lossy().as_ref()) {
                Ok(c_str) => c_str.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        } else {
            // 如果无法规范化，返回原路径
            match CString::new(path_str.as_ref()) {
                Ok(c_str) => c_str.into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        }
    }
}

/// 路径是否存在
#[no_mangle]
pub extern "C" fn qi_path_exists(path: *const c_char) -> i32 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();
        let p = Path::new(path_str.as_ref());

        if p.exists() {
            1
        } else {
            0
        }
    }
}

/// 是否是目录
#[no_mangle]
pub extern "C" fn qi_path_is_dir(path: *const c_char) -> i32 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();
        let p = Path::new(path_str.as_ref());

        if p.is_dir() {
            1
        } else {
            0
        }
    }
}

/// 是否是文件
#[no_mangle]
pub extern "C" fn qi_path_is_file(path: *const c_char) -> i32 {
    if path.is_null() {
        return 0;
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();
        let p = Path::new(path_str.as_ref());

        if p.is_file() {
            1
        } else {
            0
        }
    }
}

/// 释放字符串
#[no_mangle]
pub extern "C" fn qi_path_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_join() {
        let p1 = CString::new("/home/user").unwrap();
        let p2 = CString::new("documents/file.txt").unwrap();

        let result = qi_path_join(p1.as_ptr(), p2.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert!(result_str.contains("documents"));
            qi_path_free_string(result);
        }
    }

    #[test]
    fn test_filename() {
        let path = CString::new("/home/user/file.txt").unwrap();

        let result = qi_path_filename(path.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert_eq!(result_str, "file.txt");
            qi_path_free_string(result);
        }
    }

    #[test]
    fn test_extension() {
        let path = CString::new("/home/user/file.txt").unwrap();

        let result = qi_path_extension(path.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert_eq!(result_str, "txt");
            qi_path_free_string(result);
        }
    }
}
