//! MCP 客户端核心 FFI（标准库.MCP客户端）
//!
//! Rust 实现的 MCP 客户端，解决纯 Qi 实现的两个根本问题：
//!   1. 大 SSE 响应（如 browser_evaluate）被截断 → 裸 TcpStream 读到 EOF 全量读取。
//!   2. id 未关联 → 每条请求分配单调递增 id，在响应关联 map 中匹配。
//!
//! 传输：
//!   - stdio：复用 subprocess_ffi 的子进程基础设施（spawn + 后台 reader），
//!     在此层再加 id 关联，允许多并发请求（理论上；MCP stdio 通常串行）。
//!   - HTTP：裸 std::net::TcpStream 手写 HTTP/1.1 客户端，每次 POST 读全部 body，
//!     对 `text/event-stream` 应答解析出第一条（也通常只有一条）`data:` 行。
//!     （历史：曾用 reqwest::blocking，但在 qi-web 进程内发大 POST body 时，
//!      reqwest 内部 tokio runtime 与外层 hyper server runtime 交互，导致
//!      Playwright 返回 200 空 body / 会话失效。裸 TcpStream 无 runtime，彻底规避。）
//!
//! 连接描述符格式（Qi 侧字符串）：
//!   "mcpc|<conn_id>"
//!
//! 公开 FFI：
//!   qi_mcpc_connect_stdio(cmd, args_json) -> i64   (>0 = conn_id, <=0 = 失败)
//!   qi_mcpc_connect_http(base_url)        -> i64
//!   qi_mcpc_request(conn_id, method, params_json) -> *mut c_char (JSON result/error 串)
//!   qi_mcpc_close(conn_id)                -> i32
//!   qi_mcpc_free_string(ptr)              -> void

#![allow(non_snake_case)]

use std::collections::{HashMap, VecDeque};
use std::ffi::{CStr, CString};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::os::raw::c_char;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value as Json};

/// 缓冲的服务器→客户端通知上限（环形缓冲）。
const MAX_NOTIFICATIONS: usize = 64;

/// 调用 Qi 闭包：传入 params JSON 串，返回结果 JSON 串。
///
/// Qi 闭包调用约定（与 mcp_ffi.rs 的 invoke_tool 完全一致）：
///   - `obj_addr` 是 Qi 闭包对象地址；
///   - 对象 offset 0 存的是 trampoline 函数指针；
///   - trampoline ABI: `extern "C" fn(env: *const c_void, args_json: *const c_char) -> *mut c_char`；
///   - 调用时传 env=obj_addr, args_json=入参 JSON 串，返回结果 JSON 串。
///
/// 用 `usize` 存地址：闭包对象地址是 Qi GC 管理的全局，进程生命周期内有效；
/// 跨线程（后台 reader 线程）调用与 server 侧同构。
/// `None` 表示未注册或调用失败。
fn invoke_qi_closure(obj_addr: usize, params_json: &str) -> Option<String> {
    let c_args = CString::new(params_json.replace('\0', "\u{FFFD}")).ok()?;
    let result_ptr = unsafe {
        // 闭包对象 offset 0 = trampoline 函数指针
        let obj_ptr = obj_addr as *const *const std::ffi::c_void;
        let trampoline_raw = *obj_ptr;
        if trampoline_raw.is_null() {
            return None;
        }
        let trampoline = std::mem::transmute::<
            *const std::ffi::c_void,
            extern "C" fn(*const std::ffi::c_void, *const c_char) -> *mut c_char,
        >(trampoline_raw);
        let env_ptr = obj_addr as *const std::ffi::c_void;
        trampoline(env_ptr, c_args.as_ptr())
    };
    if result_ptr.is_null() {
        return None;
    }
    let s = unsafe { CStr::from_ptr(result_ptr).to_string_lossy().to_string() };
    // 不释放 result_ptr：Qi runtime 字符串由 Qi GC 管理，CString::from_raw 会双重释放。
    Some(s)
}

/// server→client 处理器 + 配置（每连接一份，后台 reader 线程读取）。
struct ServerHandlers {
    /// sampling/createMessage 处理器（Qi 闭包对象地址）。
    sampling: Option<usize>,
    /// elicitation/create 处理器（Qi 闭包对象地址）。
    elicitation: Option<usize>,
    /// roots/list 返回的 roots 数组（JSON）。默认 `[]`。
    roots: Json,
    /// 缓冲的通知（环形，capped at MAX_NOTIFICATIONS）。
    notifications: VecDeque<Json>,
}

impl Default for ServerHandlers {
    fn default() -> Self {
        ServerHandlers {
            sampling: None,
            elicitation: None,
            roots: json!([]),
            notifications: VecDeque::new(),
        }
    }
}

/// 标准客户端 capabilities：声明支持 sampling 与 roots（listChanged:false）。
fn client_capabilities() -> Json {
    json!({
        "sampling": {},
        "roots": { "listChanged": false }
    })
}

// ─────────────────────────────────────────────────────────────────────────────
// 传输类型
// ─────────────────────────────────────────────────────────────────────────────

/// stdio 子进程状态（共享给后台读线程 + 主线程写）
struct StdioChild {
    _child: Child,     // 保持子进程存活
    stdin: ChildStdin, // 写端（序列化用，须持锁）
    /// 按 id 存储的响应队列（有 id 的消息，即 response）
    responses: Arc<Mutex<HashMap<i64, Json>>>,
    eof: Arc<AtomicBool>,
    /// server→client 请求处理器 + 通知缓冲（后台 reader 线程读，FFI 写）
    handlers: Arc<Mutex<ServerHandlers>>,
}

/// HTTP(Streamable) MCP 连接状态。
///
/// ⚠️ 关键：Playwright MCP 把会话绑定在 TCP 连接上 —— 一旦某个请求带
/// `Connection: close` 并断开，整条会话立刻失效（后续请求 404 Session not found）。
/// 因此这里必须复用【同一条 keep-alive 连接】跑完整个会话，而不是每请求新建/关闭。
struct HttpConn {
    base_url: String,
    session_id: String,
    /// 持久 keep-alive 连接（懒建/断后重连）。须持锁串行化请求。
    stream: Option<TcpStream>,
}

enum Transport {
    Stdio { child_state: Arc<Mutex<StdioChild>> },
    Http { http: Arc<Mutex<HttpConn>> },
}

struct Connection {
    transport: Transport,
    /// 每条请求分配唯一 id（单调递增）
    next_id: AtomicI64,
}

// ─────────────────────────────────────────────────────────────────────────────
// 全局连接注册表
// ─────────────────────────────────────────────────────────────────────────────

type ConnRegistry = Mutex<HashMap<i64, Arc<Connection>>>;

fn conn_registry() -> &'static ConnRegistry {
    static REG: OnceLock<ConnRegistry> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

static CONN_COUNTER: AtomicI64 = AtomicI64::new(1);

fn next_conn_id() -> i64 {
    CONN_COUNTER.fetch_add(1, Ordering::SeqCst)
}

fn get_conn(id: i64) -> Option<Arc<Connection>> {
    conn_registry().lock().ok()?.get(&id).cloned()
}

// ─────────────────────────────────────────────────────────────────────────────
// 辅助：将 Rust String 转 C 字符串（所有权移出）
// ─────────────────────────────────────────────────────────────────────────────

fn to_cstr(s: String) -> *mut c_char {
    // 替换 NUL 字节，防止 CString 创建失败
    match CString::new(s.replace('\0', "\u{FFFD}")) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

fn empty_cstr() -> *mut c_char {
    CString::new("").unwrap().into_raw()
}

// ─────────────────────────────────────────────────────────────────────────────
// SSE 解析：从 text/event-stream 响应体中提取 data: 行的 JSON
//
// Playwright MCP 和标准 MCP 服务器的 SSE 格式：
//   event: message\ndata: {...}\n\n
//
// 可能有多个 data: 行（如带进度通知的消息流），我们拼接所有 data: 行，
// 取最后一条包含 "result" 或 "error" 的 JSON-RPC 响应。
// ─────────────────────────────────────────────────────────────────────────────

fn parse_sse_body(body: &str) -> String {
    // 优先找包含 result 或 error 的 data: 行（JSON-RPC 响应）
    let mut last_data = String::new();

    for line in body.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("data:") {
            let data = rest.trim();
            if data.is_empty() {
                continue;
            }
            // 是有效 JSON 且含 result/error 字段的 JSON-RPC 响应
            if let Ok(v) = serde_json::from_str::<Json>(data) {
                if v.get("result").is_some() || v.get("error").is_some() {
                    return data.to_string();
                }
                // 记录最后一条有效 data: 行，用于 fallback
                last_data = data.to_string();
            } else {
                // 非 JSON，直接返回原始文本
                last_data = data.to_string();
            }
        }
    }

    // 没有找到明确的 result/error，返回最后一条 data: 行，或整个 body
    if !last_data.is_empty() {
        return last_data;
    }

    // 如果 body 本身就是 JSON（非 SSE 格式，直接 application/json 响应）
    body.trim().to_string()
}

// ─────────────────────────────────────────────────────────────────────────────
// HTTP MCP 请求辅助
// ─────────────────────────────────────────────────────────────────────────────

// ⚠️ 两个历史/隐藏 bug，本实现一并解决：
//   (1) reqwest::blocking 内部自建 tokio runtime。在 qi-web 进程内（已有外层
//       hyper/tokio HTTP server）发大 POST body 时，行为异常（空 body/会话失效）。
//       → 改用裸 std::net::TcpStream 手写 HTTP/1.1，无任何 async runtime。
//   (2) Playwright MCP 把会话绑定在 TCP 连接上：任何带 `Connection: close` 的请求
//       一旦断开，整条会话立即失效（后续 404 Session not found）。这才是「大请求空
//       body」真正的根因 —— 大请求恰好更容易触发连接被关/复用断裂。
//       → 复用同一条 keep-alive 连接跑完整个会话；按 Content-Length / chunked 精确
//         读取每条响应，绝不主动关闭连接。
// 仅支持 http://（localhost MCP 无需 TLS）。

/// 解析 `http://host:port/path` → (host, port, path)。
fn parse_http_url(url: &str) -> Result<(String, u16, String), String> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| format!("仅支持 http:// 的 MCP 端点: {}", url))?;
    // host[:port] 与 path 切分
    let (authority, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rfind(':') {
        Some(idx) => {
            let h = &authority[..idx];
            let p: u16 = authority[idx + 1..]
                .parse()
                .map_err(|_| format!("非法端口: {}", authority))?;
            (h.to_string(), p)
        }
        None => (authority.to_string(), 80u16),
    };
    let path = if path.is_empty() {
        "/".to_string()
    } else {
        path.to_string()
    };
    Ok((host, port, path))
}

/// 在响应头原文里大小写不敏感地查找 header 值。
fn find_header_value(headers: &str, name: &str) -> Option<String> {
    let name_lc = name.to_ascii_lowercase();
    for line in headers.lines() {
        if let Some(idx) = line.find(':') {
            let (k, v) = line.split_at(idx);
            if k.trim().to_ascii_lowercase() == name_lc {
                return Some(v[1..].trim().to_string());
            }
        }
    }
    None
}

/// 在已建立的 keep-alive 连接上读取【恰好一条】HTTP 响应。
/// 支持 Content-Length 与 chunked 两种 body 框架；不依赖 EOF / 连接关闭。
/// 返回 (响应头原文, body 字符串)。
fn read_one_http_response(stream: &mut TcpStream) -> Result<(String, String), String> {
    let mut buf: Vec<u8> = Vec::with_capacity(4096);
    let mut chunk = [0u8; 8192];

    // 1) 读到头部结束（\r\n\r\n）
    let header_end = loop {
        if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break p + 4;
        }
        let n = stream
            .read(&mut chunk)
            .map_err(|e| format!("read header 失败: {}", e))?;
        if n == 0 {
            return Err("连接在读取响应头时关闭（会话可能已失效）".to_string());
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let headers_text = String::from_utf8_lossy(&buf[..header_end - 4]).to_string();
    let content_length: Option<usize> =
        find_header_value(&headers_text, "Content-Length").and_then(|v| v.trim().parse().ok());
    let is_chunked = find_header_value(&headers_text, "Transfer-Encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);

    // 已经读到的 body 部分
    let mut body_raw: Vec<u8> = buf[header_end..].to_vec();

    if is_chunked {
        // 读到终止块 "0\r\n\r\n"
        while !body_raw.windows(5).any(|w| w == b"0\r\n\r\n") {
            let n = stream
                .read(&mut chunk)
                .map_err(|e| format!("read chunked 失败: {}", e))?;
            if n == 0 {
                break;
            }
            body_raw.extend_from_slice(&chunk[..n]);
        }
        let decoded = decode_chunked(&body_raw);
        Ok((headers_text, decoded))
    } else if let Some(cl) = content_length {
        while body_raw.len() < cl {
            let n = stream
                .read(&mut chunk)
                .map_err(|e| format!("read body 失败: {}", e))?;
            if n == 0 {
                break;
            }
            body_raw.extend_from_slice(&chunk[..n]);
        }
        body_raw.truncate(cl);
        Ok((headers_text, String::from_utf8_lossy(&body_raw).to_string()))
    } else {
        // 无 Content-Length 且非 chunked（如 202 空体）：返回已读到的部分
        Ok((headers_text, String::from_utf8_lossy(&body_raw).to_string()))
    }
}

/// 解码 HTTP/1.1 chunked transfer-encoding body。
fn decode_chunked(raw: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let mut i = 0usize;
    while i < raw.len() {
        // 读块大小行
        let line_end = match raw[i..].windows(2).position(|w| w == b"\r\n") {
            Some(p) => i + p,
            None => break,
        };
        let size_str = String::from_utf8_lossy(&raw[i..line_end]);
        let size_hex = size_str.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16).unwrap_or(0);
        i = line_end + 2; // 跳过 \r\n
        if size == 0 {
            break;
        }
        let end = (i + size).min(raw.len());
        out.extend_from_slice(&raw[i..end]);
        i = end + 2; // 跳过块尾 \r\n
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 在【持久 keep-alive 连接】上发一条 POST，读回恰好一条响应。
/// 断线时自动重连一次（连接可能被 server 空闲回收）。
fn http_post_keepalive(
    conn: &mut HttpConn,
    body: &str,
    timeout_secs: u64,
) -> Result<(String, String), String> {
    let (host, port, path) = parse_http_url(&conn.base_url)?;

    // 组装请求（keep-alive：不发 Connection: close）
    let mut req = String::new();
    req.push_str(&format!("POST {} HTTP/1.1\r\n", path));
    req.push_str(&format!("Host: {}:{}\r\n", host, port));
    req.push_str("Content-Type: application/json\r\n");
    req.push_str("Accept: application/json, text/event-stream\r\n");
    if !conn.session_id.is_empty() {
        req.push_str(&format!("Mcp-Session-Id: {}\r\n", conn.session_id));
    }
    req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    req.push_str("\r\n");
    let mut wire = req.into_bytes();
    wire.extend_from_slice(body.as_bytes());

    // 最多尝试两次：首次用现有连接，失败则重连后重试一次
    let mut last_err = String::new();
    for attempt in 0..2 {
        if conn.stream.is_none() {
            match TcpStream::connect((host.as_str(), port)) {
                Ok(s) => {
                    let t = Some(Duration::from_secs(timeout_secs));
                    let _ = s.set_read_timeout(t);
                    let _ = s.set_write_timeout(t);
                    conn.stream = Some(s);
                }
                Err(e) => {
                    last_err = format!("connect {}:{} 失败: {}", host, port, e);
                    continue;
                }
            }
        }

        let stream = conn.stream.as_mut().unwrap();
        // 每次刷新超时
        let t = Some(Duration::from_secs(timeout_secs));
        let _ = stream.set_read_timeout(t);
        let _ = stream.set_write_timeout(t);

        let write_ok = stream.write_all(&wire).and_then(|_| stream.flush()).is_ok();
        if !write_ok {
            // 连接坏了，丢弃后重连重试
            conn.stream = None;
            last_err = "写请求失败，重连重试".to_string();
            continue;
        }

        match read_one_http_response(stream) {
            Ok(r) => return Ok(r),
            Err(e) => {
                // 第一次失败：可能是 server 关闭了空闲连接 → 重连重试
                conn.stream = None;
                last_err = e;
                if attempt == 0 {
                    continue;
                }
            }
        }
    }
    Err(last_err)
}

fn http_extract_session(
    base_url: &str,
    body: &str,
) -> Result<(String, String, Option<TcpStream>), String> {
    let mut conn = HttpConn {
        base_url: base_url.to_string(),
        session_id: String::new(),
        stream: None,
    };
    let (headers, body_text) = http_post_keepalive(&mut conn, body, 30)?;
    let session_id = find_header_value(&headers, "Mcp-Session-Id").unwrap_or_default();
    // 复用这条已建立的连接给后续请求（保持会话存活的关键）
    Ok((session_id, body_text, conn.stream.take()))
}

/// 在持久连接上发 DELETE 关闭会话（忽略失败）。
fn http_delete_keepalive(conn: &mut HttpConn) -> Result<(), String> {
    let (host, port, path) = parse_http_url(&conn.base_url)?;
    if conn.stream.is_none() {
        let s = TcpStream::connect((host.as_str(), port))
            .map_err(|e| format!("connect 失败: {}", e))?;
        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
        conn.stream = Some(s);
    }
    let stream = conn.stream.as_mut().unwrap();
    let req = format!(
        "DELETE {} HTTP/1.1\r\nHost: {}:{}\r\nMcp-Session-Id: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        path, host, port, conn.session_id
    );
    let _ = stream.write_all(req.as_bytes());
    let _ = stream.flush();
    let _ = read_one_http_response(stream);
    conn.stream = None;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// stdio 入站消息路由（后台 reader 线程）
//
// 三类消息：
//   1) 有 method + 有 id → server→client REQUEST：派发处理器，把 JSON-RPC 响应写回 stdin。
//   2) 有 method 无 id    → notification：存入环形缓冲（供 取通知 FFI 排空），不破坏关联。
//   3) 无 method（有 id）  → response：按 id 存入关联 map（原行为）。
// ─────────────────────────────────────────────────────────────────────────────

fn route_inbound_stdio(
    v: Json,
    responses: &Arc<Mutex<HashMap<i64, Json>>>,
    handlers: &Arc<Mutex<ServerHandlers>>,
    child_state: &Arc<Mutex<StdioChild>>,
) {
    let has_method = v.get("method").and_then(|m| m.as_str()).is_some();
    let id_val = v.get("id").cloned();

    if has_method {
        if let Some(id) = id_val {
            if !id.is_null() {
                // server→client REQUEST
                handle_server_request(&v, id, handlers, child_state);
                return;
            }
        }
        // notification（method 但无 id / id 为 null）
        if let Ok(mut h) = handlers.lock() {
            if h.notifications.len() >= MAX_NOTIFICATIONS {
                h.notifications.pop_front();
            }
            h.notifications.push_back(v);
        }
        return;
    }

    // response（含 result 或 error）
    if let Some(id_num) = id_val.and_then(|i| i.as_i64()) {
        if let Ok(mut map) = responses.lock() {
            map.insert(id_num, v);
        }
    }
}

/// 派发一条 server→client 请求，构造 JSON-RPC 响应写回子进程 stdin。
fn handle_server_request(
    req: &Json,
    id: Json,
    handlers: &Arc<Mutex<ServerHandlers>>,
    child_state: &Arc<Mutex<StdioChild>>,
) {
    let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));

    let outcome: Result<Json, (i64, String)> = match method {
        "sampling/createMessage" => {
            let sampling = handlers.lock().ok().and_then(|h| h.sampling);
            match sampling {
                Some(addr) => match invoke_qi_closure(addr, &params.to_string()) {
                    Some(s) => match serde_json::from_str::<Json>(&s) {
                        Ok(result) => Ok(result),
                        // 处理器返回了非 JSON：包成文本结果而非让 server 挂起
                        Err(_) => Err((-32603, format!("采样处理器返回非法 JSON: {}", s))),
                    },
                    None => Err((-32603, "采样处理器调用失败".to_string())),
                },
                None => Err((
                    -32601,
                    "client 未注册采样处理器（无 sampling 能力）".to_string(),
                )),
            }
        }
        "roots/list" => {
            let roots = handlers
                .lock()
                .ok()
                .map(|h| h.roots.clone())
                .unwrap_or_else(|| json!([]));
            Ok(json!({ "roots": roots }))
        }
        "elicitation/create" => {
            let elicit = handlers.lock().ok().and_then(|h| h.elicitation);
            match elicit {
                Some(addr) => match invoke_qi_closure(addr, &params.to_string()) {
                    Some(s) => match serde_json::from_str::<Json>(&s) {
                        Ok(result) => Ok(result),
                        Err(_) => Ok(json!({ "action": "decline" })),
                    },
                    None => Ok(json!({ "action": "decline" })),
                },
                None => Ok(json!({ "action": "decline" })),
            }
        }
        _ => Err((-32601, format!("未知 server→client 方法: {}", method))),
    };

    let response = match outcome {
        Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        Err((code, msg)) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": msg }
        }),
    };

    // 写回子进程 stdin（持锁串行化，与请求写共用同一把锁）
    if let Ok(mut st) = child_state.lock() {
        let _ = writeln!(st.stdin, "{}", response);
        let _ = st.stdin.flush();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// stdio 等待 id 关联的响应
// ─────────────────────────────────────────────────────────────────────────────

fn stdio_wait_response(
    responses: Arc<Mutex<HashMap<i64, Json>>>,
    eof: Arc<AtomicBool>,
    id: i64,
    timeout_secs: u64,
) -> Option<Json> {
    let deadline = Instant::now() + Duration::from_secs(timeout_secs);
    loop {
        {
            let mut map = responses.lock().ok()?;
            if let Some(v) = map.remove(&id) {
                return Some(v);
            }
        }
        if eof.load(Ordering::SeqCst) {
            // 最后再检查一次
            let mut map = responses.lock().ok()?;
            return map.remove(&id);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(5));
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FFI：连接 stdio MCP server
// ─────────────────────────────────────────────────────────────────────────────

/// 启动 stdio MCP 子进程，完成 initialize 握手。
/// 成功返回 conn_id (>0)，失败返回 -1。
#[no_mangle]
pub extern "C" fn qi_mcpc_connect_stdio(cmd: *const c_char, args_json: *const c_char) -> i64 {
    if cmd.is_null() {
        return -1;
    }
    let cmd_str = unsafe { CStr::from_ptr(cmd).to_string_lossy().to_string() };
    let args: Vec<String> = if args_json.is_null() {
        Vec::new()
    } else {
        let s = unsafe { CStr::from_ptr(args_json).to_string_lossy() };
        serde_json::from_str::<Vec<String>>(&s).unwrap_or_default()
    };

    let mut child = match Command::new(&cmd_str)
        .args(&args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[qi-mcpc] spawn 失败: {}", e);
            return -1;
        }
    };

    let stdin = match child.stdin.take() {
        Some(s) => s,
        None => return -1,
    };
    let stdout = match child.stdout.take() {
        Some(s) => s,
        None => return -1,
    };

    // 后台读线程：持续读 stdout，路由消息
    let responses: Arc<Mutex<HashMap<i64, Json>>> = Arc::new(Mutex::new(HashMap::new()));
    let eof: Arc<AtomicBool> = Arc::new(AtomicBool::new(false));
    let handlers: Arc<Mutex<ServerHandlers>> = Arc::new(Mutex::new(ServerHandlers::default()));

    let child_state = Arc::new(Mutex::new(StdioChild {
        _child: child,
        stdin,
        responses: responses.clone(),
        eof: eof.clone(),
        handlers: handlers.clone(),
    }));

    let responses_clone = responses.clone();
    let eof_clone = eof.clone();
    let handlers_clone = handlers.clone();
    let child_state_for_reader = child_state.clone();

    thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line_result in reader.lines() {
            match line_result {
                Ok(line) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(v) = serde_json::from_str::<Json>(trimmed) {
                        route_inbound_stdio(
                            v,
                            &responses_clone,
                            &handlers_clone,
                            &child_state_for_reader,
                        );
                    }
                }
                Err(_) => break,
            }
        }
        eof_clone.store(true, Ordering::SeqCst);
    });

    let conn = Arc::new(Connection {
        transport: Transport::Stdio {
            child_state: child_state.clone(),
        },
        next_id: AtomicI64::new(1),
    });

    // initialize 握手
    let req_id = 1i64;
    let init_req = json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": client_capabilities(),
            "clientInfo": {"name": "qi-harness", "version": "0.1.0"}
        }
    });

    {
        let mut st = match child_state.lock() {
            Ok(g) => g,
            Err(_) => return -1,
        };
        if writeln!(st.stdin, "{}", init_req).is_err() {
            eprintln!("[qi-mcpc] initialize 写入失败");
            return -1;
        }
        if st.stdin.flush().is_err() {
            return -1;
        }
    }

    // 等待 initialize 响应（30s）
    let init_resp = stdio_wait_response(responses.clone(), eof.clone(), req_id, 30);
    if init_resp.is_none() {
        eprintln!("[qi-mcpc] initialize 超时或 EOF");
        return -1;
    }

    // 发送 notifications/initialized
    {
        let mut st = match child_state.lock() {
            Ok(g) => g,
            Err(_) => return -1,
        };
        let notif = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        let _ = writeln!(st.stdin, "{}", notif.to_string());
        let _ = st.stdin.flush();
    }

    // 注册连接并返回 conn_id
    let conn_id = next_conn_id();
    // 更新 next_id（初始化用了 id=1）
    conn.next_id.fetch_add(1, Ordering::SeqCst); // 下次从 2 开始
    conn_registry().lock().unwrap().insert(conn_id, conn);
    conn_id
}

// ─────────────────────────────────────────────────────────────────────────────
// FFI：连接 HTTP MCP server
// ─────────────────────────────────────────────────────────────────────────────

/// 连接 HTTP(Streamable) MCP server，完成 initialize 握手。
/// 成功返回 conn_id (>0)，失败返回 -1。
#[no_mangle]
pub extern "C" fn qi_mcpc_connect_http(base_url: *const c_char) -> i64 {
    if base_url.is_null() {
        return -1;
    }
    let url = unsafe { CStr::from_ptr(base_url).to_string_lossy().to_string() };

    let init_body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": client_capabilities(),
            "clientInfo": {"name": "qi-harness", "version": "0.1.0"}
        }
    })
    .to_string();

    let (session_id, init_body_text, stream) = match http_extract_session(&url, &init_body) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[qi-mcpc] HTTP initialize 失败: {}", e);
            return -1;
        }
    };

    if session_id.is_empty() {
        // 某些服务器不需要会话 id（但 Playwright MCP 需要）；也允许继续
        eprintln!("[qi-mcpc] HTTP initialize: 未返回 Mcp-Session-Id，继续（可能不需要会话）");
    }

    // 验证 initialize 响应包含 result
    let parsed_init = parse_sse_body(&init_body_text);
    if parsed_init.is_empty() {
        eprintln!("[qi-mcpc] HTTP initialize: 响应体为空");
        return -1;
    }

    // 复用 initialize 时建立的 keep-alive 连接，保持会话存活
    let http = Arc::new(Mutex::new(HttpConn {
        base_url: url,
        session_id,
        stream,
    }));

    // 发送 notifications/initialized（同一条连接；忽略 202/空体）
    {
        let notif = json!({"jsonrpc":"2.0","method":"notifications/initialized"}).to_string();
        if let Ok(mut c) = http.lock() {
            let _ = http_post_keepalive(&mut c, &notif, 10);
        }
    }

    let conn_id = next_conn_id();
    let conn = Arc::new(Connection {
        transport: Transport::Http { http },
        next_id: AtomicI64::new(2), // id=1 已用于 initialize
    });

    conn_registry().lock().unwrap().insert(conn_id, conn);
    conn_id
}

// ─────────────────────────────────────────────────────────────────────────────
// FFI：发送 MCP 请求并等待响应
// ─────────────────────────────────────────────────────────────────────────────

/// 发送 JSON-RPC 请求（method + params_json），等待对应 id 的响应。
/// 返回响应的 result 字段的 JSON 串（或 error 对象）。
/// 失败返回空 C 字符串（非 NULL）。
#[no_mangle]
pub extern "C" fn qi_mcpc_request(
    conn_id: i64,
    method: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    if method.is_null() || params_json.is_null() {
        return empty_cstr();
    }

    let method_str = unsafe { CStr::from_ptr(method).to_string_lossy().to_string() };
    let params_str = unsafe { CStr::from_ptr(params_json).to_string_lossy().to_string() };

    let params: Json = match serde_json::from_str(&params_str) {
        Ok(v) => v,
        Err(_) => json!({}),
    };

    let conn = match get_conn(conn_id) {
        Some(c) => c,
        None => {
            eprintln!("[qi-mcpc] 连接 {} 不存在", conn_id);
            return empty_cstr();
        }
    };

    let req_id = conn.next_id.fetch_add(1, Ordering::SeqCst);
    let request = json!({
        "jsonrpc": "2.0",
        "id": req_id,
        "method": method_str,
        "params": params
    });
    let request_str = request.to_string();

    match &conn.transport {
        Transport::Stdio { child_state } => {
            let (responses, eof) = {
                let st = match child_state.lock() {
                    Ok(g) => g,
                    Err(_) => return empty_cstr(),
                };
                (st.responses.clone(), st.eof.clone())
            };

            // 写请求
            {
                let mut st = match child_state.lock() {
                    Ok(g) => g,
                    Err(_) => return empty_cstr(),
                };
                if writeln!(st.stdin, "{}", request_str).is_err() || st.stdin.flush().is_err() {
                    return empty_cstr();
                }
            }

            // 等待响应（60s）
            match stdio_wait_response(responses, eof, req_id, 60) {
                // 返回完整 JSON-RPC 响应行（含 jsonrpc/id/result 或 error），
                // 与原纯 Qi 实现的 发送请求 返回值格式一致。
                Some(resp) => to_cstr(resp.to_string()),
                None => {
                    eprintln!("[qi-mcpc] stdio 响应超时 (id={})", req_id);
                    empty_cstr()
                }
            }
        }

        Transport::Http { http } => {
            // 在【持久 keep-alive 连接】上发请求并精确读回一条响应。
            // 复用连接是保持 Playwright 会话存活的关键（Connection: close 会杀会话）。
            let mut c = match http.lock() {
                Ok(g) => g,
                Err(_) => return empty_cstr(),
            };
            match http_post_keepalive(&mut c, &request_str, 60) {
                Ok((_headers, body)) => {
                    if body.is_empty() {
                        // 202 Accepted with empty body（通知响应）
                        return empty_cstr();
                    }
                    // 解析 SSE 或 JSON body，提取 data: 行
                    let data = parse_sse_body(&body);
                    if data.is_empty() {
                        return empty_cstr();
                    }
                    // 返回与原纯 Qi 实现一致的格式：完整 JSON-RPC 响应行
                    to_cstr(data)
                }
                Err(e) => {
                    eprintln!("[qi-mcpc] HTTP 请求失败 (id={}): {}", req_id, e);
                    empty_cstr()
                }
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FFI：关闭连接
// ─────────────────────────────────────────────────────────────────────────────

/// 关闭 MCP 连接。
/// stdio: 杀子进程。HTTP: 发 DELETE（可选，忽略失败）。
/// 成功返回 1，失败返回 0。返回 i64 与 Qi 整数类型对齐。
#[no_mangle]
pub extern "C" fn qi_mcpc_close(conn_id: i64) -> i64 {
    let conn = match conn_registry().lock().unwrap().remove(&conn_id) {
        Some(c) => c,
        None => return 0,
    };

    match &conn.transport {
        Transport::Stdio { child_state } => {
            if let Ok(mut st) = child_state.lock() {
                let _ = st._child.kill();
                let _ = st._child.wait();
            }
            1
        }
        Transport::Http { http } => {
            if let Ok(mut c) = http.lock() {
                if !c.session_id.is_empty() {
                    // 发 DELETE 关闭会话（忽略失败）
                    let _ = http_delete_keepalive(&mut c);
                }
            }
            1
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FFI：注册 server→client 处理器 / 配置（仅 stdio）
// ─────────────────────────────────────────────────────────────────────────────

/// 取连接的 stdio handlers（仅 stdio 传输有；HTTP 返回 None）。
fn stdio_handlers(conn_id: i64) -> Option<Arc<Mutex<ServerHandlers>>> {
    let conn = get_conn(conn_id)?;
    match &conn.transport {
        Transport::Stdio { child_state } => {
            let st = child_state.lock().ok()?;
            Some(st.handlers.clone())
        }
        Transport::Http { .. } => None,
    }
}

/// 注册 sampling/createMessage 处理器（Qi 闭包对象指针）。
///
/// closure_ptr 是 Qi 闭包对象地址：offset 0 为 trampoline
/// `extern "C" fn(env, params_json) -> result_json`。处理器收到 sampling 参数 JSON，
/// 返回 sampling 结果 JSON（如 `{"role":"assistant","content":{...},"model":...}`）。
///
/// 返回 0 成功，-1 失败（连接不存在 / 非 stdio）。
#[no_mangle]
pub extern "C" fn qi_mcpc_set_sampling_handler(
    conn_id: i64,
    closure_ptr: *const std::ffi::c_void,
) -> i32 {
    if closure_ptr.is_null() {
        return -1;
    }
    match stdio_handlers(conn_id) {
        Some(h) => {
            if let Ok(mut hh) = h.lock() {
                hh.sampling = Some(closure_ptr as usize);
                0
            } else {
                -1
            }
        }
        None => -1,
    }
}

/// 注册 elicitation/create 处理器（Qi 闭包对象指针）。返回 0/-1。
#[no_mangle]
pub extern "C" fn qi_mcpc_set_elicitation_handler(
    conn_id: i64,
    closure_ptr: *const std::ffi::c_void,
) -> i32 {
    if closure_ptr.is_null() {
        return -1;
    }
    match stdio_handlers(conn_id) {
        Some(h) => {
            if let Ok(mut hh) = h.lock() {
                hh.elicitation = Some(closure_ptr as usize);
                0
            } else {
                -1
            }
        }
        None => -1,
    }
}

/// 设置 roots/list 返回的 roots 数组（JSON 串）。返回 0/-1。
#[no_mangle]
pub extern "C" fn qi_mcpc_set_roots(conn_id: i64, roots_json: *const c_char) -> i32 {
    if roots_json.is_null() {
        return -1;
    }
    let s = unsafe { CStr::from_ptr(roots_json).to_string_lossy().to_string() };
    let parsed: Json = serde_json::from_str(&s).unwrap_or_else(|_| json!([]));
    match stdio_handlers(conn_id) {
        Some(h) => {
            if let Ok(mut hh) = h.lock() {
                hh.roots = parsed;
                0
            } else {
                -1
            }
        }
        None => -1,
    }
}

/// 排空缓冲的通知，返回 JSON 数组串（并清空）。无连接 / 无通知返回 `[]`。
#[no_mangle]
pub extern "C" fn qi_mcpc_drain_notifications(conn_id: i64) -> *mut c_char {
    match stdio_handlers(conn_id) {
        Some(h) => {
            if let Ok(mut hh) = h.lock() {
                let drained: Vec<Json> = hh.notifications.drain(..).collect();
                to_cstr(Json::Array(drained).to_string())
            } else {
                to_cstr("[]".to_string())
            }
        }
        None => to_cstr("[]".to_string()),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FFI：释放字符串
// ─────────────────────────────────────────────────────────────────────────────

/// 释放由 qi_mcpc_request 返回的字符串。
#[no_mangle]
pub extern "C" fn qi_mcpc_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// 单元测试
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_chunked() {
        // 5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n
        let raw = b"5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        assert_eq!(decode_chunked(raw), "hello world");
    }

    #[test]
    fn test_find_header_value_case_insensitive() {
        let headers =
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nMcp-Session-Id: abc123";
        assert_eq!(
            find_header_value(headers, "mcp-session-id").as_deref(),
            Some("abc123")
        );
        assert_eq!(find_header_value(headers, "Content-Length"), None);
    }

    #[test]
    fn test_parse_http_url() {
        assert_eq!(
            parse_http_url("http://localhost:43570/mcp").unwrap(),
            ("localhost".to_string(), 43570u16, "/mcp".to_string())
        );
        assert_eq!(
            parse_http_url("http://127.0.0.1:8/x/y").unwrap(),
            ("127.0.0.1".to_string(), 8u16, "/x/y".to_string())
        );
        assert!(parse_http_url("https://x/y").is_err());
    }

    #[test]
    fn test_parse_sse_body_direct_json() {
        // 直接 JSON 响应（application/json）
        let body = r#"{"jsonrpc":"2.0","id":1,"result":{"tools":[]}}"#;
        let out = parse_sse_body(body);
        assert!(out.contains("result"));
    }

    #[test]
    fn test_parse_sse_body_event_message() {
        // 标准 SSE 格式
        let body =
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"tools\":[]}}\n\n";
        let out = parse_sse_body(body);
        assert!(out.contains("tools"));
    }

    #[test]
    fn test_parse_sse_body_large_data() {
        // 大体：多行 SSE，result 在其中一行
        let big_value = "x".repeat(10000);
        let body = format!(
            "event: message\ndata: {{\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{{\"output\":\"{}\"}}}}\n\n",
            big_value
        );
        let out = parse_sse_body(&body);
        assert!(out.contains("result"));
        assert!(out.contains(&big_value));
    }

    #[test]
    fn test_parse_sse_body_with_notifications() {
        // SSE 流中先有通知再有响应
        let body = concat!(
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}\n\n",
            "event: message\n",
            "data: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"content\":[]}}\n\n",
        );
        let out = parse_sse_body(body);
        // 应该返回有 result 的那一条
        assert!(out.contains("result"));
        assert!(!out.contains("notifications/progress") || out.contains("result"));
    }

    // ── P4b：server→client 双向（sampling/createMessage）端到端验证 ──────────────
    //
    // 对官方参考 server `@modelcontextprotocol/server-everything` 验证：
    //   1) 客户端 initialize 时声明 sampling 能力（client_capabilities）；
    //   2) server 的 oninitialized 后才注册条件工具 `trigger-sampling-request`
    //      并发 tools/list_changed；
    //   3) 调用该工具 → server 回发 `sampling/createMessage` 请求；
    //   4) 后台 reader 派发到我们注册的【桩 sampling 处理器】，把结果写回 stdin；
    //   5) server 收到结果后完成 tool 调用，返回 "LLM sampling result: ..."。
    //
    // 运行：
    //   cargo test --lib mcp_client_ffi sampling_roundtrip -- --ignored --nocapture
    //
    // 桩处理器：Qi 闭包 ABI 模拟 —— 一个 offset-0 为 extern "C" fn 的小对象。
    extern "C" fn stub_sampling_trampoline(
        _env: *const std::ffi::c_void,
        params: *const c_char,
    ) -> *mut c_char {
        let p = unsafe { CStr::from_ptr(params).to_string_lossy().into_owned() };
        eprintln!("[stub-sampling] 收到 params 字节数={}", p.len());
        // 返回一条合法的 MCP CreateMessageResult
        let result = r#"{"role":"assistant","content":{"type":"text","text":"stubbed"},"model":"stub","stopReason":"endTurn"}"#;
        CString::new(result).unwrap().into_raw()
    }

    #[test]
    #[ignore]
    fn sampling_roundtrip() {
        // 构造一个「Qi 闭包对象」：offset 0 = trampoline 函数指针。
        // 用 Box<[*const c_void; 1]> 模拟闭包对象布局，进程生命期内泄漏（测试用）。
        let trampoline_ptr = stub_sampling_trampoline as *const std::ffi::c_void;
        let closure_obj: Box<[*const std::ffi::c_void; 1]> = Box::new([trampoline_ptr]);
        let closure_addr = Box::into_raw(closure_obj) as *const std::ffi::c_void;

        let cmd = CString::new("npx").unwrap();
        let args = CString::new(r#"["-y","@modelcontextprotocol/server-everything"]"#).unwrap();
        let conn = qi_mcpc_connect_stdio(cmd.as_ptr(), args.as_ptr());
        eprintln!("[test] conn={}", conn);
        assert!(conn > 0, "连接 server-everything 失败");

        // 注册桩 sampling 处理器
        let set = qi_mcpc_set_sampling_handler(conn, closure_addr);
        assert_eq!(set, 0, "注册 sampling 处理器失败");

        let method = CString::new("tools/call").unwrap();
        let call = |p: &str| -> String {
            let cp = CString::new(p).unwrap();
            let r = qi_mcpc_request(conn, method.as_ptr(), cp.as_ptr());
            unsafe { CStr::from_ptr(r).to_string_lossy().into_owned() }
        };

        // 给 server 的 oninitialized + 条件工具注册一点时间（tools/list_changed）
        std::thread::sleep(Duration::from_millis(800));

        // 确认 trigger-sampling-request 已注册（条件工具）
        let list_method = CString::new("tools/list").unwrap();
        let lp = CString::new("{}").unwrap();
        let list_raw = qi_mcpc_request(conn, list_method.as_ptr(), lp.as_ptr());
        let list = unsafe { CStr::from_ptr(list_raw).to_string_lossy().into_owned() };
        eprintln!(
            "[test] tools/list 含 trigger-sampling-request: {}",
            list.contains("trigger-sampling-request")
        );
        assert!(
            list.contains("trigger-sampling-request"),
            "server 未注册 sampling 工具（client 能力未被识别？）"
        );

        // 调用 sampling 触发工具 —— 这会让 server 回发 sampling/createMessage
        let res = call(
            r#"{"name":"trigger-sampling-request","arguments":{"prompt":"hello from qi","maxTokens":50}}"#,
        );
        eprintln!("[test] sampling 工具结果 len={}: {}", res.len(), res);

        // tool 必须完成并返回 result（证明双向往返成功）
        assert!(!res.is_empty(), "sampling 工具无响应（双向往返失败/挂起）");
        assert!(
            res.contains("result"),
            "sampling 工具返回了 error 而非 result: {}",
            res
        );
        // server 把我们桩返回的文本回显在 "LLM sampling result" 里
        assert!(
            res.contains("stubbed") || res.contains("LLM sampling result"),
            "结果未包含桩 sampling 内容: {}",
            res
        );

        // 排空通知（应至少有 tools/list_changed）
        let notifs_raw = qi_mcpc_drain_notifications(conn);
        let notifs = unsafe { CStr::from_ptr(notifs_raw).to_string_lossy().into_owned() };
        eprintln!("[test] 缓冲通知: {}", notifs);

        let _ = qi_mcpc_close(conn);
    }

    // 复现 HTTP 大 browser_evaluate（带转义引号/换行）失败。
    // 用法：先 `npx -y @playwright/mcp@latest --port 43560`，再
    // `QI_TEST_MCP_URL=http://localhost:43560/mcp cargo test debug_http_big_eval -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn debug_http_big_eval() {
        use std::ffi::{CStr, CString};
        let url = std::env::var("QI_TEST_MCP_URL").unwrap_or_default();
        if url.is_empty() {
            eprintln!("跳过：未设 QI_TEST_MCP_URL");
            return;
        }
        let cu = CString::new(url).unwrap();
        let conn = qi_mcpc_connect_http(cu.as_ptr());
        eprintln!("[dbg] conn={}", conn);
        assert!(conn > 0);
        let method = CString::new("tools/call").unwrap();
        let call = |p: &str| -> String {
            let cp = CString::new(p).unwrap();
            let r = qi_mcpc_request(conn, method.as_ptr(), cp.as_ptr());
            unsafe { CStr::from_ptr(r).to_string_lossy().into_owned() }
        };
        let nav = call(r#"{"name":"browser_navigate","arguments":{"url":"https://example.com/"}}"#);
        eprintln!(
            "[dbg] navigate len={} head={}",
            nav.len(),
            &nav[..nav.len().min(80)]
        );
        // 真实失败的那段大函数（~1.5KB），用 serde 正确转义构造 params
        let func = r##"() => {
  const title = document.title;
  const metaDesc = document.querySelector('meta[name="description"]')?.getAttribute('content') || '';
  const h1s = document.querySelectorAll('h1').length;
  const h2s = document.querySelectorAll('h2').length;
  const h3s = document.querySelectorAll('h3').length;
  const jsonlds = document.querySelectorAll('script[type="application/ld+json"]').length;
  const ogTitle = document.querySelector('meta[property="og:title"]');
  const ogDesc = document.querySelector('meta[property="og:description"]');
  const ogImage = document.querySelector('meta[property="og:image"]');
  const twitterCard = document.querySelector('meta[name="twitter:card"]');
  const twitterTitle = document.querySelector('meta[name="twitter:title"]');
  const twitterDesc = document.querySelector('meta[name="twitter:description"]');
  const viewport = document.querySelector('meta[name="viewport"]')?.getAttribute('content') || '';
  const imgs = document.querySelectorAll('img');
  let imgsWithAlt = 0;
  imgs.forEach(img => { if(img.getAttribute('alt') !== null && img.getAttribute('alt') !== '') imgsWithAlt++; });
  const imgAltCoverage = imgs.length > 0 ? Math.round((imgsWithAlt / imgs.length) * 100) : 100;
  const bodyText = document.body.innerText.replace(/\s+/g, ' ').trim();
  const wordCount = bodyText.split(' ').filter(w => w.length > 0).length;
  return JSON.stringify({ title, metaDescription: metaDesc, h1Count: h1s, h2Count: h2s, h3Count: h3s, jsonldCount: jsonlds, ogTitle: !!ogTitle, ogDesc: !!ogDesc, ogImage: !!ogImage, twitterCard: !!twitterCard, twitterTitle: !!twitterTitle, twitterDesc: !!twitterDesc, viewport, imgAltCoverage, wordCount });
}"##;
        let big = serde_json::json!({"name":"browser_evaluate","arguments":{"function": func}})
            .to_string();
        eprintln!("[dbg] big params bytes={}", big.len());
        let res = call(&big);
        eprintln!(
            "[dbg] BIG EVAL RESULT len={} : {}",
            res.len(),
            &res[..res.len().min(400)]
        );
        // 看是否还能用（session 是否被搞坏）
        let snap = call(r#"{"name":"browser_snapshot","arguments":{}}"#);
        eprintln!(
            "[dbg] snapshot-after len={} head={}",
            snap.len(),
            &snap[..snap.len().min(80)]
        );
    }
}
