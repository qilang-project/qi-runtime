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

/// 一条已经排队、还没收尾的调用。
///
/// 一元和流式共用这一套：一元不过是「收一条、发一条、收尾」的特例。
/// 分成两套 API 会让 qi 侧多一个「这个方法是不是流式」的分叉，
/// 而那个分叉的答案已经在 .proto 里了。
struct PendingCall {
    method: String,
    /// 客户端发过来的消息队列（协议层往里塞，qi 侧取）
    inbound: Arc<Inbound>,
    /// 往客户端发的口子。发消息和收尾都走它，保证顺序。
    outbound: tokio::sync::mpsc::UnboundedSender<Outbound>,
    /// 一元路径缓存的第一条消息 —— 请求字节 可以被调多次，
    /// 每次都从队列里弹一条的话第二次就空了。
    first_message: Mutex<Option<i64>>,
}

struct Inbound {
    queue: Mutex<VecDeque<Vec<u8>>>,
    ready: Condvar,
    /// 客户端半关了流（不会再有消息）。跟「这一轮没消息」是两回事：
    /// 前者要让 收一条 返回 -1 让循环退出，后者返回 0 让它接着等。
    ended: AtomicBool,
}

enum Outbound {
    Data(Vec<u8>),
    Finish(i64, String),
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
const STATUS_DEADLINE_EXCEEDED: i64 = 4;

static DEADLINES: OnceLock<Mutex<HashMap<i64, std::time::Instant>>> = OnceLock::new();

fn deadlines() -> &'static Mutex<HashMap<i64, std::time::Instant>> {
    DEADLINES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn set_deadline(call_id: i64, at: std::time::Instant) {
    deadlines()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(call_id, at);
}

fn clear_deadline(call_id: i64) {
    deadlines()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&call_id);
}

/// `grpc-timeout` 的值：一串数字 + 一个单位字母。
/// H 小时 / M 分钟 / S 秒 / m 毫秒 / u 微秒 / n 纳秒。
/// **M 是分钟、m 是毫秒**，大小写弄反就是六万倍的误差。
fn parse_grpc_timeout(raw: &str) -> Option<Duration> {
    let raw = raw.trim();
    let (digits, unit) = raw.split_at(raw.len().checked_sub(1)?);
    let n: u64 = digits.parse().ok()?;
    Some(match unit {
        "H" => Duration::from_secs(n.saturating_mul(3600)),
        "M" => Duration::from_secs(n.saturating_mul(60)),
        "S" => Duration::from_secs(n),
        "m" => Duration::from_millis(n),
        "u" => Duration::from_micros(n),
        "n" => Duration::from_nanos(n),
        _ => return None,
    })
}

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
    let (head, body) = req.into_parts();
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
    // 上游给的 deadline。它是**整条调用**的预算，超了就算业务还在算也没意义 ——
    // 对面早就不等了。没给就按 5 分钟兜底，免得跑飞的 handler 把流挂到天荒地老。
    let deadline = head
        .headers
        .get("grpc-timeout")
        .and_then(|v| v.to_str().ok())
        .and_then(parse_grpc_timeout)
        .unwrap_or_else(|| Duration::from_secs(300));

    let client_accepts_gzip = head
        .headers
        .get("grpc-accept-encoding")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.split(',').any(|one| one.trim() == "gzip"))
        .unwrap_or(false);

    let inbound = Arc::new(Inbound {
        queue: Mutex::new(VecDeque::new()),
        ready: Condvar::new(),
        ended: AtomicBool::new(false),
    });
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Outbound>();

    // **收头就派发**，不等第一条消息。
    //
    // 等第一条消息才派发的话，「服务端先说话」的双向流会直接卡住：
    // 客户端在等服务端，服务端在等客户端的第一条消息。一元调用不受影响 ——
    // 它那条路上 请求字节 会阻塞着等第一条，跟以前一样。
    let call_id = NEXT_CALL_ID.fetch_add(1, Ordering::SeqCst);
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
                    method,
                    inbound: inbound.clone(),
                    outbound: out_tx,
                    first_message: Mutex::new(None),
                },
            );
        // 队列里只放句柄 —— 别的都挂在 PendingCall 那一份上，存两份迟早对不上
        queue.push_back(call_id);
    }
    server.ready.notify_one();

    // 读腿：把客户端发来的消息拆帧塞进队列
    let reader_inbound = inbound.clone();
    tokio::spawn(async move {
        let mut body = body;
        let mut buf: Vec<u8> = Vec::new();
        while let Some(chunk) = body.data().await {
            let Ok(data) = chunk else { break };
            let _ = body.flow_control().release_capacity(data.len());
            buf.extend_from_slice(&data);
            loop {
                match take_one_message(&buf) {
                    Some((compressed, raw)) => {
                        let used = 5 + raw_len_after_frame(&buf);
                        buf.drain(..used);
                        let plain = if compressed {
                            if request_encoding != "gzip" {
                                break;
                            }
                            match gzip_decode(&raw) {
                                Some(p) => p,
                                None => break,
                            }
                        } else {
                            raw
                        };
                        let mut q = reader_inbound
                            .queue
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        q.push_back(plain);
                        drop(q);
                        reader_inbound.ready.notify_all();
                    }
                    None => break,
                }
            }
        }
        reader_inbound.ended.store(true, Ordering::SeqCst);
        reader_inbound.ready.notify_all();
    });

    // 记下截止时间，qi 侧可以查还剩多少预算
    set_deadline(call_id, std::time::Instant::now() + deadline);

    // 写腿：qi 侧发什么就写什么。头**懒发** —— 收尾时还没发过数据且状态非 0，
    // 就走 trailers-only 响应（gRPC 允许，且是错误路径的常规形状）。
    let mut stream: Option<h2::SendStream<Bytes>> = None;
    let sleep = tokio::time::sleep(deadline);
    tokio::pin!(sleep);
    loop {
        let item = tokio::select! {
            // 到点了：**运行时自己把话说清楚**。不主动收尾的话，客户端只能
            // 等自己那侧超时，服务端日志上什么都看不到。
            _ = &mut sleep => {
                clear_deadline(call_id);
                pending_calls().lock().unwrap_or_else(|e| e.into_inner()).remove(&call_id);
                match stream.as_mut() {
                    Some(s) => { let _ = s.send_trailers(build_trailers(STATUS_DEADLINE_EXCEEDED, "超过调用方给的期限")); }
                    None => send_status_only(&mut respond, STATUS_DEADLINE_EXCEEDED, "超过调用方给的期限"),
                }
                return;
            }
            got = out_rx.recv() => match got {
                Some(v) => v,
                None => break,
            },
        };
        {
            match item {
                Outbound::Data(payload) => {
                    if stream.is_none() {
                        match open_response(&mut respond, client_accepts_gzip) {
                            Some(s) => stream = Some(s),
                            None => return,
                        }
                    }
                    let use_gzip = client_accepts_gzip && payload.len() >= GZIP_MIN_BYTES;
                    let framed = frame_message_maybe_gzip(&payload, use_gzip);
                    if let Some(s) = stream.as_mut() {
                        if s.send_data(Bytes::from(framed), false).is_err() {
                            return;
                        }
                    }
                }
                Outbound::Finish(status, message) => {
                    clear_deadline(call_id);
                    match stream.as_mut() {
                        Some(s) => {
                            let _ = s.send_trailers(build_trailers(status, &message));
                        }
                        None => send_status_only(&mut respond, status, &message),
                    }
                    return;
                }
            }
        }
    }
    clear_deadline(call_id);

    // 通道关了却没收到 Finish —— qi 侧把这条调用丢了
    match stream.as_mut() {
        Some(s) => {
            let _ = s.send_trailers(build_trailers(STATUS_INTERNAL, "服务端没有回复这次调用"));
        }
        None => send_status_only(&mut respond, STATUS_INTERNAL, "服务端没有回复这次调用"),
    }
}

/// take_one_message 只给出消息体，这里补一个「这一帧占了多少字节」。
fn raw_len_after_frame(buf: &[u8]) -> usize {
    if buf.len() < 5 {
        return 0;
    }
    u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize
}

fn open_response(
    respond: &mut server::SendResponse<Bytes>,
    accepts_gzip: bool,
) -> Option<h2::SendStream<Bytes>> {
    // **HTTP 状态永远 200**，成败看 trailer
    let mut builder = Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        // 告诉对面我们收得下 gzip（它下次可以压着发）
        .header("grpc-accept-encoding", "identity,gzip");
    if accepts_gzip {
        builder = builder.header("grpc-encoding", "gzip");
    }
    let resp = builder.body(()).ok()?;
    respond.send_response(resp, false).ok()
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

fn find_call_inbound(call_id: i64) -> Option<Arc<Inbound>> {
    pending_calls()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&call_id)
        .map(|c| c.inbound.clone())
}

/// 收一条客户端消息，最多等 `timeout_ms`。
///
/// 返回字节切片句柄；**0 = 这一轮没收到（超时）**，**-1 = 客户端半关了，
/// 不会再有消息**。这两个必须分开：前者该接着等，后者该退出循环。
/// 混成一个值的话，客户端流的处理循环要么提前收摊要么永远转下去。
#[no_mangle]
pub extern "C" fn qi_grpc_recv(call_id: i64, timeout_ms: i64) -> i64 {
    let Some(inbound) = find_call_inbound(call_id) else {
        return -1;
    };
    let mut queue = inbound.queue.lock().unwrap_or_else(|e| e.into_inner());
    if queue.is_empty() && !inbound.ended.load(Ordering::SeqCst) {
        let (q, _) = inbound
            .ready
            .wait_timeout(queue, Duration::from_millis(timeout_ms.max(1) as u64))
            .unwrap_or_else(|e| e.into_inner());
        queue = q;
    }
    match queue.pop_front() {
        Some(msg) => register_bytes(msg),
        None => {
            if inbound.ended.load(Ordering::SeqCst) {
                -1
            } else {
                0
            }
        }
    }
}

/// 一元调用的便利入口：拿第一条请求消息（最多等 30 秒）。
///
/// 会**缓存**，调多次拿到的是同一个句柄 —— 直接每次弹队列的话第二次就空了。
/// 流式的方法别用这个，用 收一条。
#[no_mangle]
pub extern "C" fn qi_grpc_request(call_id: i64) -> i64 {
    {
        let calls = pending_calls().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(call) = calls.get(&call_id) {
            if let Some(cached) = *call.first_message.lock().unwrap_or_else(|e| e.into_inner()) {
                return cached;
            }
        } else {
            return 0;
        }
    }
    let handle = qi_grpc_recv(call_id, 30_000);
    // 没有消息（空请求 / 客户端直接关了）当空消息处理：proto3 里
    // 「所有字段都是默认值」的编码就是零字节
    let handle = if handle <= 0 {
        register_bytes(Vec::new())
    } else {
        handle
    };
    let calls = pending_calls().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(call) = calls.get(&call_id) {
        *call.first_message.lock().unwrap_or_else(|e| e.into_inner()) = Some(handle);
    }
    handle
}

/// 发一条响应消息（服务端流用）。不收尾 —— 收尾要显式调 收尾。
#[no_mangle]
pub extern "C" fn qi_grpc_send(call_id: i64, bytes_handle: i64) -> i64 {
    let calls = pending_calls().lock().unwrap_or_else(|e| e.into_inner());
    let Some(call) = calls.get(&call_id) else {
        return -1;
    };
    let payload = clone_bytes(bytes_handle).unwrap_or_default();
    match call.outbound.send(Outbound::Data(payload)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// 收尾：发 trailers 并结束这条流。调用句柄随即失效。
///
/// **每条调用都必须收尾**（哪怕是错）。不收尾的话客户端一直等到自己超时，
/// 而服务端这边一点痕迹都没有 —— 这是 gRPC 最难查的一类症状。
#[no_mangle]
pub extern "C" fn qi_grpc_finish(call_id: i64, status: i64, message: *const c_char) -> i64 {
    let Some(call) = pending_calls()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&call_id)
    else {
        return -1;
    };
    match call
        .outbound
        .send(Outbound::Finish(status, read_cstr(message)))
    {
        Ok(()) => 0,
        // 协议层已经走了 —— 客户端多半断了，不算错误
        Err(_) => -1,
    }
}

/// 一元回复 = 发一条 + 收尾。状态码非 0 时不发响应体。
#[no_mangle]
pub extern "C" fn qi_grpc_respond(
    call_id: i64,
    status: i64,
    message: *const c_char,
    response_bytes: i64,
) -> i64 {
    if status == STATUS_OK && qi_grpc_send(call_id, response_bytes) < 0 {
        return -1;
    }
    qi_grpc_finish(call_id, status, message)
}

/// 这条调用还剩多少毫秒预算。没有 deadline 信息时返回 -1。
///
/// 长活儿（查库、调 LLM）动手前问一句：只剩两百毫秒就别开工了，
/// 直接回 DEADLINE_EXCEEDED —— 干完也没人要。
#[no_mangle]
pub extern "C" fn qi_grpc_deadline_left(call_id: i64) -> i64 {
    match deadlines()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&call_id)
    {
        Some(at) => at
            .saturating_duration_since(std::time::Instant::now())
            .as_millis() as i64,
        None => -1,
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
