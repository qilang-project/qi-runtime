//! 压缩解压模块 FFI
//!
//! 提供 gzip, deflate 压缩解压功能

use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use std::ffi::{CStr, CString};
use std::fs::File;
use std::io::{Read, Write};
use std::os::raw::c_char;

/// 压缩文件（gzip）
#[no_mangle]
pub extern "C" fn qi_compress_gzip_file(source: *const c_char, dest: *const c_char) -> i32 {
    if source.is_null() || dest.is_null() {
        return -1;
    }

    unsafe {
        let source_str = CStr::from_ptr(source).to_string_lossy();
        let dest_str = CStr::from_ptr(dest).to_string_lossy();

        // 读取源文件
        let mut input = match File::open(source_str.as_ref()) {
            Ok(f) => f,
            Err(_) => return -1,
        };

        let mut buffer = Vec::new();
        if input.read_to_end(&mut buffer).is_err() {
            return -1;
        }

        // 创建压缩文件
        let output = match File::create(dest_str.as_ref()) {
            Ok(f) => f,
            Err(_) => return -1,
        };

        let mut encoder = GzEncoder::new(output, Compression::default());

        match encoder.write_all(&buffer) {
            Ok(_) => match encoder.finish() {
                Ok(_) => 0,
                Err(_) => -1,
            },
            Err(_) => -1,
        }
    }
}

/// 解压文件（gzip）
#[no_mangle]
pub extern "C" fn qi_compress_gunzip_file(source: *const c_char, dest: *const c_char) -> i32 {
    if source.is_null() || dest.is_null() {
        return -1;
    }

    unsafe {
        let source_str = CStr::from_ptr(source).to_string_lossy();
        let dest_str = CStr::from_ptr(dest).to_string_lossy();

        // 打开压缩文件
        let input = match File::open(source_str.as_ref()) {
            Ok(f) => f,
            Err(_) => return -1,
        };

        let mut decoder = GzDecoder::new(input);
        let mut buffer = Vec::new();

        if decoder.read_to_end(&mut buffer).is_err() {
            return -1;
        }

        // 写入解压文件
        let mut output = match File::create(dest_str.as_ref()) {
            Ok(f) => f,
            Err(_) => return -1,
        };

        match output.write_all(&buffer) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    }
}

/// 压缩字符串（返回 base64 编码的压缩数据）
#[no_mangle]
pub extern "C" fn qi_compress_gzip_string(data: *const c_char) -> *mut c_char {
    if data.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let data_str = CStr::from_ptr(data).to_string_lossy();

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());

        if encoder.write_all(data_str.as_bytes()).is_err() {
            return std::ptr::null_mut();
        }

        match encoder.finish() {
            Ok(compressed) => {
                use base64::{engine::general_purpose, Engine as _};
                let encoded = general_purpose::STANDARD.encode(&compressed);

                match CString::new(encoded) {
                    Ok(c_str) => c_str.into_raw(),
                    Err(_) => std::ptr::null_mut(),
                }
            }
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 解压字符串（输入 base64 编码的压缩数据）
#[no_mangle]
pub extern "C" fn qi_compress_gunzip_string(data: *const c_char) -> *mut c_char {
    if data.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let data_str = CStr::from_ptr(data).to_string_lossy();

        use base64::{engine::general_purpose, Engine as _};
        let compressed = match general_purpose::STANDARD.decode(data_str.as_ref()) {
            Ok(d) => d,
            Err(_) => return std::ptr::null_mut(),
        };

        let mut decoder = GzDecoder::new(&compressed[..]);
        let mut buffer = Vec::new();

        if decoder.read_to_end(&mut buffer).is_err() {
            return std::ptr::null_mut();
        }

        match String::from_utf8(buffer) {
            Ok(s) => match CString::new(s) {
                Ok(c_str) => c_str.into_raw(),
                Err(_) => std::ptr::null_mut(),
            },
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 释放字符串
#[no_mangle]
pub extern "C" fn qi_compress_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

/// 二进制安全的 gzip 压缩：输入字节切片句柄 -> 输出新字节切片句柄。
/// 失败返回 -1。新句柄归调用方所有，需调用 字节切片::释放 释放。
#[no_mangle]
pub extern "C" fn qi_compress_gzip_bytes(handle: i64) -> i64 {
    let data = match crate::stdlib::bytes_ffi::clone_bytes(handle) {
        Some(v) => v,
        None => return -1,
    };
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    if encoder.write_all(&data).is_err() {
        return -1;
    }
    match encoder.finish() {
        Ok(compressed) => crate::stdlib::bytes_ffi::register_bytes(compressed),
        Err(_) => -1,
    }
}

/// 二进制安全的 gunzip 解压：输入字节切片句柄 -> 输出新字节切片句柄。
/// 失败返回 -1。
#[no_mangle]
pub extern "C" fn qi_compress_gunzip_bytes(handle: i64) -> i64 {
    let data = match crate::stdlib::bytes_ffi::clone_bytes(handle) {
        Some(v) => v,
        None => return -1,
    };
    let mut decoder = GzDecoder::new(&data[..]);
    let mut out = Vec::new();
    if decoder.read_to_end(&mut out).is_err() {
        return -1;
    }
    crate::stdlib::bytes_ffi::register_bytes(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::fs;

    #[test]
    fn test_string_compression() {
        let original = CString::new("Hello, World! This is a test string.").unwrap();

        let compressed = qi_compress_gzip_string(original.as_ptr());
        assert!(!compressed.is_null());

        let decompressed = qi_compress_gunzip_string(compressed);
        assert!(!decompressed.is_null());

        unsafe {
            let result = CStr::from_ptr(decompressed).to_string_lossy();
            assert_eq!(result, "Hello, World! This is a test string.");

            qi_compress_free_string(compressed);
            qi_compress_free_string(decompressed);
        }
    }

    #[test]
    fn test_file_compression() {
        let tmp = std::env::temp_dir();
        let source_buf = tmp.join("test_compress.txt");
        let compressed_buf = tmp.join("test_compress.txt.gz");
        let decompressed_buf = tmp.join("test_decompress.txt");
        let source = source_buf.to_str().unwrap();
        let compressed = compressed_buf.to_str().unwrap();
        let decompressed = decompressed_buf.to_str().unwrap();

        // 创建测试文件
        fs::write(source, "Test data for compression").unwrap();

        let src = CString::new(source).unwrap();
        let dst = CString::new(compressed).unwrap();

        // 压缩
        assert_eq!(qi_compress_gzip_file(src.as_ptr(), dst.as_ptr()), 0);

        // 解压
        let dec = CString::new(decompressed).unwrap();
        assert_eq!(qi_compress_gunzip_file(dst.as_ptr(), dec.as_ptr()), 0);

        // 验证
        let content = fs::read_to_string(decompressed).unwrap();
        assert_eq!(content, "Test data for compression");

        // 清理
        fs::remove_file(source).ok();
        fs::remove_file(compressed).ok();
        fs::remove_file(decompressed).ok();
    }
}
