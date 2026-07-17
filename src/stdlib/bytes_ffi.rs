//! 字节切片模块 FFI
//!
//! 提供二进制安全的字节缓冲区，核心原语为句柄→Vec<u8>。
//! 字符串/字节切片之间通过 UTF-8 (lossy) 互转。

#![allow(non_snake_case)]

use dashmap::DashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;

// 用 DashMap 替换 Mutex<HashMap>。读 / 写 / 插入 / 删除 都走分片锁——不同
// 句柄之间不再相互阻塞，hot path（追加切片、读取、释放）的瓶颈消失。
static POOL: OnceLock<DashMap<i64, Vec<u8>>> = OnceLock::new();
static COUNTER: AtomicI64 = AtomicI64::new(0);

fn pool() -> &'static DashMap<i64, Vec<u8>> {
    POOL.get_or_init(DashMap::new)
}

fn next_handle() -> i64 {
    COUNTER.fetch_add(1, Ordering::Relaxed) + 1
}

/// 内部 API：把已有的 Vec<u8> 注册到字节池，返回新句柄。
pub(crate) fn register_bytes(data: Vec<u8>) -> i64 {
    let h = next_handle();
    pool().insert(h, data);
    h
}

/// 内部 API：克隆字节池中指定句柄的数据
pub(crate) fn clone_bytes(handle: i64) -> Option<Vec<u8>> {
    pool().get(&handle).map(|v| v.clone())
}

/// 内部 API：用闭包借引用字节池数据，零拷贝。返回 None 表示句柄无效。
pub(crate) fn with_bytes<F, R>(handle: i64, f: F) -> Option<R>
where
    F: FnOnce(&[u8]) -> R,
{
    pool().get(&handle).map(|v| f(v.as_slice()))
}

/// 内部 API：取出（移除）字节池中指定句柄的数据，避免 clone+remove 两次原子操作
pub(crate) fn take_bytes(handle: i64) -> Option<Vec<u8>> {
    pool().remove(&handle).map(|(_, v)| v)
}

/// 内部 API：释放句柄
pub(crate) fn free_bytes(handle: i64) {
    if handle < 0 {
        return; // 持久句柄：释放是 no-op（进程生命周期常驻）
    }
    pool().remove(&handle);
}

// ── 持久字节（负数句柄）────────────────────────────────────────────
// 进程生命周期常驻、只读共享的字节块（预构建的缓存 HTTP 响应等）。写出方
// 克隆 Arc（一次原子引用计数）后在池锁外写 socket——每请求零分配零拷贝；
// qi 侧照常调 释放切片 也无副作用（no-op）。与普通句柄用符号区分：恒为负。

static PERSISTENT: OnceLock<DashMap<i64, std::sync::Arc<Vec<u8>>>> = OnceLock::new();

fn persistent_pool() -> &'static DashMap<i64, std::sync::Arc<Vec<u8>>> {
    PERSISTENT.get_or_init(DashMap::new)
}

/// 注册持久字节，返回负数句柄。
pub(crate) fn register_persistent_bytes(data: Vec<u8>) -> i64 {
    let h = -next_handle();
    persistent_pool().insert(h, std::sync::Arc::new(data));
    h
}

/// 取持久字节的 Arc 克隆（廉价，一次原子操作）。
pub(crate) fn persistent_arc(handle: i64) -> Option<std::sync::Arc<Vec<u8>>> {
    persistent_pool().get(&handle).map(|v| v.clone())
}

fn cstr_bytes(p: *const c_char) -> Option<&'static [u8]> {
    if p.is_null() {
        return None;
    }
    unsafe { Some(CStr::from_ptr(p).to_bytes()) }
}

/// 创建一个空的字节切片
#[no_mangle]
pub extern "C" fn qi_bytes_create() -> i64 {
    let h = next_handle();
    pool().insert(h, Vec::new());
    h
}

/// 用初始容量创建字节切片
#[no_mangle]
pub extern "C" fn qi_bytes_with_capacity(cap: i64) -> i64 {
    let h = next_handle();
    let cap = if cap < 0 { 0 } else { cap as usize };
    pool().insert(h, Vec::with_capacity(cap));
    h
}

/// 从字符串拷贝出字节切片（按 UTF-8 编码）
#[no_mangle]
pub extern "C" fn qi_bytes_from_string(s: *const c_char) -> i64 {
    let bytes = match cstr_bytes(s) {
        Some(b) => b.to_vec(),
        None => Vec::new(),
    };
    let h = next_handle();
    pool().insert(h, bytes);
    h
}

/// 把字节切片当 UTF-8 解码成字符串（非 UTF-8 按 lossy 替换；
/// NUL 字节用空格替换，因为 C 字符串不能含 NUL）
#[no_mangle]
pub extern "C" fn qi_bytes_to_string(handle: i64) -> *mut c_char {
    let s = match pool().get(&handle) {
        Some(v) => {
            let lossy = String::from_utf8_lossy(&v).into_owned();
            lossy.replace('\0', " ")
        }
        None => String::new(),
    };
    crate::stdlib::qi_str::rc_cstr_from_string(s)
}

/// 字节长度
#[no_mangle]
pub extern "C" fn qi_bytes_length(handle: i64) -> i64 {
    if handle < 0 {
        return persistent_arc(handle).map(|v| v.len() as i64).unwrap_or(0);
    }
    pool().get(&handle).map(|v| v.len() as i64).unwrap_or(0)
}

/// 取第 i 个字节，返回 0..255 的整数；越界返回 -1
#[no_mangle]
pub extern "C" fn qi_bytes_get(handle: i64, index: i64) -> i64 {
    if index < 0 {
        return -1;
    }
    pool()
        .get(&handle)
        .and_then(|v| v.get(index as usize).map(|b| *b as i64))
        .unwrap_or(-1)
}

/// 设置第 i 个字节
#[no_mangle]
pub extern "C" fn qi_bytes_set(handle: i64, index: i64, value: i64) -> i64 {
    if index < 0 {
        return -1;
    }
    if let Some(mut entry) = pool().get_mut(&handle) {
        if let Some(slot) = entry.get_mut(index as usize) {
            *slot = (value & 0xFF) as u8;
            return 0;
        }
    }
    -1
}

/// 追加单个字节
#[no_mangle]
pub extern "C" fn qi_bytes_push(handle: i64, value: i64) -> i64 {
    if let Some(mut entry) = pool().get_mut(&handle) {
        entry.push((value & 0xFF) as u8);
        return 0;
    }
    -1
}

/// 把字符串字节追加到字节切片末尾
#[no_mangle]
pub extern "C" fn qi_bytes_push_string(handle: i64, s: *const c_char) -> i64 {
    let bytes = match cstr_bytes(s) {
        Some(b) => b.to_vec(),
        None => return -1,
    };
    if let Some(mut entry) = pool().get_mut(&handle) {
        entry.extend_from_slice(&bytes);
        return 0;
    }
    -1
}

/// 把另一个字节切片追加到末尾
#[no_mangle]
pub extern "C" fn qi_bytes_extend(handle: i64, other: i64) -> i64 {
    // 注意：必须 *先* 拿到 other 的拷贝，再去 get_mut(handle)。否则 dashmap
    // 同一个 shard 里两个键同时取读+写锁会死锁。
    let other_bytes = match pool().get(&other) {
        Some(v) => v.clone(),
        None => return -1,
    };
    if let Some(mut entry) = pool().get_mut(&handle) {
        entry.extend_from_slice(&other_bytes);
        return 0;
    }
    -1
}

/// 取一段切片成新句柄
#[no_mangle]
pub extern "C" fn qi_bytes_slice(handle: i64, start: i64, len: i64) -> i64 {
    if start < 0 || len < 0 {
        return -1;
    }
    let new_vec = {
        let v = match pool().get(&handle) {
            Some(v) => v,
            None => return -1,
        };
        let s = start as usize;
        let l = len as usize;
        if s > v.len() {
            return -1;
        }
        let end = (s + l).min(v.len());
        v[s..end].to_vec()
    };
    let h = next_handle();
    pool().insert(h, new_vec);
    h
}

/// 比较两个字节切片：相等返回 0，否则非零
#[no_mangle]
pub extern "C" fn qi_bytes_compare(a: i64, b: i64) -> i64 {
    // 同样：先拷出 b 再拿 a，避免同一 shard 双键读取
    let vb = match pool().get(&b) {
        Some(v) => v.clone(),
        None => return i64::MIN,
    };
    match pool().get(&a) {
        Some(va) => {
            let va: &Vec<u8> = &va;
            if va == &vb {
                0
            } else if va < &vb {
                -1
            } else {
                1
            }
        }
        None => i64::MIN,
    }
}

/// 在 haystack 里查找 needle 切片，返回起始索引；找不到返回 -1
#[no_mangle]
pub extern "C" fn qi_bytes_find(haystack: i64, needle: i64) -> i64 {
    let h = match pool().get(&haystack) {
        Some(v) => v.clone(),
        None => return -1,
    };
    let n = match pool().get(&needle) {
        Some(v) => v.clone(),
        None => return -1,
    };
    if n.is_empty() {
        return 0;
    }
    if n.len() > h.len() {
        return -1;
    }
    for i in 0..=h.len() - n.len() {
        if h[i..i + n.len()] == n[..] {
            return i as i64;
        }
    }
    -1
}

/// 转十六进制字符串（小写）
#[no_mangle]
pub extern "C" fn qi_bytes_to_hex(handle: i64) -> *mut c_char {
    let hex = match pool().get(&handle) {
        Some(v) => v.iter().map(|b| format!("{:02x}", b)).collect::<String>(),
        None => String::new(),
    };
    crate::stdlib::qi_str::rc_cstr_from_string(hex)
}

/// 从十六进制字符串构造字节切片；非法字符返回 -1
#[no_mangle]
pub extern "C" fn qi_bytes_from_hex(s: *const c_char) -> i64 {
    let bytes = match cstr_bytes(s) {
        Some(b) => b,
        None => return -1,
    };
    if bytes.len() % 2 != 0 {
        return -1;
    }
    let mut out = Vec::with_capacity(bytes.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = match (bytes[i] as char).to_digit(16) {
            Some(v) => v,
            None => return -1,
        };
        let lo = match (bytes[i + 1] as char).to_digit(16) {
            Some(v) => v,
            None => return -1,
        };
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    let h = next_handle();
    pool().insert(h, out);
    h
}

/// 转 Base64
#[no_mangle]
pub extern "C" fn qi_bytes_to_base64(handle: i64) -> *mut c_char {
    let bytes = match pool().get(&handle) {
        Some(v) => v.clone(),
        None => Vec::new(),
    };
    let s = encode_base64(&bytes);
    crate::stdlib::qi_str::rc_cstr_from_string(s)
}

/// 从 Base64 解码；非法返回 -1
#[no_mangle]
pub extern "C" fn qi_bytes_from_base64(s: *const c_char) -> i64 {
    let bytes = match cstr_bytes(s) {
        Some(b) => b,
        None => return -1,
    };
    match decode_base64(bytes) {
        Some(v) => {
            let h = next_handle();
            pool().insert(h, v);
            h
        }
        None => -1,
    }
}

/// 释放字节切片（持久负句柄是 no-op）
#[no_mangle]
pub extern "C" fn qi_bytes_free(handle: i64) -> i64 {
    free_bytes(handle);
    0
}

/// 把字节切片原样写入文件（二进制安全——multipart 上传落盘、图片存储等）。
/// 成功返回写入字节数，失败返回 -1。支持持久负句柄。
/// 数据先克隆出来、**在池锁外做磁盘 IO**（勿持池锁写盘，见 TCP/UDP/WS 池教训）。
#[no_mangle]
pub extern "C" fn qi_bytes_write_file(handle: i64, path_ptr: *const c_char) -> i64 {
    if path_ptr.is_null() {
        return -1;
    }
    let path = unsafe { CStr::from_ptr(path_ptr).to_string_lossy().to_string() };
    let data: Vec<u8> = if handle < 0 {
        match persistent_arc(handle) {
            Some(a) => (*a).clone(),
            None => return -1,
        }
    } else {
        match clone_bytes(handle) {
            Some(v) => v,
            None => return -1,
        }
    };
    match std::fs::write(&path, &data) {
        Ok(_) => data.len() as i64,
        Err(_) => -1,
    }
}

/// 释放由 to_string / to_hex / to_base64 返回的 C 字符串（header-aware rc_cstr）
#[no_mangle]
pub extern "C" fn qi_bytes_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

// ===== 内置 Base64 编解码 (RFC 4648) — 不引入新依赖 =====

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn encode_base64(input: &[u8]) -> String {
    let mut out = String::with_capacity((input.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= input.len() {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8) | (input[i + 2] as u32);
        out.push(B64_TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64_TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(B64_TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push(B64_TABLE[(n & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = input.len() - i;
    if rem == 1 {
        let n = (input[i] as u32) << 16;
        out.push(B64_TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64_TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push('=');
        out.push('=');
    } else if rem == 2 {
        let n = ((input[i] as u32) << 16) | ((input[i + 1] as u32) << 8);
        out.push(B64_TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(B64_TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(B64_TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    }
    out
}

fn decode_base64(input: &[u8]) -> Option<Vec<u8>> {
    fn val(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let trimmed: Vec<u8> = input
        .iter()
        .copied()
        .filter(|c| !c.is_ascii_whitespace())
        .collect();
    if trimmed.is_empty() {
        return Some(Vec::new());
    }
    if trimmed.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(trimmed.len() * 3 / 4);
    let mut i = 0;
    while i < trimmed.len() {
        let q0 = val(trimmed[i])?;
        let q1 = val(trimmed[i + 1])?;
        let q2 = trimmed[i + 2];
        let q3 = trimmed[i + 3];
        let n = (q0 << 18) | (q1 << 12);
        out.push(((n >> 16) & 0xFF) as u8);
        if q2 != b'=' {
            let q2v = val(q2)?;
            let n = n | (q2v << 6);
            out.push(((n >> 8) & 0xFF) as u8);
            if q3 != b'=' {
                let q3v = val(q3)?;
                let n = n | q3v;
                out.push((n & 0xFF) as u8);
            }
        }
        i += 4;
    }
    Some(out)
}
