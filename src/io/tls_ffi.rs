//! TLS 模块 FFI 接口
//!
//! 基于 rustls 提供阻塞式 TLS 服务器能力。供 qi-web 等上层使用。
//!
//! 句柄空间：
//! - 配置句柄：服务器证书 + 私钥的 ServerConfig
//! - 监听器句柄：TcpListener + 共享的 ServerConfig（接受新连接时握手）
//! - 连接句柄：握手完成后的 (TcpStream, ServerConnection) 对，作为同步流读写

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::CStr;
use std::io::{BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::raw::c_char;
use std::sync::{Arc, Mutex, OnceLock};

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ServerConfig, ServerConnection, StreamOwned};

type Stream = StreamOwned<ServerConnection, TcpStream>;

static CONFIG_POOL: OnceLock<Mutex<HashMap<i64, Arc<ServerConfig>>>> = OnceLock::new();
static LISTENER_POOL: OnceLock<Mutex<HashMap<i64, (TcpListener, Arc<ServerConfig>)>>> =
    OnceLock::new();
static STREAM_POOL: OnceLock<Mutex<HashMap<i64, Stream>>> = OnceLock::new();
static HANDLE_COUNTER: OnceLock<Mutex<i64>> = OnceLock::new();

fn next_handle() -> i64 {
    let counter = HANDLE_COUNTER.get_or_init(|| Mutex::new(0));
    let mut guard = counter.lock().unwrap();
    *guard += 1;
    *guard
}

fn config_pool() -> &'static Mutex<HashMap<i64, Arc<ServerConfig>>> {
    CONFIG_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn listener_pool() -> &'static Mutex<HashMap<i64, (TcpListener, Arc<ServerConfig>)>> {
    LISTENER_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn stream_pool() -> &'static Mutex<HashMap<i64, Stream>> {
    STREAM_POOL.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cstr_to_string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { Some(CStr::from_ptr(p).to_string_lossy().into_owned()) }
}

fn install_default_provider_once() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn load_certs(path: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开证书 {} 失败: {}", path, e))?;
    let mut reader = BufReader::new(file);
    let certs: Result<Vec<_>, _> = rustls_pemfile::certs(&mut reader).collect();
    let certs = certs.map_err(|e| format!("解析证书 {} 失败: {}", path, e))?;
    if certs.is_empty() {
        return Err(format!("{} 不包含证书", path));
    }
    Ok(certs)
}

fn load_private_key(path: &str) -> Result<PrivateKeyDer<'static>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开私钥 {} 失败: {}", path, e))?;
    let mut reader = BufReader::new(file);
    let key = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| format!("解析私钥 {} 失败: {}", path, e))?
        .ok_or_else(|| format!("{} 不包含可用私钥", path))?;
    Ok(key)
}

/// 创建一个 TLS 服务器配置
/// cert_path / key_path 都是 PEM 文件路径
/// 成功返回 配置句柄 (>0)，失败返回 -1
#[no_mangle]
pub extern "C" fn qi_tls_create_config(cert_path: *const c_char, key_path: *const c_char) -> i64 {
    install_default_provider_once();

    let cert_path = match cstr_to_string(cert_path) {
        Some(s) => s,
        None => return -1,
    };
    let key_path = match cstr_to_string(key_path) {
        Some(s) => s,
        None => return -1,
    };

    let certs = match load_certs(&cert_path) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[qi-tls] {}", e);
            return -1;
        }
    };

    let key = match load_private_key(&key_path) {
        Ok(k) => k,
        Err(e) => {
            eprintln!("[qi-tls] {}", e);
            return -1;
        }
    };

    let config = match ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[qi-tls] 构建 TLS 配置失败: {}", e);
            return -1;
        }
    };

    let handle = next_handle();
    config_pool()
        .lock()
        .unwrap()
        .insert(handle, Arc::new(config));
    handle
}

/// 释放 TLS 配置
#[no_mangle]
pub extern "C" fn qi_tls_free_config(handle: i64) -> i64 {
    config_pool().lock().unwrap().remove(&handle);
    0
}

/// TLS 监听
/// 在指定 host:port 绑定一个 TCP 监听器，关联给定的 TLS 配置
/// 端口和 backlog 用 i64 以匹配 qi 的整数 ABI（runtime 内部按需要截断）
#[no_mangle]
pub extern "C" fn qi_tls_listen(
    host: *const c_char,
    port: i64,
    backlog: i64,
    config_handle: i64,
) -> i64 {
    let port = port as u16;
    let _ = backlog; // std 不直接暴露 backlog 控制
    let host = match cstr_to_string(host) {
        Some(s) => s,
        None => return -1,
    };

    let config = {
        let pool = config_pool().lock().unwrap();
        match pool.get(&config_handle) {
            Some(c) => c.clone(),
            None => return -1,
        }
    };

    let addr = format!("{}:{}", host, port);
    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[qi-tls] 绑定 {} 失败: {}", addr, e);
            return -1;
        }
    };

    let handle = next_handle();
    listener_pool()
        .lock()
        .unwrap()
        .insert(handle, (listener, config));
    handle
}

/// 接受一个 TLS 连接（阻塞，包含 TLS 握手）
/// 成功返回连接句柄 (>0)，失败返回 -1
#[no_mangle]
pub extern "C" fn qi_tls_accept(server_handle: i64) -> i64 {
    // 复制出 listener 引用 + config，避免 accept 时持锁阻塞别的连接
    let (stream, config) = {
        let pool = listener_pool().lock().unwrap();
        let (listener, config) = match pool.get(&server_handle) {
            Some(v) => v,
            None => return -1,
        };
        let listener = match listener.try_clone() {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[qi-tls] try_clone listener 失败: {}", e);
                return -1;
            }
        };
        let config = config.clone();
        drop(pool);

        match listener.accept() {
            Ok((s, _addr)) => (s, config),
            Err(e) => {
                eprintln!("[qi-tls] accept 失败: {}", e);
                return -1;
            }
        }
    };

    let server_conn = match ServerConnection::new(config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[qi-tls] ServerConnection::new 失败: {}", e);
            return -1;
        }
    };

    let mut tls_stream = StreamOwned::new(server_conn, stream);
    // 主动完成握手，让后续 read/write 是已握手的明文
    if let Err(e) = tls_stream.flush() {
        let _ = e; // flush 在握手前可能没数据可发；忽略
    }
    if let Err(e) = tls_stream.conn.complete_io(&mut tls_stream.sock) {
        eprintln!("[qi-tls] 握手失败: {}", e);
        return -1;
    }

    let handle = next_handle();
    stream_pool().lock().unwrap().insert(handle, tls_stream);
    handle
}

/// 从 TLS 连接读取一段字符串（最多 buffer_size 字节）。
/// 返回新分配的 C 字符串；读 0 字节或错误时返回空字符串。
/// 调用方负责通过 qi_tls_free_string 释放。
#[no_mangle]
pub extern "C" fn qi_tls_read_string(handle: i64, buffer_size: i64) -> *mut c_char {
    let size = if buffer_size <= 0 {
        4096
    } else {
        buffer_size as usize
    };
    let mut buf = vec![0u8; size];

    let n = {
        let mut pool = stream_pool().lock().unwrap();
        match pool.get_mut(&handle) {
            Some(s) => match s.read(&mut buf) {
                Ok(n) => n,
                Err(_) => 0,
            },
            None => 0,
        }
    };

    let slice = &buf[..n];
    // 转成有效 UTF-8 字符串（lossy 处理非法字节）
    let text = String::from_utf8_lossy(slice).into_owned();
    crate::stdlib::qi_str::rc_cstr_from_string(text)
}

/// 向 TLS 连接写入 C 字符串
/// 返回写入字节数（< 0 失败）
#[no_mangle]
pub extern "C" fn qi_tls_write_string(handle: i64, data: *const c_char) -> i64 {
    if data.is_null() {
        return -1;
    }
    let bytes = unsafe { CStr::from_ptr(data) }.to_bytes();
    let mut pool = stream_pool().lock().unwrap();
    match pool.get_mut(&handle) {
        Some(s) => match s.write_all(bytes) {
            Ok(_) => bytes.len() as i64,
            Err(e) => {
                eprintln!("[qi-tls] write 失败: {}", e);
                -1
            }
        },
        None => -1,
    }
}

/// 关闭 TLS 连接
#[no_mangle]
pub extern "C" fn qi_tls_close(handle: i64) -> i64 {
    let mut pool = stream_pool().lock().unwrap();
    if let Some(mut s) = pool.remove(&handle) {
        // 发送 close_notify
        s.conn.send_close_notify();
        let _ = s.flush();
    }
    0
}

/// 关闭 TLS 监听器
#[no_mangle]
pub extern "C" fn qi_tls_server_close(server_handle: i64) -> i64 {
    listener_pool().lock().unwrap().remove(&server_handle);
    0
}

/// 释放由 qi_tls_read_string 返回的字符串
/// （委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃）
#[no_mangle]
pub extern "C" fn qi_tls_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}
