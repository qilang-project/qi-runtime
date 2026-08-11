//! gRPC 客户端 FFI（HTTP/2 · 一元调用）
//!
//! 运行时原来只有 h2 **服务端**，客户端一行没有。这里补上。
//!
//! ── 一条连接、多条流 ────────────────────────────────────────────
//!
//! gRPC 的常态是「一个进程对一个后端只开一条 HTTP/2 连接，所有调用在上面
//! 多路复用」。所以 连接 是长期持有的句柄，调用 只是在它上面开一条流；
//! **不要每次调用都重连** —— 那样每次都要付 TCP + HTTP/2 握手的钱，
//! 还把连接数打上去。
//!
//! h2 的 `SendRequest` 可以 clone 且是 Send 的，正好一条连接发多路请求。
//! 驱动连接的那个 future 得一直有人 poll，所以后台 tokio 线程要活着 ——
//! 它一停，所有在途的调用直接断。
//!
//! ── 状态码可能在两个地方 ────────────────────────────────────────
//!
//! 正常情况：HEADERS(200) → DATA(消息) → TRAILERS(grpc-status)。
//! **但出错时服务端可以只发一个 HEADERS 就结束**（trailers-only 响应），
//! 这时 `grpc-status` 在**初始头**里而不是 trailers 里。只看 trailers 的
//! 客户端会把这种响应读成「没有状态」，然后报一个和真实原因毫不相干的错。
//! 两处都要看。

use bytes::Bytes;
use http::{HeaderMap, Request};
use rustls::{ClientConfig, RootCertStore};
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::stdlib::bytes_ffi::{clone_bytes, register_bytes};
use crate::stdlib::qi_str::rc_cstr_from_string;

/// 客户端连接句柄从 6_000_000 起，调用结果句柄从 6_500_000 起 ——
/// 跟服务端(5_000_000/5_500_000)、描述符(4_000_000) 错开。
static NEXT_CONN_ID: AtomicI64 = AtomicI64::new(6_000_000);
static NEXT_RESULT_ID: AtomicI64 = AtomicI64::new(6_500_000);

static CONNS: OnceLock<Mutex<HashMap<i64, Arc<ClientConn>>>> = OnceLock::new();
static RESULTS: OnceLock<Mutex<HashMap<i64, CallResult>>> = OnceLock::new();

/// 所有客户端共用一个 tokio 运行时。每条连接单起一个的话，几十个后端就是
/// 几十套线程池 —— 而这些线程绝大部分时间在睡觉。
static CLIENT_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

struct ClientConn {
    sender: h2::client::SendRequest<Bytes>,
    authority: String,
    /// URL 的 scheme。TLS 连接要写 `https`，写错了有的服务端会拒 ——
    /// :scheme 是 HTTP/2 的伪头，不是可有可无的装饰。
    scheme: &'static str,
}

struct CallResult {
    status: i64,
    message: String,
    bytes_handle: i64,
}

const STATUS_OK: i64 = 0;
const STATUS_UNKNOWN: i64 = 2;
const STATUS_DEADLINE_EXCEEDED: i64 = 4;
const STATUS_UNAVAILABLE: i64 = 14;
const STATUS_INTERNAL: i64 = 13;

fn client_runtime() -> Option<&'static tokio::runtime::Runtime> {
    if let Some(rt) = CLIENT_RUNTIME.get() {
        return Some(rt);
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .ok()?;
    Some(CLIENT_RUNTIME.get_or_init(|| rt))
}

fn conns() -> &'static Mutex<HashMap<i64, Arc<ClientConn>>> {
    CONNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn results() -> &'static Mutex<HashMap<i64, CallResult>> {
    RESULTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn read_cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p).to_string_lossy().to_string() }
}

fn out_str(s: String) -> *mut c_char {
    rc_cstr_from_string(s)
}

/// 小于这个字节数就不压 —— gzip 光头部 18 字节，压小消息是负收益。
const GZIP_MIN_BYTES: usize = 512;

fn frame_message(msg: &[u8], gzip: bool) -> (Vec<u8>, bool) {
    let compressed_body = if gzip { gzip_encode(msg) } else { None };
    let (body, flag): (&[u8], u8) = match compressed_body.as_deref() {
        Some(z) => (z, 1),
        None => (msg, 0),
    };
    let mut out = Vec::with_capacity(5 + body.len());
    out.push(flag);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    (out, flag == 1)
}

fn gzip_encode(raw: &[u8]) -> Option<Vec<u8>> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(raw).ok()?;
    enc.finish().ok()
}

fn gzip_decode(raw: &[u8]) -> Option<Vec<u8>> {
    use flate2::read::GzDecoder;
    use std::io::Read;
    let mut out = Vec::new();
    GzDecoder::new(raw).read_to_end(&mut out).ok()?;
    Some(out)
}

fn take_one_message(buf: &[u8]) -> Option<(bool, Vec<u8>)> {
    if buf.len() < 5 {
        return None;
    }
    let compressed = buf[0] != 0;
    let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    if buf.len() < 5 + len {
        return None;
    }
    Some((compressed, buf[5..5 + len].to_vec()))
}

/// grpc-message 的反转义（服务端按 percent-encoding 编过）。
fn percent_decode(text: &str) -> String {
    let raw = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(raw.len());
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'%' && i + 2 < raw.len() {
            let hex = std::str::from_utf8(&raw[i + 1..i + 3]).unwrap_or("");
            if let Ok(b) = u8::from_str_radix(hex, 16) {
                out.push(b);
                i += 3;
                continue;
            }
        }
        out.push(raw[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// 从一组头里读 grpc-status / grpc-message。没有 grpc-status 返回 None。
fn read_status(headers: &HeaderMap) -> Option<(i64, String)> {
    let status = headers.get("grpc-status")?;
    let code = status.to_str().ok()?.parse::<i64>().ok()?;
    let message = headers
        .get("grpc-message")
        .and_then(|v| v.to_str().ok())
        .map(percent_decode)
        .unwrap_or_default();
    Some((code, message))
}

/// 连一个 gRPC 后端（明文 h2c），返回连接句柄；失败 -1。
///
/// `target` 形如 `127.0.0.1:47813`。带 scheme 的写法（`http://…`）也认。
#[no_mangle]
pub extern "C" fn qi_grpc_dial(target: *const c_char) -> i64 {
    dial_inner(read_cstr(target), None)
}

/// 连一个 TLS 的 gRPC 后端。`ca_path` 给自签 CA 的 PEM 路径；留空走系统
/// 内置根证书（webpki-roots）。
///
/// **不提供「跳过证书校验」的开关**：那种开关一旦存在，就一定会有人为了让
/// 联调过去而打开它，然后一路带到线上。自签证书就把 CA 传进来。
#[no_mangle]
pub extern "C" fn qi_grpc_dial_tls(target: *const c_char, ca_path: *const c_char) -> i64 {
    dial_inner(read_cstr(target), Some(read_cstr(ca_path)))
}

fn build_client_config(ca_path: &str) -> Result<ClientConfig, String> {
    // 进程里只装一次默认加密后端；装两次会 panic
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });

    let mut roots = RootCertStore::empty();
    if ca_path.is_empty() {
        roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    } else {
        let file =
            std::fs::File::open(ca_path).map_err(|e| format!("打开 CA {} 失败: {}", ca_path, e))?;
        let mut reader = std::io::BufReader::new(file);
        for cert in rustls_pemfile::certs(&mut reader) {
            let cert = cert.map_err(|e| format!("解析 CA {} 失败: {}", ca_path, e))?;
            roots
                .add(cert)
                .map_err(|e| format!("CA {} 不可用: {}", ca_path, e))?;
        }
    }

    let mut config = ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    // ALPN 只报 h2 —— gRPC 只跑 HTTP/2
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(config)
}

fn dial_inner(raw: String, tls_ca: Option<String>) -> i64 {
    if raw.is_empty() {
        return -1;
    }
    let tls = tls_ca.is_some() || raw.starts_with("https://") || raw.starts_with("grpcs://");
    let authority = raw
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("grpcs://")
        .trim_start_matches("grpc://")
        .trim_end_matches('/')
        .to_string();

    let Some(rt) = client_runtime() else {
        return -1;
    };

    let host_only = authority
        .rsplit_once(':')
        .map(|(h, _)| h.to_string())
        .unwrap_or_else(|| authority.clone());
    let ca = tls_ca.unwrap_or_default();

    let result = rt.block_on(async {
        let tcp = tokio::net::TcpStream::connect(&authority).await?;
        // Nagle 会把小的 gRPC 帧攒起来等，一元调用的延迟直接翻倍
        let _ = tcp.set_nodelay(true);

        // 连接 future 必须一直有人 poll，否则这条连接上什么都动不了
        let sender = if tls {
            let config = build_client_config(&ca)
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { e.into() })?;
            let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
            let name = rustls::pki_types::ServerName::try_from(host_only.clone()).map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("主机名 {} 不合法: {}", host_only, e).into()
                },
            )?;
            let stream = connector.connect(name, tcp).await?;
            let (sender, connection) = h2::client::handshake(stream).await?;
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    eprintln!("[qi-grpc 客户端] 连接结束: {}", e);
                }
            });
            sender
        } else {
            let (sender, connection) = h2::client::handshake(tcp).await?;
            tokio::spawn(async move {
                if let Err(e) = connection.await {
                    eprintln!("[qi-grpc 客户端] 连接结束: {}", e);
                }
            });
            sender
        };
        Ok::<_, Box<dyn std::error::Error + Send + Sync>>(sender)
    });

    let sender = match result {
        Ok(s) => s,
        Err(e) => {
            eprintln!("[qi-grpc 客户端] 连 {} 失败: {}", authority, e);
            return -1;
        }
    };

    let id = NEXT_CONN_ID.fetch_add(1, Ordering::SeqCst);
    conns().lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        Arc::new(ClientConn {
            sender,
            authority,
            scheme: if tls { "https" } else { "http" },
        }),
    );
    id
}

/// 关连接。在途的调用会被断掉。
#[no_mangle]
pub extern "C" fn qi_grpc_close_conn(conn_id: i64) -> i64 {
    match conns()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&conn_id)
    {
        Some(_) => 0,
        None => -1,
    }
}

/// 发一次一元调用，阻塞到有结果。返回结果句柄；连接无效返回 -1。
///
/// **网络层的失败也走结果句柄**（状态码 UNAVAILABLE 之类），不是返回 -1 ——
/// 这样调用方只有一条错误路径要处理，而不是「-1 表示一类错、状态码表示另一类」。
#[no_mangle]
pub extern "C" fn qi_grpc_call(
    conn_id: i64,
    method: *const c_char,
    request_bytes: i64,
    timeout_ms: i64,
) -> i64 {
    let Some(conn) = conns()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&conn_id)
        .cloned()
    else {
        return -1;
    };
    let Some(rt) = client_runtime() else {
        return -1;
    };

    let method_name = read_cstr(method);
    let payload = clone_bytes(request_bytes).unwrap_or_default();
    let sender = conn.sender.clone();
    let authority = conn.authority.clone();
    let scheme = conn.scheme;
    let wait = Duration::from_millis(if timeout_ms <= 0 {
        30_000
    } else {
        timeout_ms as u64
    });

    let outcome = rt.block_on(async move {
        let fut = unary_call(sender, scheme, &authority, &method_name, payload);
        match tokio::time::timeout(wait, fut).await {
            Ok(r) => r,
            Err(_) => (
                STATUS_DEADLINE_EXCEEDED,
                format!("等 {} 毫秒还没回来", wait.as_millis()),
                Vec::new(),
            ),
        }
    });

    let (status, message, body) = outcome;
    let bytes_handle = if body.is_empty() {
        0
    } else {
        register_bytes(body)
    };
    let id = NEXT_RESULT_ID.fetch_add(1, Ordering::SeqCst);
    results().lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        CallResult {
            status,
            message,
            bytes_handle,
        },
    );
    id
}

async fn unary_call(
    sender: h2::client::SendRequest<Bytes>,
    scheme: &str,
    authority: &str,
    method: &str,
    payload: Vec<u8>,
) -> (i64, String, Vec<u8>) {
    let uri = format!(
        "{}://{}/{}",
        scheme,
        authority,
        method.trim_start_matches('/')
    );
    // 大请求压着发；无论压不压，都声明我们**收**得下 gzip，
    // 这样服务端可以压着回（两个头是不同方向的）。
    let (framed, did_gzip) = frame_message(&payload, payload.len() >= GZIP_MIN_BYTES);
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/grpc")
        .header("te", "trailers") // 规范要求，少了有的实现会拒
        .header("grpc-accept-encoding", "identity,gzip")
        .header("user-agent", "qi-grpc");
    if did_gzip {
        builder = builder.header("grpc-encoding", "gzip");
    }
    let request = match builder.body(()) {
        Ok(r) => r,
        Err(e) => return (STATUS_INTERNAL, format!("请求组不出来: {}", e), Vec::new()),
    };

    // ready() 吃掉 self 再还回来（h2 的所有权风格），所以这里收的是值不是引用
    let mut ready = match sender.ready().await {
        Ok(s) => s,
        Err(e) => return (STATUS_UNAVAILABLE, format!("连接不可用: {}", e), Vec::new()),
    };
    let (response, mut send_stream) = match ready.send_request(request, false) {
        Ok(p) => p,
        Err(e) => return (STATUS_UNAVAILABLE, format!("开流失败: {}", e), Vec::new()),
    };
    if let Err(e) = send_stream.send_data(Bytes::from(framed), true) {
        return (STATUS_UNAVAILABLE, format!("发请求失败: {}", e), Vec::new());
    }

    let response = match response.await {
        Ok(r) => r,
        Err(e) => {
            // h2 层的 RST_STREAM 也可能带着 grpc-status
            return (STATUS_UNAVAILABLE, format!("没等到响应: {}", e), Vec::new());
        }
    };

    // trailers-only 响应：状态在初始头里（见文件顶部）
    let head_status = read_status(response.headers());
    let response_gzip = response
        .headers()
        .get("grpc-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim() == "gzip")
        .unwrap_or(false);
    let mut body = response.into_body();

    let mut buf = Vec::new();
    while let Some(chunk) = body.data().await {
        match chunk {
            Ok(data) => {
                let _ = body.flow_control().release_capacity(data.len());
                buf.extend_from_slice(&data);
            }
            Err(e) => {
                if let Some((code, msg)) = head_status {
                    return (code, msg, Vec::new());
                }
                return (STATUS_UNAVAILABLE, format!("读响应失败: {}", e), Vec::new());
            }
        }
    }

    let trailer_status = match body.trailers().await {
        Ok(Some(t)) => read_status(&t),
        _ => None,
    };

    let (status, message) = trailer_status
        .or(head_status)
        // 既没有 trailers 也没有头里的状态 —— 对面不是个正经 gRPC 服务端
        .unwrap_or((STATUS_UNKNOWN, "对面没给 grpc-status".to_string()));

    if status != STATUS_OK {
        return (status, message, Vec::new());
    }
    match take_one_message(&buf) {
        Some((true, raw)) => {
            if !response_gzip {
                // 标志位说压了，头却没说用什么压的
                return (
                    STATUS_INTERNAL,
                    "对面回了压缩消息但没说压缩方式".to_string(),
                    Vec::new(),
                );
            }
            match gzip_decode(&raw) {
                Some(plain) => (STATUS_OK, String::new(), plain),
                None => (STATUS_INTERNAL, "gzip 解不开".to_string(), Vec::new()),
            }
        }
        Some((false, msg)) => (STATUS_OK, String::new(), msg),
        None => {
            if buf.is_empty() {
                (STATUS_OK, String::new(), Vec::new()) // 空消息合法
            } else {
                (STATUS_INTERNAL, "响应分帧不完整".to_string(), Vec::new())
            }
        }
    }
}

/// 结果的状态码：0 = 成功。
#[no_mangle]
pub extern "C" fn qi_grpc_result_status(result_id: i64) -> i64 {
    match results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&result_id)
    {
        Some(r) => r.status,
        None => STATUS_INTERNAL,
    }
}

/// 结果的错误说明；成功时是空串。
#[no_mangle]
pub extern "C" fn qi_grpc_result_message(result_id: i64) -> *mut c_char {
    match results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&result_id)
    {
        Some(r) => out_str(r.message.clone()),
        None => out_str("结果句柄无效".to_string()),
    }
}

/// 响应消息的字节切片句柄；没有响应体时是 0。
///
/// 这个句柄的释放跟着 结果释放 一起走，别单独释放它 ——
/// 单独释放之后再 结果释放 就是二次释放。
#[no_mangle]
pub extern "C" fn qi_grpc_result_bytes(result_id: i64) -> i64 {
    match results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&result_id)
    {
        Some(r) => r.bytes_handle,
        None => 0,
    }
}

/// 释放结果（连同它的响应字节）。
#[no_mangle]
pub extern "C" fn qi_grpc_result_free(result_id: i64) -> i64 {
    match results()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&result_id)
    {
        Some(r) => {
            if r.bytes_handle != 0 {
                crate::stdlib::bytes_ffi::free_bytes(r.bytes_handle);
            }
            0
        }
        None => -1,
    }
}
