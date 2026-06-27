//! WebSocket 模块 FFI 接口
//!
//! 为 Qi 语言提供 WebSocket 协议支持

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::os::raw::c_char;
use std::sync::Mutex;

use std::sync::OnceLock;

// WebSocket 连接池
static WEBSOCKET连接池: OnceLock<Mutex<HashMap<i64, WebSocketConnection>>> = OnceLock::new();
static WS句柄计数器: OnceLock<Mutex<i64>> = OnceLock::new();

fn 获取WS连接池() -> &'static Mutex<HashMap<i64, WebSocketConnection>> {
    WEBSOCKET连接池.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 获取WS句柄计数器() -> &'static Mutex<i64> {
    WS句柄计数器.get_or_init(|| Mutex::new(1000)) // 从 1000 开始避免与 TCP 句柄冲突
}

/// WebSocket 操作码
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum WebSocketOpcode {
    Continuation = 0x0,
    Text = 0x1,
    Binary = 0x2,
    Close = 0x8,
    Ping = 0x9,
    Pong = 0xA,
}

impl WebSocketOpcode {
    fn from_u8(value: u8) -> Option<Self> {
        match value {
            0x0 => Some(WebSocketOpcode::Continuation),
            0x1 => Some(WebSocketOpcode::Text),
            0x2 => Some(WebSocketOpcode::Binary),
            0x8 => Some(WebSocketOpcode::Close),
            0x9 => Some(WebSocketOpcode::Ping),
            0xA => Some(WebSocketOpcode::Pong),
            _ => None,
        }
    }
}

/// WebSocket 帧
#[derive(Debug)]
pub struct WebSocketFrame {
    pub fin: bool,
    pub opcode: WebSocketOpcode,
    pub payload: Vec<u8>,
}

/// WebSocket 连接状态
pub struct WebSocketConnection {
    stream: TcpStream,
    is_server: bool, // true = 服务器端, false = 客户端
    is_connected: bool,
}

impl WebSocketConnection {
    /// 从已升级的 TCP 连接创建 WebSocket 连接
    pub fn from_upgraded_stream(stream: TcpStream, is_server: bool) -> Self {
        WebSocketConnection {
            stream,
            is_server,
            is_connected: true,
        }
    }

    /// 发送 WebSocket 帧
    pub fn send_frame(
        &mut self,
        opcode: WebSocketOpcode,
        payload: &[u8],
    ) -> Result<(), std::io::Error> {
        let mut frame = Vec::new();

        // 第一字节: FIN + opcode
        frame.push(0x80 | (opcode as u8));

        // 第二字节及后续: 长度 + mask bit
        let mask_bit = if self.is_server { 0x00 } else { 0x80 }; // 客户端发送需要 mask

        if payload.len() < 126 {
            frame.push(mask_bit | (payload.len() as u8));
        } else if payload.len() < 65536 {
            frame.push(mask_bit | 126);
            frame.push((payload.len() >> 8) as u8);
            frame.push(payload.len() as u8);
        } else {
            frame.push(mask_bit | 127);
            for i in (0..8).rev() {
                frame.push((payload.len() >> (i * 8)) as u8);
            }
        }

        // 如果是客户端，添加 mask key 并 mask 数据
        if !self.is_server {
            let mask_key: [u8; 4] = rand_mask_key();
            frame.extend_from_slice(&mask_key);

            for (i, byte) in payload.iter().enumerate() {
                frame.push(byte ^ mask_key[i % 4]);
            }
        } else {
            frame.extend_from_slice(payload);
        }

        self.stream.write_all(&frame)?;
        self.stream.flush()?;
        Ok(())
    }

    /// 接收 WebSocket 帧
    pub fn recv_frame(&mut self) -> Result<WebSocketFrame, std::io::Error> {
        let mut header = [0u8; 2];
        self.stream.read_exact(&mut header)?;

        let fin = (header[0] & 0x80) != 0;
        let opcode = WebSocketOpcode::from_u8(header[0] & 0x0F).ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "Invalid opcode")
        })?;

        let masked = (header[1] & 0x80) != 0;
        let mut payload_len = (header[1] & 0x7F) as u64;

        // 读取扩展长度
        if payload_len == 126 {
            let mut ext_len = [0u8; 2];
            self.stream.read_exact(&mut ext_len)?;
            payload_len = u16::from_be_bytes(ext_len) as u64;
        } else if payload_len == 127 {
            let mut ext_len = [0u8; 8];
            self.stream.read_exact(&mut ext_len)?;
            payload_len = u64::from_be_bytes(ext_len);
        }

        // 读取 mask key（如果存在）
        let mask_key = if masked {
            let mut key = [0u8; 4];
            self.stream.read_exact(&mut key)?;
            Some(key)
        } else {
            None
        };

        // 读取 payload
        let mut payload = vec![0u8; payload_len as usize];
        self.stream.read_exact(&mut payload)?;

        // 如果有 mask，解码
        if let Some(key) = mask_key {
            for (i, byte) in payload.iter_mut().enumerate() {
                *byte ^= key[i % 4];
            }
        }

        Ok(WebSocketFrame {
            fin,
            opcode,
            payload,
        })
    }

    /// 发送文本消息
    pub fn send_text(&mut self, text: &str) -> Result<(), std::io::Error> {
        self.send_frame(WebSocketOpcode::Text, text.as_bytes())
    }

    /// 发送二进制消息
    pub fn send_binary(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
        self.send_frame(WebSocketOpcode::Binary, data)
    }

    /// 发送 ping
    pub fn send_ping(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
        self.send_frame(WebSocketOpcode::Ping, data)
    }

    /// 发送 pong
    pub fn send_pong(&mut self, data: &[u8]) -> Result<(), std::io::Error> {
        self.send_frame(WebSocketOpcode::Pong, data)
    }

    /// 发送关闭帧
    pub fn send_close(&mut self, code: u16, reason: &str) -> Result<(), std::io::Error> {
        let mut payload = Vec::new();
        payload.push((code >> 8) as u8);
        payload.push(code as u8);
        payload.extend_from_slice(reason.as_bytes());
        self.send_frame(WebSocketOpcode::Close, &payload)?;
        self.is_connected = false;
        Ok(())
    }
}

/// 生成随机 mask key
fn rand_mask_key() -> [u8; 4] {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    [
        (seed >> 24) as u8,
        (seed >> 16) as u8,
        (seed >> 8) as u8,
        seed as u8,
    ]
}

/// 计算 WebSocket Accept Key (使用 SHA-1)
fn compute_accept_key(client_key: &str) -> String {
    use sha1::{Digest, Sha1};

    const GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";
    let mut hasher = Sha1::new();
    hasher.update(client_key.trim());
    hasher.update(GUID);
    let result = hasher.finalize();
    base64_encode(&result)
}

/// Base64 编码
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::new();

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as usize;
        let b1 = chunk.get(1).copied().unwrap_or(0) as usize;
        let b2 = chunk.get(2).copied().unwrap_or(0) as usize;

        result.push(ALPHABET[b0 >> 2] as char);
        result.push(ALPHABET[((b0 & 0x03) << 4) | (b1 >> 4)] as char);

        if chunk.len() > 1 {
            result.push(ALPHABET[((b1 & 0x0F) << 2) | (b2 >> 6)] as char);
        } else {
            result.push('=');
        }

        if chunk.len() > 2 {
            result.push(ALPHABET[b2 & 0x3F] as char);
        } else {
            result.push('=');
        }
    }

    result
}

// ============================================================================
// FFI 函数
// ============================================================================

/// 服务器端: 处理 WebSocket 升级请求
/// 检查请求是否为 WebSocket 升级请求
/// 返回 1 = 是 WebSocket 升级请求, 0 = 不是
#[no_mangle]
pub extern "C" fn qi_websocket_is_upgrade_request(request_headers: *const c_char) -> i64 {
    if request_headers.is_null() {
        return 0;
    }

    let headers = unsafe { CStr::from_ptr(request_headers).to_string_lossy() };
    let headers_lower = headers.to_lowercase();

    if headers_lower.contains("upgrade: websocket")
        && headers_lower.contains("connection:")
        && headers_lower.contains("upgrade")
    {
        1
    } else {
        0
    }
}

/// 服务器端: 从请求中提取 WebSocket Key
/// 返回 key 字符串（需释放）
#[no_mangle]
pub extern "C" fn qi_websocket_get_client_key(request_headers: *const c_char) -> *mut c_char {
    if request_headers.is_null() {
        return CString::new("").unwrap().into_raw();
    }

    let headers = unsafe { CStr::from_ptr(request_headers).to_string_lossy() };

    for line in headers.lines() {
        let line_lower = line.to_lowercase();
        if line_lower.starts_with("sec-websocket-key:") {
            let key = line[18..].trim();
            return CString::new(key).unwrap().into_raw();
        }
    }

    CString::new("").unwrap().into_raw()
}

/// 服务器端: 生成 WebSocket 升级响应
/// 返回 HTTP 响应字符串（需释放）
#[no_mangle]
pub extern "C" fn qi_websocket_create_upgrade_response(client_key: *const c_char) -> *mut c_char {
    if client_key.is_null() {
        return CString::new("").unwrap().into_raw();
    }

    let key = unsafe { CStr::from_ptr(client_key).to_string_lossy() };
    let accept_key = compute_accept_key(&key);

    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        accept_key
    );

    CString::new(response).unwrap().into_raw()
}

/// 服务器端: 接受 WebSocket 连接
/// 在已有的 TCP 服务器上接受连接并自动完成 WebSocket 握手
/// server_handle: TCP 服务器句柄
/// 返回 WebSocket 句柄 (>0 成功, <0 失败)
#[no_mangle]
pub extern "C" fn qi_websocket_accept(host: *const c_char, port: u16) -> i64 {
    if host.is_null() {
        return -1;
    }

    let host_str = unsafe { CStr::from_ptr(host).to_string_lossy() };
    let addr = format!("{}:{}", host_str, port);

    // 创建 TCP 监听器
    let listener = match std::net::TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(_) => return -2,
    };

    // 接受连接
    let (mut stream, _) = match listener.accept() {
        Ok(conn) => conn,
        Err(_) => return -3,
    };

    // 读取 HTTP 请求
    let mut request = vec![0u8; 4096];
    let n = match stream.read(&mut request) {
        Ok(n) => n,
        Err(_) => return -4,
    };

    let request_str = String::from_utf8_lossy(&request[..n]);

    // 检查是否是 WebSocket 升级请求
    if !request_str.to_lowercase().contains("upgrade: websocket") {
        return -5;
    }

    // 提取 Sec-WebSocket-Key
    let mut client_key = String::new();
    for line in request_str.lines() {
        if line.to_lowercase().starts_with("sec-websocket-key:") {
            client_key = line[18..].trim().to_string();
            break;
        }
    }

    if client_key.is_empty() {
        return -6;
    }

    // 计算 Accept Key 并发送响应
    let accept_key = compute_accept_key(&client_key);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        accept_key
    );

    if stream.write_all(response.as_bytes()).is_err() {
        return -7;
    }

    // 创建 WebSocket 连接
    let ws_conn = WebSocketConnection::from_upgraded_stream(stream, true);

    let mut 句柄计数 = 获取WS句柄计数器().lock().unwrap();
    *句柄计数 += 1;
    let 句柄 = *句柄计数;

    let mut ws_pool = 获取WS连接池().lock().unwrap();
    ws_pool.insert(句柄, ws_conn);

    句柄
}

/// 服务器端: 从已存在的 TcpStream 升级为 WebSocket
/// 这个函数用于在 HTTP 请求已被读取后升级连接
#[no_mangle]
pub extern "C" fn qi_websocket_upgrade_connection(
    host: *const c_char,
    port: u16,
    client_key: *const c_char,
) -> i64 {
    if host.is_null() || client_key.is_null() {
        return -1;
    }

    let host_str = unsafe { CStr::from_ptr(host).to_string_lossy() };
    let key_str = unsafe { CStr::from_ptr(client_key).to_string_lossy() };
    let addr = format!("{}:{}", host_str, port);

    // 连接到指定地址（用于测试目的）
    let mut stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(_) => return -2,
    };

    // 计算 Accept Key 并发送响应
    let accept_key = compute_accept_key(&key_str);
    let response = format!(
        "HTTP/1.1 101 Switching Protocols\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Accept: {}\r\n\r\n",
        accept_key
    );

    if stream.write_all(response.as_bytes()).is_err() {
        return -3;
    }

    // 创建 WebSocket 连接
    let ws_conn = WebSocketConnection::from_upgraded_stream(stream, true);

    let mut 句柄计数 = 获取WS句柄计数器().lock().unwrap();
    *句柄计数 += 1;
    let 句柄 = *句柄计数;

    let mut ws_pool = 获取WS连接池().lock().unwrap();
    ws_pool.insert(句柄, ws_conn);

    句柄
}

/// 客户端: 连接到 WebSocket 服务器
/// 返回 WebSocket 句柄 (>0 成功, <0 失败)
#[no_mangle]
pub extern "C" fn qi_websocket_connect(url: *const c_char) -> i64 {
    if url.is_null() {
        return -1;
    }

    let url_str = unsafe { CStr::from_ptr(url).to_string_lossy() };

    // 解析 URL: ws://host:port/path or wss://host:port/path
    let (host, port, path) = match parse_websocket_url(&url_str) {
        Some(parsed) => parsed,
        None => return -2,
    };

    // 连接到服务器
    let addr = format!("{}:{}", host, port);
    let mut stream = match TcpStream::connect(&addr) {
        Ok(s) => s,
        Err(_) => return -3,
    };

    // 生成随机 key
    let key = generate_websocket_key();

    // 发送升级请求
    let request = format!(
        "GET {} HTTP/1.1\r\n\
         Host: {}:{}\r\n\
         Upgrade: websocket\r\n\
         Connection: Upgrade\r\n\
         Sec-WebSocket-Key: {}\r\n\
         Sec-WebSocket-Version: 13\r\n\r\n",
        path, host, port, key
    );

    if stream.write_all(request.as_bytes()).is_err() {
        return -4;
    }

    // 读取响应
    let mut response = vec![0u8; 1024];
    let n = match stream.read(&mut response) {
        Ok(n) => n,
        Err(_) => return -5,
    };

    let response_str = String::from_utf8_lossy(&response[..n]);

    // 验证响应
    if !response_str.contains("101") || !response_str.to_lowercase().contains("upgrade") {
        return -6;
    }

    // 创建 WebSocket 连接
    let ws_conn = WebSocketConnection::from_upgraded_stream(stream, false);

    let mut 句柄计数 = 获取WS句柄计数器().lock().unwrap();
    *句柄计数 += 1;
    let 句柄 = *句柄计数;

    let mut ws_pool = 获取WS连接池().lock().unwrap();
    ws_pool.insert(句柄, ws_conn);

    句柄
}

/// 解析 WebSocket URL
fn parse_websocket_url(url: &str) -> Option<(String, u16, String)> {
    let url = url.trim();
    let (scheme, rest) = if url.starts_with("ws://") {
        ("ws", &url[5..])
    } else if url.starts_with("wss://") {
        ("wss", &url[6..])
    } else {
        return None;
    };

    let default_port = if scheme == "wss" { 443 } else { 80 };

    let (host_port, path) = if let Some(idx) = rest.find('/') {
        (&rest[..idx], &rest[idx..])
    } else {
        (rest, "/")
    };

    let (host, port) = if let Some(idx) = host_port.find(':') {
        let host = &host_port[..idx];
        let port: u16 = host_port[idx + 1..].parse().ok()?;
        (host.to_string(), port)
    } else {
        (host_port.to_string(), default_port)
    };

    Some((host, port, path.to_string()))
}

/// 生成 WebSocket 客户端 key
fn generate_websocket_key() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    let bytes: Vec<u8> = (0..16).map(|i| ((seed >> (i * 4)) & 0xFF) as u8).collect();
    base64_encode(&bytes)
}

/// 发送文本消息
/// 返回 1 成功, 0 失败
#[no_mangle]
pub extern "C" fn qi_websocket_send_text(handle: i64, message: *const c_char) -> i64 {
    if message.is_null() {
        return 0;
    }

    let text = unsafe { CStr::from_ptr(message).to_string_lossy() };

    let mut ws_pool = 获取WS连接池().lock().unwrap();
    if let Some(conn) = ws_pool.get_mut(&handle) {
        match conn.send_text(&text) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    } else {
        0
    }
}

/// 发送二进制消息
/// 返回 1 成功, 0 失败
#[no_mangle]
pub extern "C" fn qi_websocket_send_binary(handle: i64, data: *const u8, size: i64) -> i64 {
    if data.is_null() || size <= 0 {
        return 0;
    }

    let bytes = unsafe { std::slice::from_raw_parts(data, size as usize) };

    let mut ws_pool = 获取WS连接池().lock().unwrap();
    if let Some(conn) = ws_pool.get_mut(&handle) {
        match conn.send_binary(bytes) {
            Ok(_) => 1,
            Err(_) => 0,
        }
    } else {
        0
    }
}

/// 接收消息
/// 返回消息类型: 1=文本, 2=二进制, 8=关闭, 9=ping, 10=pong, 0=错误
/// 消息内容写入 buffer
#[no_mangle]
pub extern "C" fn qi_websocket_recv(handle: i64, buffer: *mut u8, buffer_size: i64) -> i64 {
    if buffer.is_null() || buffer_size <= 0 {
        return 0;
    }

    let mut ws_pool = 获取WS连接池().lock().unwrap();
    if let Some(conn) = ws_pool.get_mut(&handle) {
        match conn.recv_frame() {
            Ok(frame) => {
                let copy_len = std::cmp::min(frame.payload.len(), buffer_size as usize);
                unsafe {
                    std::ptr::copy_nonoverlapping(frame.payload.as_ptr(), buffer, copy_len);
                }

                // 自动回复 ping
                if frame.opcode == WebSocketOpcode::Ping {
                    let _ = conn.send_pong(&frame.payload);
                }

                frame.opcode as i64
            }
            Err(_) => 0,
        }
    } else {
        0
    }
}

/// 接收文本消息（简化版本）
/// 返回消息字符串（需释放）
#[no_mangle]
pub extern "C" fn qi_websocket_recv_text(handle: i64) -> *mut c_char {
    let mut ws_pool = 获取WS连接池().lock().unwrap();
    if let Some(conn) = ws_pool.get_mut(&handle) {
        loop {
            match conn.recv_frame() {
                Ok(frame) => {
                    // 自动回复 ping
                    if frame.opcode == WebSocketOpcode::Ping {
                        let _ = conn.send_pong(&frame.payload);
                        continue; // 继续接收下一帧
                    }

                    if frame.opcode == WebSocketOpcode::Text {
                        if let Ok(text) = String::from_utf8(frame.payload) {
                            return CString::new(text).unwrap().into_raw();
                        }
                    }

                    // 收到关闭帧
                    if frame.opcode == WebSocketOpcode::Close {
                        conn.is_connected = false;
                        return CString::new("").unwrap().into_raw();
                    }
                }
                Err(_) => {
                    // 读取错误，标记连接已断开
                    conn.is_connected = false;
                    break;
                }
            }
        }
    }

    CString::new("").unwrap().into_raw()
}

/// 发送 ping
/// 返回 1 成功, 0 失败
#[no_mangle]
pub extern "C" fn qi_websocket_ping(handle: i64) -> i64 {
    let mut ws_pool = 获取WS连接池().lock().unwrap();
    if let Some(conn) = ws_pool.get_mut(&handle) {
        match conn.send_ping(b"ping") {
            Ok(_) => 1,
            Err(_) => 0,
        }
    } else {
        0
    }
}

/// 关闭 WebSocket 连接
/// 返回 1 成功, 0 失败
#[no_mangle]
pub extern "C" fn qi_websocket_close(handle: i64, code: u16, reason: *const c_char) -> i64 {
    let reason_str = if reason.is_null() {
        "".to_string()
    } else {
        unsafe { CStr::from_ptr(reason).to_string_lossy().to_string() }
    };

    let mut ws_pool = 获取WS连接池().lock().unwrap();
    if let Some(mut conn) = ws_pool.remove(&handle) {
        let _ = conn.send_close(code, &reason_str);
        1
    } else {
        0
    }
}

/// 释放字符串内存
#[no_mangle]
pub extern "C" fn qi_websocket_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

/// 检查连接是否仍然有效
/// 返回 1 有效, 0 无效
#[no_mangle]
pub extern "C" fn qi_websocket_is_connected(handle: i64) -> i64 {
    let ws_pool = 获取WS连接池().lock().unwrap();
    if let Some(conn) = ws_pool.get(&handle) {
        if conn.is_connected {
            1
        } else {
            0
        }
    } else {
        0
    }
}

/// 将已有的 TCP 连接句柄注册为 WebSocket 连接
/// 用于服务器端：在完成 HTTP 握手后，将 TCP 连接升级为 WebSocket
/// tcp_handle: TCP 连接池中的句柄（不是原始文件描述符）
/// is_server: 1 = 服务器端, 0 = 客户端
/// 返回 WebSocket 句柄 (>0 成功, <0 失败)
#[no_mangle]
pub extern "C" fn qi_websocket_register_tcp(tcp_handle: i64, is_server: i64) -> i64 {
    if tcp_handle < 0 {
        return -1;
    }

    // 从TCP连接池中克隆TcpStream
    // 使用克隆而不是取出，这样原TCP句柄仍然可用于关闭等操作
    let stream = match super::network_ffi::克隆TCP流(tcp_handle) {
        Some(s) => s,
        None => {
            eprintln!("[WebSocket] 无法从TCP句柄 {} 获取流", tcp_handle);
            return -1;
        }
    };

    // 设置为阻塞模式
    if let Err(_) = stream.set_nonblocking(false) {
        // 忽略错误，继续使用默认模式
    }

    // 创建 WebSocket 连接
    let ws_conn = WebSocketConnection::from_upgraded_stream(stream, is_server == 1);

    let mut 句柄计数 = 获取WS句柄计数器().lock().unwrap();
    *句柄计数 += 1;
    let 句柄 = *句柄计数;

    let mut ws_pool = 获取WS连接池().lock().unwrap();
    ws_pool.insert(句柄, ws_conn);

    句柄
}

/// 从 WebSocket 连接池中移除连接但不关闭底层 TCP
/// 用于需要单独管理 TCP 生命周期的场景
/// 返回 1 成功, 0 失败
#[no_mangle]
pub extern "C" fn qi_websocket_unregister(handle: i64) -> i64 {
    let mut ws_pool = 获取WS连接池().lock().unwrap();
    if ws_pool.remove(&handle).is_some() {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_base64_encode() {
        assert_eq!(base64_encode(b"hello"), "aGVsbG8=");
        assert_eq!(base64_encode(b"world"), "d29ybGQ=");
    }

    #[test]
    fn test_compute_accept_key() {
        // 标准测试向量
        let key = "dGhlIHNhbXBsZSBub25jZQ==";
        let expected = "s3pPLMBiTxaQ9kYGzzhZRbK+xOo=";
        assert_eq!(compute_accept_key(key), expected);
    }

    #[test]
    fn test_parse_url() {
        let (host, port, path) = parse_websocket_url("ws://localhost:8080/chat").unwrap();
        assert_eq!(host, "localhost");
        assert_eq!(port, 8080);
        assert_eq!(path, "/chat");

        let (host, port, path) = parse_websocket_url("ws://example.com").unwrap();
        assert_eq!(host, "example.com");
        assert_eq!(port, 80);
        assert_eq!(path, "/");
    }
}
