//! multipart/form-data 解析
//!
//! 输入：字节切片句柄 + boundary 字符串
//! 输出：part 列表句柄；可查询每个 part 的 name / filename / content-type / body 字节

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};

use super::bytes_ffi::{clone_bytes, register_bytes};

#[derive(Default, Clone)]
struct Part {
    name: String,
    filename: String,
    content_type: String,
    body: Vec<u8>,
}

static PARTS_POOL: OnceLock<Mutex<HashMap<i64, Vec<Part>>>> = OnceLock::new();
static COUNTER: OnceLock<Mutex<i64>> = OnceLock::new();

fn pool() -> &'static Mutex<HashMap<i64, Vec<Part>>> {
    PARTS_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn next_handle() -> i64 {
    let counter = COUNTER.get_or_init(|| Mutex::new(0));
    let mut g = counter.lock().unwrap();
    *g += 1;
    *g
}

fn cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { Some(CStr::from_ptr(p).to_string_lossy().into_owned()) }
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    for i in from..=haystack.len() - needle.len() {
        if haystack[i..i + needle.len()] == *needle {
            return Some(i);
        }
    }
    None
}

fn header_value<'a>(headers: &'a str, name_lower: &str) -> Option<&'a str> {
    for line in headers.split("\r\n") {
        if let Some(idx) = line.find(':') {
            let key = line[..idx].trim();
            if key.eq_ignore_ascii_case(name_lower) {
                return Some(line[idx + 1..].trim());
            }
        }
    }
    None
}

// 解析 Content-Disposition: form-data; name="x"; filename="y" 之类的
fn parse_disposition(value: &str) -> (String, String) {
    let mut name = String::new();
    let mut filename = String::new();
    for piece in value.split(';') {
        let p = piece.trim();
        if let Some(rest) = p.strip_prefix("name=") {
            name = strip_quotes(rest).to_string();
        } else if let Some(rest) = p.strip_prefix("filename=") {
            filename = strip_quotes(rest).to_string();
        }
    }
    (name, filename)
}

fn strip_quotes(s: &str) -> &str {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        &s[1..s.len() - 1]
    } else {
        s
    }
}

fn parse_multipart(body: &[u8], boundary: &str) -> Vec<Part> {
    let delim = format!("--{}", boundary);
    let delim_bytes = delim.as_bytes();
    let mut parts = Vec::new();

    let mut pos = match find(body, delim_bytes, 0) {
        Some(p) => p + delim_bytes.len(),
        None => return parts,
    };

    loop {
        // 跳过开始的 \r\n（或最后的 --）
        if body.len() >= pos + 2 && &body[pos..pos + 2] == b"--" {
            // closing delimiter
            break;
        }
        if body.len() >= pos + 2 && &body[pos..pos + 2] == b"\r\n" {
            pos += 2;
        }
        // 找 \r\n\r\n 分隔 headers / body
        let header_end = match find(body, b"\r\n\r\n", pos) {
            Some(p) => p,
            None => break,
        };
        let headers_str = match std::str::from_utf8(&body[pos..header_end]) {
            Ok(s) => s.to_string(),
            Err(_) => String::from_utf8_lossy(&body[pos..header_end]).into_owned(),
        };
        let body_start = header_end + 4;

        // 找下一个 boundary
        let next_delim = format!("\r\n--{}", boundary);
        let body_end = match find(body, next_delim.as_bytes(), body_start) {
            Some(p) => p,
            None => break,
        };

        let mut part = Part::default();
        if let Some(disp) = header_value(&headers_str, "content-disposition") {
            let (n, f) = parse_disposition(disp);
            part.name = n;
            part.filename = f;
        }
        if let Some(ct) = header_value(&headers_str, "content-type") {
            part.content_type = ct.to_string();
        }
        part.body = body[body_start..body_end].to_vec();
        parts.push(part);

        pos = body_end + next_delim.len();
    }

    parts
}

/// 解析 multipart/form-data；返回 parts 列表句柄；失败返回 -1
#[no_mangle]
pub extern "C" fn qi_multipart_parse(body_handle: i64, boundary: *const c_char) -> i64 {
    let body = match clone_bytes(body_handle) {
        Some(b) => b,
        None => return -1,
    };
    let boundary = match cstr(boundary) {
        Some(s) => s,
        None => return -1,
    };
    let parts = parse_multipart(&body, &boundary);
    let h = next_handle();
    pool().lock().unwrap().insert(h, parts);
    h
}

/// 从 Content-Type 头提取 boundary 值；找不到返回 *mut c_char 空字符串
#[no_mangle]
pub extern "C" fn qi_multipart_extract_boundary(content_type: *const c_char) -> *mut c_char {
    let ct = cstr(content_type).unwrap_or_default();
    let mut found = String::new();
    for piece in ct.split(';') {
        let p = piece.trim();
        if let Some(rest) = p.strip_prefix("boundary=") {
            found = strip_quotes(rest).to_string();
            break;
        }
    }
    CString::new(found)
        .unwrap_or_else(|_| CString::new("").unwrap())
        .into_raw()
}

#[no_mangle]
pub extern "C" fn qi_multipart_count(handle: i64) -> i64 {
    pool()
        .lock()
        .unwrap()
        .get(&handle)
        .map(|v| v.len() as i64)
        .unwrap_or(0)
}

fn part_field(handle: i64, idx: i64, f: impl Fn(&Part) -> String) -> *mut c_char {
    let p = pool().lock().unwrap();
    let s = match p.get(&handle) {
        Some(parts) => {
            if idx < 0 || idx as usize >= parts.len() {
                String::new()
            } else {
                f(&parts[idx as usize])
            }
        }
        None => String::new(),
    };
    CString::new(s)
        .unwrap_or_else(|_| CString::new("").unwrap())
        .into_raw()
}

#[no_mangle]
pub extern "C" fn qi_multipart_name(handle: i64, idx: i64) -> *mut c_char {
    part_field(handle, idx, |p| p.name.clone())
}

#[no_mangle]
pub extern "C" fn qi_multipart_filename(handle: i64, idx: i64) -> *mut c_char {
    part_field(handle, idx, |p| p.filename.clone())
}

#[no_mangle]
pub extern "C" fn qi_multipart_content_type(handle: i64, idx: i64) -> *mut c_char {
    part_field(handle, idx, |p| p.content_type.clone())
}

/// 取一个 part 的 body 作为新的字节切片句柄
#[no_mangle]
pub extern "C" fn qi_multipart_body(handle: i64, idx: i64) -> i64 {
    let p = pool().lock().unwrap();
    match p.get(&handle) {
        Some(parts) => {
            if idx < 0 || idx as usize >= parts.len() {
                -1
            } else {
                let body = parts[idx as usize].body.clone();
                drop(p);
                register_bytes(body)
            }
        }
        None => -1,
    }
}

#[no_mangle]
pub extern "C" fn qi_multipart_free(handle: i64) -> i64 {
    pool().lock().unwrap().remove(&handle);
    0
}
