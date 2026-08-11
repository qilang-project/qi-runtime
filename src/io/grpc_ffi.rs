//! gRPC 服务端 FFI（HTTP/2 · 一元调用）
//!
//! ── 为什么是「拉取式」而不是回调 ────────────────────────────────
//!
//! 第一版把 qi 的处理函数当函数指针传下来，运行时收到请求就回调过去。
//! **那条路是死的**：qi 把函数当值使用时会包一层闭包对象，FFI 参数拿到的是
//! 那个对象的栈地址，不是裸代码地址（实测收到 0x7b……，真身在 0x1000018b0），
//! transmute 过去调用就是 SIGBUS。h2_ffi.rs 里那个回调也是同样的写法，
//! 所以它同样调不通 —— 只是至今没有项目走过 运行应用_HTTP2 那条路。
//!
//! 现在换成：运行时只管协议（HTTP/2 分帧、HPACK、trailers），把解好的调用
//! 排进队列；qi 侧自己循环「接收调用 → 处理 → 回复」。这也正是 qi-web 已经在
//! 生产上跑的形状（accept 循环在 qi 里），额外的好处是**并发由 qi 决定**。
//!
//! ── gRPC 在线上到底长什么样 ─────────────────────────────────────
//!
//!   POST /包名.服务名/方法名                HTTP/2，路径就是方法全名
//!   content-type: application/grpc
//!   DATA: [1 字节压缩标志][4 字节大端长度][protobuf 字节]  ← 可以有多帧
//!   HEADERS(END_STREAM): grpc-status: 0     ← **状态在 trailers 里**
//!                        grpc-message: ...
//!
//! 最容易漏的是最后一条：**HTTP 状态码永远 200**，成败在 trailer 的
//! `grpc-status`。不发 trailers 的话客户端会一直等到超时，报一句
//! 「server closed the stream without sending trailers」，而服务端日志
//! 干干净净 —— 所以下面每条退出路径都保证发 trailers。
//!
//! ── 明文 h2c 是默认 ─────────────────────────────────────────────
//!
//! gRPC 大量跑在内网、跑在 sidecar 后面，那些场景就是明文 HTTP/2。
//! 浏览器不支持 h2c 无所谓 —— 浏览器本来也不能直接说 gRPC。

use bytes::Bytes;
use h2::server;
use http::{HeaderMap, Response};
use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

use crate::stdlib::bytes_ffi::{clone_bytes, register_bytes};
use crate::stdlib::qi_str::rc_cstr_from_string;

/// 服务器句柄从 5_000_000 起，调用句柄从 5_500_000 起 —— 跟
/// 描述符(4_000_000)、Redis(3_000_000)、TCP(1_000_000/2_000_000) 错开。
static NEXT_SERVER_ID: AtomicI64 = AtomicI64::new(5_000_000);
static NEXT_CALL_ID: AtomicI64 = AtomicI64::new(5_500_000);

static SERVERS: OnceLock<Mutex<HashMap<i64, Arc<Server>>>> = OnceLock::new();
static PENDING_CALLS: OnceLock<Mutex<HashMap<i64, PendingCall>>> = OnceLock::new();

/// 队列上限。堆到这儿说明 qi 侧的循环跟不上，新来的直接回 RESOURCE_EXHAUSTED ——
/// 比无限堆积然后整个进程 OOM 强，客户端也能立刻知道该退避。
const QUEUE_LIMIT: usize = 1024;

struct Server {
    queue: Mutex<VecDeque<i64>>,
    ready: Condvar,
    stopping: AtomicBool,
    /// 开了反射才有：反射要把描述符原样发回给客户端。
    reflection_pool: Mutex<Option<prost_reflect::DescriptorPool>>,
}

/// 一条已经排队、还没被回复的调用。回复时通过 oneshot 送回协议层。
struct PendingCall {
    reply: tokio::sync::oneshot::Sender<(i64, String, Vec<u8>)>,
    method: String,
    request_handle: i64,
}

fn servers() -> &'static Mutex<HashMap<i64, Arc<Server>>> {
    SERVERS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pending_calls() -> &'static Mutex<HashMap<i64, PendingCall>> {
    PENDING_CALLS.get_or_init(|| Mutex::new(HashMap::new()))
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

// ── gRPC 消息分帧 ───────────────────────────────────────────────
//
// 每条消息前 5 个字节：1 字节压缩标志 + 4 字节大端长度。
// 一个 DATA 帧里可以有多条消息，一条消息也能跨多个 DATA 帧 ——
// 所以不能假设「一帧一消息」，得边收边试着拆。

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

fn frame_message(msg: &[u8]) -> Vec<u8> {
    frame_message_maybe_gzip(msg, false)
}

/// 加分帧，可选 gzip。压缩标志位那一字节说的就是「这条消息压了没有」——
/// 它是**逐条消息**的，不是整条流的属性。
fn frame_message_maybe_gzip(msg: &[u8], gzip: bool) -> Vec<u8> {
    let body: std::borrow::Cow<[u8]> = if gzip {
        match gzip_encode(msg) {
            Some(z) => std::borrow::Cow::Owned(z),
            // 压不动就发原文。压缩失败不该让整个调用失败
            None => std::borrow::Cow::Borrowed(msg),
        }
    } else {
        std::borrow::Cow::Borrowed(msg)
    };
    let compressed = gzip && body.len() != msg.len();
    let mut out = Vec::with_capacity(5 + body.len());
    out.push(if compressed { 1u8 } else { 0u8 });
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out
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

const STATUS_OK: i64 = 0;
const STATUS_UNIMPLEMENTED: i64 = 12;
const STATUS_INTERNAL: i64 = 13;
const STATUS_RESOURCE_EXHAUSTED: i64 = 8;

/// 小于这个字节数就不压了 —— gzip 光头部就 18 字节，压小消息是负收益。
const GZIP_MIN_BYTES: usize = 512;

/// 起监听（明文 h2c），**立刻返回**服务器句柄；失败 -1。
///
/// 端口占用这类错误在这一步就同步报出来 —— 后台线程里发现再打日志的话，
/// qi 侧会以为起好了，然后卡在「接收调用」上等一个永远不来的请求。
#[no_mangle]
pub extern "C" fn qi_grpc_listen(host: *const c_char, port: i64) -> i64 {
    listen_inner(host, port, None)
}

/// 起 TLS 监听（HTTP/2 over TLS，ALPN 只报 h2）。证书和私钥是 PEM 路径。
///
/// ALPN **只报 h2**、不给 http/1.1 兜底：gRPC 只跑在 HTTP/2 上，留个
/// http/1.1 的口子只会让配错的客户端拿到一个更难懂的错误（协商成 1.1
/// 之后所有 gRPC 帧都没人认），不如让它在 ALPN 阶段就失败。
#[no_mangle]
pub extern "C" fn qi_grpc_listen_tls(
    host: *const c_char,
    port: i64,
    cert_path: *const c_char,
    key_path: *const c_char,
) -> i64 {
    let cert = read_cstr(cert_path);
    let key = read_cstr(key_path);
    if cert.is_empty() || key.is_empty() {
        eprintln!("[qi-grpc] TLS 监听要证书和私钥路径");
        return -1;
    }
    let acceptor = match build_acceptor(&cert, &key) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("[qi-grpc] TLS 配置: {}", e);
            return -1;
        }
    };
    listen_inner(host, port, Some(acceptor))
}

fn load_certs(path: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开证书 {} 失败: {}", path, e))?;
    let mut reader = std::io::BufReader::new(file);
    let certs: Result<Vec<_>, _> = rustls_pemfile::certs(&mut reader).collect();
    let certs = certs.map_err(|e| format!("解析证书 {} 失败: {}", path, e))?;
    if certs.is_empty() {
        return Err(format!("{} 不包含证书", path));
    }
    Ok(certs)
}

fn load_private_key(path: &str) -> Result<PrivateKeyDer<'static>, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("打开私钥 {} 失败: {}", path, e))?;
    let mut reader = std::io::BufReader::new(file);
    let key = rustls_pemfile::private_key(&mut reader)
        .map_err(|e| format!("解析私钥 {} 失败: {}", path, e))?
        .ok_or_else(|| format!("{} 不包含可用私钥", path))?;
    Ok(key)
}

fn build_acceptor(cert_path: &str, key_path: &str) -> Result<TlsAcceptor, String> {
    // 进程里只装一次默认加密后端；装两次会 panic
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });

    let certs = load_certs(cert_path)?;
    let key = load_private_key(key_path)?;
    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("ServerConfig: {}", e))?;
    config.alpn_protocols = vec![b"h2".to_vec()];
    Ok(TlsAcceptor::from(Arc::new(config)))
}

fn listen_inner(host: *const c_char, port: i64, acceptor: Option<TlsAcceptor>) -> i64 {
    let host = read_cstr(host);
    let host = if host.is_empty() {
        "127.0.0.1".to_string()
    } else {
        host
    };

    let server = Arc::new(Server {
        queue: Mutex::new(VecDeque::new()),
        ready: Condvar::new(),
        stopping: AtomicBool::new(false),
        reflection_pool: Mutex::new(None),
    });

    // 先同步绑定，绑上了才起线程
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(_) => return -1,
    };
    let listener = match runtime
        .block_on(async { tokio::net::TcpListener::bind((host.as_str(), port as u16)).await })
    {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[qi-grpc] bind {}:{} 失败: {}", host, port, e);
            return -1;
        }
    };

    let for_thread = server.clone();
    std::thread::spawn(move || {
        runtime.block_on(accept_loop(listener, for_thread, acceptor));
    });

    let id = NEXT_SERVER_ID.fetch_add(1, Ordering::SeqCst);
    servers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, server);
    id
}

async fn accept_loop(
    listener: tokio::net::TcpListener,
    server: Arc<Server>,
    acceptor: Option<TlsAcceptor>,
) {
    loop {
        if server.stopping.load(Ordering::SeqCst) {
            return;
        }
        let (sock, _) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[qi-grpc] accept: {}", e);
                continue;
            }
        };
        let server = server.clone();
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            match acceptor {
                Some(a) => match a.accept(sock).await {
                    Ok(tls) => serve_h2(tls, server).await,
                    Err(e) => eprintln!("[qi-grpc] TLS 握手: {}", e),
                },
                // 明文：直接进 HTTP/2（h2c 的 prior-knowledge 模式）。
                // gRPC 客户端连明文端口时就是这么干的，不走 HTTP/1.1 Upgrade。
                None => serve_h2(sock, server).await,
            }
        });
    }
}

async fn serve_h2<IO>(io: IO, server: Arc<Server>)
where
    IO: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let mut conn = match server::handshake(io).await {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[qi-grpc] h2 握手: {}", e);
            return;
        }
    };
    while let Some(result) = conn.accept().await {
        match result {
            Ok((req, respond)) => {
                tokio::spawn(serve_stream(req, respond, server.clone()));
            }
            Err(e) => {
                eprintln!("[qi-grpc] 流: {}", e);
                break;
            }
        }
    }
}

async fn serve_stream(
    req: http::Request<h2::RecvStream>,
    mut respond: server::SendResponse<Bytes>,
    server: Arc<Server>,
) {
    let (head, mut body) = req.into_parts();
    // 路径就是方法全名：/greet.Greeter/SayHello → greet.Greeter/SayHello
    let method = head.uri.path().trim_start_matches('/').to_string();

    // 反射由运行时**就地答复**，不进 qi 的队列 —— 它要的东西全在描述符池里，
    // 业务代码插不上手，交给 qi 只会逼着 qi 侧先有流式 API 才能开反射。
    if crate::io::grpc_reflection::is_reflection_method(&method) {
        let pool = server
            .reflection_pool
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        match pool {
            Some(p) => crate::io::grpc_reflection::serve(body, respond, p).await,
            None => send_status_only(
                &mut respond,
                STATUS_UNIMPLEMENTED,
                "本服务没开反射（调 开反射 之后再起）",
            ),
        }
        return;
    }

    // 对面用什么压的（grpc-encoding），以及它能收什么（grpc-accept-encoding）。
    // 两个头是**不同方向**的：前者说请求，后者说它希望响应怎么压。
    let request_encoding = head
        .headers
        .get("grpc-encoding")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("identity")
        .to_string();
    let client_accepts_gzip = head
        .headers
        .get("grpc-accept-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').any(|one| one.trim() == "gzip"))
        .unwrap_or(false);

    // **凑齐一条完整消息就走，不等 END_STREAM。**
    //
    // 一元调用里客户端发完就半关流，两种写法等价；但客户端流/双向流的客户端
    // （Go 的 stream.Send()）发完**不关流**，等 END_STREAM 就是永远等下去 ——
    // 于是对面拿到的不是「本服务没实现流式」而是一个挂死，直到它自己超时。
    // 实测：grpcurl 调双向流能立刻拿到 UNIMPLEMENTED（它发完就半关），
    // Go 客户端却挂满 10 秒 —— 同一个 bug 两种表现，光用 grpcurl 测发现不了。
    let mut buf = Vec::new();
    let mut message: Option<Vec<u8>> = None;
    while let Some(chunk) = body.data().await {
        match chunk {
            Ok(data) => {
                let _ = body.flow_control().release_capacity(data.len());
                buf.extend_from_slice(&data);
                match take_one_message(&buf) {
                    Some((true, raw)) => {
                        if request_encoding != "gzip" {
                            // 标志位说压了，头却没说用什么压的 —— 无从下手
                            send_status_only(
                                &mut respond,
                                STATUS_UNIMPLEMENTED,
                                &format!("不认识的压缩方式: {}", request_encoding),
                            );
                            return;
                        }
                        match gzip_decode(&raw) {
                            Some(plain) => {
                                message = Some(plain);
                                break;
                            }
                            None => {
                                send_status_only(&mut respond, STATUS_INTERNAL, "gzip 解不开");
                                return;
                            }
                        }
                    }
                    Some((false, msg)) => {
                        message = Some(msg);
                        break;
                    }
                    None => continue,
                }
            }
            Err(e) => {
                eprintln!("[qi-grpc] 读请求体: {}", e);
                send_status_only(&mut respond, STATUS_INTERNAL, "读请求体失败");
                return;
            }
        }
    }

    let request_msg = match message {
        Some(m) => m,
        None => {
            if buf.is_empty() {
                Vec::new() // 空消息是合法的（所有字段都是默认值）
            } else {
                send_status_only(&mut respond, STATUS_INTERNAL, "请求分帧不完整");
                return;
            }
        }
    };

    // 排进队列，等 qi 侧回复
    let (tx, rx) = tokio::sync::oneshot::channel();
    let call_id = NEXT_CALL_ID.fetch_add(1, Ordering::SeqCst);
    let request_handle = register_bytes(request_msg);
    {
        let mut queue = server.queue.lock().unwrap_or_else(|e| e.into_inner());
        if queue.len() >= QUEUE_LIMIT {
            drop(queue);
            send_status_only(
                &mut respond,
                STATUS_RESOURCE_EXHAUSTED,
                "服务端排队已满，稍后再试",
            );
            return;
        }
        pending_calls()
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(
                call_id,
                PendingCall {
                    reply: tx,
                    method,
                    request_handle,
                },
            );
        // 队列里只放句柄 —— 方法名和请求字节挂在 PendingCall 那一份上，
        // 存两份迟早对不上
        queue.push_back(call_id);
    }
    server.ready.notify_one();

    let (status, message_text, response_bytes) = match rx.await {
        Ok(triple) => triple,
        // qi 侧把这条调用丢了（进程要退出、或者 handler 没回复）
        Err(_) => (
            STATUS_INTERNAL,
            "服务端没有回复这次调用".to_string(),
            Vec::new(),
        ),
    };

    // **HTTP 状态永远 200**，成败看 trailer
    // 小消息压了反而更大（gzip 头就 18 字节），所以设个门槛。
    // 门槛之下发原文，压缩标志位照实写 0 —— 标志位是逐条消息的，
    // 声明了 grpc-encoding 也**不代表每条都必须压**。
    let use_gzip = client_accepts_gzip && response_bytes.len() >= GZIP_MIN_BYTES;

    let mut builder = Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        // 告诉对面我们收得下 gzip（它下次可以压着发）
        .header("grpc-accept-encoding", "identity,gzip");
    if use_gzip {
        builder = builder.header("grpc-encoding", "gzip");
    }
    let resp = match builder.body(()) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut stream = match respond.send_response(resp, false) {
        Ok(s) => s,
        Err(_) => return,
    };
    if status == STATUS_OK {
        let framed = frame_message_maybe_gzip(&response_bytes, use_gzip);
        if let Err(e) = stream.send_data(Bytes::from(framed), false) {
            eprintln!("[qi-grpc] 发响应体: {}", e);
            return;
        }
    }
    if let Err(e) = stream.send_trailers(build_trailers(status, &message_text)) {
        eprintln!("[qi-grpc] 发 trailers: {}", e);
    }
}

fn build_trailers(status: i64, message: &str) -> HeaderMap {
    let mut trailers = HeaderMap::new();
    trailers.insert(
        "grpc-status",
        status
            .to_string()
            .parse()
            .unwrap_or_else(|_| "13".parse().unwrap()),
    );
    if !message.is_empty() {
        // grpc-message 要按 percent-encoding 转义：HTTP 头里放不了非 ASCII，
        // 而我们的错误消息大概率是中文。不转义 h2 会拒收这个头，整条 trailers
        // 发不出去，客户端于是等到超时 —— 症状极难看出成因。
        if let Ok(value) = percent_encode(message).parse() {
            trailers.insert("grpc-message", value);
        }
    }
    trailers
}

/// 出错时只发 trailers（gRPC 允许这种 trailers-only 响应）。
fn send_status_only(respond: &mut server::SendResponse<Bytes>, status: i64, message: &str) {
    let resp = match Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .body(())
    {
        Ok(r) => r,
        Err(_) => return,
    };
    if let Ok(mut stream) = respond.send_response(resp, false) {
        let _ = stream.send_trailers(build_trailers(status, message));
    }
}

/// grpc-message 的转义：非可打印 ASCII 和 % 按 %XX 编码（规范要求）。
fn percent_encode(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.as_bytes() {
        let b = *byte;
        if (0x20..=0x7e).contains(&b) && b != b'%' {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{:02X}", b));
        }
    }
    out
}

/// 等下一条调用，最多等 `timeout_ms`。返回调用句柄；超时或服务器没了返回 0。
///
/// 超时返回而不是死等：qi 侧的循环得能腾出手看看「该退出了吗」，
/// 也才能在没请求的间隙干别的。
#[no_mangle]
pub extern "C" fn qi_grpc_accept(server_id: i64, timeout_ms: i64) -> i64 {
    let Some(server) = servers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&server_id)
        .cloned()
    else {
        return 0;
    };

    let mut queue = server.queue.lock().unwrap_or_else(|e| e.into_inner());
    if queue.is_empty() {
        let (q, _) = server
            .ready
            .wait_timeout(queue, Duration::from_millis(timeout_ms.max(1) as u64))
            .unwrap_or_else(|e| e.into_inner());
        queue = q;
    }
    queue.pop_front().unwrap_or(0)
}

/// 这条调用要打的方法全名，形如 `greet.Greeter/SayHello`。
#[no_mangle]
pub extern "C" fn qi_grpc_method(call_id: i64) -> *mut c_char {
    match pending_calls()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&call_id)
    {
        Some(call) => out_str(call.method.clone()),
        None => out_str(String::new()),
    }
}

/// 请求消息的字节切片句柄（5 字节分帧头已经脱掉）。
#[no_mangle]
pub extern "C" fn qi_grpc_request(call_id: i64) -> i64 {
    match pending_calls()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&call_id)
    {
        Some(call) => call.request_handle,
        None => 0,
    }
}

/// 回复这条调用。状态码 0 = 成功（这时才发响应体），非 0 只发状态。
///
/// 回复之后调用句柄立即失效 —— 重复回复返回 -1，不会把第二次的内容
/// 发到别人的流上。
#[no_mangle]
pub extern "C" fn qi_grpc_respond(
    call_id: i64,
    status: i64,
    message: *const c_char,
    response_bytes: i64,
) -> i64 {
    let Some(call) = pending_calls()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&call_id)
    else {
        return -1;
    };
    let payload = clone_bytes(response_bytes).unwrap_or_default();
    match call.reply.send((status, read_cstr(message), payload)) {
        // 协议层已经走了 —— 客户端多半断了，不算错误
        Err(_) => -1,
        Ok(()) => 0,
    }
}

/// 开服务端反射：把一个描述符句柄挂到服务器上。
///
/// 开了之后 grpcurl 不用带 `-proto` 就能 list/describe/调用，grpcui、
/// Postman 这类工具也才认得出服务长什么样。
#[no_mangle]
pub extern "C" fn qi_grpc_enable_reflection(server_id: i64, pool_id: i64) -> i64 {
    let Some(server) = servers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&server_id)
        .cloned()
    else {
        return -1;
    };
    let Some(pool) = crate::stdlib::protobuf_ffi::get_pool(pool_id) else {
        return -1;
    };
    *server
        .reflection_pool
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(pool);
    0
}

/// 停服务：唤醒等在「接收调用」上的循环，并让 accept 循环退出。
#[no_mangle]
pub extern "C" fn qi_grpc_stop(server_id: i64) -> i64 {
    match servers()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&server_id)
    {
        Some(server) => {
            server.stopping.store(true, Ordering::SeqCst);
            server.ready.notify_all();
            0
        }
        None => -1,
    }
}
