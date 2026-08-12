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

/// 一个客户端句柄背后可以挂多个后端。
///
/// **这不是完整的负载均衡**：没有服务发现、没有健康检查、没有 EDS 那一套。
/// 有的是最常用的那一小块 —— 一组固定地址、轮询挑一个、连坏了重连、
/// 明确「没打出去」的失败按退避重试。这块能覆盖「后端有几个副本、
/// 滚动重启时别掉请求」这个最常见的诉求。
struct ClientConn {
    subs: Vec<Mutex<SubChannel>>,
    /// 轮询游标
    next: AtomicI64,
    scheme: &'static str,
    tls_ca: Option<String>,
    /// UNAVAILABLE 时最多再试几次（0 = 不重试）
    retries: i64,
    /// 连接级默认元数据。auth token 这类每次都要带的东西放这儿，
    /// 免得每个调用点都记得传一遍 —— 漏一个就是一次 401。
    default_metadata: Vec<(String, String)>,
}

struct SubChannel {
    authority: String,
    /// None = 这条子连接现在是坏的，下次用之前要重连
    sender: Option<h2::client::SendRequest<Bytes>>,
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
const STATUS_RESOURCE_EXHAUSTED: i64 = 8;

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

/// 单条消息上限，默认 4MB（跟 gRPC 各家实现一致）。**客户端一样要设** ——
/// 一个坏掉（或恶意）的服务端可以用一条声称 2GB 的响应把客户端撑死。
/// 用 QI_GRPC_MAX_MESSAGE 调（字节）。
fn max_message_bytes() -> usize {
    static CACHED: OnceLock<usize> = OnceLock::new();
    *CACHED.get_or_init(|| {
        std::env::var("QI_GRPC_MAX_MESSAGE")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| *n > 0)
            .unwrap_or(4 * 1024 * 1024)
    })
}

/// 出站队列深度（有界 = 背压）
const OUTBOUND_DEPTH: usize = 64;

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

/// 拆一条消息。`Err(声称的长度)` = 超了上限。
fn take_one_message(buf: &[u8]) -> Result<Option<(bool, Vec<u8>)>, usize> {
    if buf.len() < 5 {
        return Ok(None);
    }
    let compressed = buf[0] != 0;
    let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    if len > max_message_bytes() {
        return Err(len);
    }
    if buf.len() < 5 + len {
        return Ok(None);
    }
    Ok(Some((compressed, buf[5..5 + len].to_vec())))
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
    // 逗号分隔多个后端：`a:1,b:2,c:3`
    let addrs: Vec<String> = raw
        .split(',')
        .map(|one| {
            one.trim()
                .trim_start_matches("https://")
                .trim_start_matches("http://")
                .trim_start_matches("grpcs://")
                .trim_start_matches("grpc://")
                .trim_end_matches('/')
                .to_string()
        })
        .filter(|s| !s.is_empty())
        .collect();
    if addrs.is_empty() {
        return -1;
    }

    let Some(rt) = client_runtime() else {
        return -1;
    };
    let ca = tls_ca.clone().unwrap_or_default();

    // 至少要连上一个才算成功；连不上的先记成坏的，用的时候再重连 ——
    // 滚动重启时有一个副本没起来不该让整个客户端建不起来。
    let mut subs = Vec::new();
    let mut any_ok = false;
    for authority in &addrs {
        let sender = rt.block_on(connect_one(authority, tls, &ca));
        if sender.is_some() {
            any_ok = true;
        }
        subs.push(Mutex::new(SubChannel {
            authority: authority.clone(),
            sender,
        }));
    }
    if !any_ok {
        eprintln!("[qi-grpc 客户端] {} 一个都连不上", addrs.join(","));
        return -1;
    }

    let id = NEXT_CONN_ID.fetch_add(1, Ordering::SeqCst);
    conns().lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        Arc::new(ClientConn {
            subs,
            next: AtomicI64::new(0),
            scheme: if tls { "https" } else { "http" },
            tls_ca: if tls { Some(ca) } else { None },
            retries: 2,
            default_metadata: Vec::new(),
        }),
    );
    id
}

/// 连一个后端。连不上返回 None（调用方决定要不要当成致命）。
async fn connect_one(
    authority: &str,
    tls: bool,
    ca: &str,
) -> Option<h2::client::SendRequest<Bytes>> {
    let host_only = authority
        .rsplit_once(':')
        .map(|(h, _)| h.to_string())
        .unwrap_or_else(|| authority.to_string());
    let authority = authority.to_string();
    let ca = ca.to_string();

    let result: Result<_, Box<dyn std::error::Error + Send + Sync>> = async {
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
        Ok(sender)
    }
    .await;

    match result {
        Ok(s) => Some(s),
        Err(e) => {
            eprintln!("[qi-grpc 客户端] 连 {} 失败: {}", authority, e);
            None
        }
    }
}

/// 轮询挑一条可用的子连接。坏的就地重连；一圈都不行返回 None。
fn pick_sender(conn: &ClientConn) -> Option<(h2::client::SendRequest<Bytes>, String)> {
    let n = conn.subs.len();
    let start = conn
        .next
        .fetch_add(1, Ordering::SeqCst)
        .rem_euclid(n as i64) as usize;
    let rt = client_runtime()?;
    for step in 0..n {
        let idx = (start + step) % n;
        let mut sub = conn.subs[idx].lock().unwrap_or_else(|e| e.into_inner());
        // **挑之前先探一下活**。h2 的 SendRequest 在连接断了之后 poll_ready
        // 会立刻报错，比「发出去等失败再重试」少一整个往返。对端静默消失时
        // 尤其重要（进程被 kill -9、中间的 NAT 悄悄把连接丢了）—— 那种情况
        // socket 上不会有 FIN，不主动探就只能等下一次调用超时才发现。
        if let Some(sender) = sub.sender.clone() {
            let alive = rt.block_on(async {
                let probe = sender.clone();
                probe.ready().await
            });
            match alive {
                Ok(ready) => {
                    // ready() 把 sender 吃掉又还回来，用还回来的那个
                    sub.sender = Some(ready.clone());
                    return Some((ready, sub.authority.clone()));
                }
                Err(_) => sub.sender = None,
            }
        }
        // 坏的：重连一次。失败就换下一个，别在这儿死磕
        let tls = conn.tls_ca.is_some() || conn.scheme == "https";
        let ca = conn.tls_ca.clone().unwrap_or_default();
        let fresh = rt.block_on(connect_one(&sub.authority, tls, &ca));
        if let Some(sender) = fresh {
            sub.sender = Some(sender.clone());
            return Some((sender, sub.authority.clone()));
        }
    }
    None
}

/// 把某条子连接标成坏的，下次用之前会重连。
fn mark_dead(conn: &ClientConn, authority: &str) {
    for sub in &conn.subs {
        let mut sub = sub.lock().unwrap_or_else(|e| e.into_inner());
        if sub.authority == authority {
            sub.sender = None;
            return;
        }
    }
}

/// 给连接设一组默认元数据（JSON 对象），每次调用都会带上。
///
/// auth token 这类东西放这儿，免得每个调用点都得记得传 —— 漏一个就是一次 401。
/// 每次调用还可以再补，同名的以**调用级**为准。
#[no_mangle]
pub extern "C" fn qi_grpc_set_metadata(conn_id: i64, json: *const c_char) -> i64 {
    let text = read_cstr(json);
    let parsed: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(_) => return -1,
    };
    let serde_json::Value::Object(map) = parsed else {
        return -1;
    };
    let mut list = Vec::new();
    for (k, v) in map {
        if let Some(text) = json_meta_value(&v) {
            list.push((k.to_ascii_lowercase(), text));
        }
    }

    let mut conns = conns().lock().unwrap_or_else(|e| e.into_inner());
    let Some(conn) = conns.get(&conn_id) else {
        return -1;
    };
    // ClientConn 在 Arc 里且没有内部可变性，直接换一个新的
    let replaced = Arc::new(ClientConn {
        subs: conn
            .subs
            .iter()
            .map(|s| {
                let s = s.lock().unwrap_or_else(|e| e.into_inner());
                Mutex::new(SubChannel {
                    authority: s.authority.clone(),
                    sender: s.sender.clone(),
                })
            })
            .collect(),
        next: AtomicI64::new(conn.next.load(Ordering::SeqCst)),
        scheme: conn.scheme,
        tls_ca: conn.tls_ca.clone(),
        retries: conn.retries,
        default_metadata: list,
    });
    conns.insert(conn_id, replaced);
    0
}

fn json_meta_value(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// 元数据往请求头上贴。**保留头不给覆盖** —— content-type、te、grpc-timeout
/// 这些是协议自己的地盘，让业务改只会把调用弄坏，而且坏得莫名其妙。
fn apply_metadata(
    mut builder: http::request::Builder,
    metadata: &[(String, String)],
) -> http::request::Builder {
    for (k, v) in metadata {
        let key = k.to_ascii_lowercase();
        if key.starts_with(':')
            || key == "content-type"
            || key == "te"
            || key == "user-agent"
            || key.starts_with("grpc-")
        {
            continue;
        }
        builder = builder.header(key, v.clone());
    }
    builder
}

/// 把调用级元数据 JSON 并到连接级默认之上（同名以调用级为准）。
fn merge_metadata(base: &[(String, String)], call_json: &str) -> Vec<(String, String)> {
    let mut out = base.to_vec();
    if call_json.trim().is_empty() {
        return out;
    }
    let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(call_json)
    else {
        return out;
    };
    for (k, v) in map {
        let Some(text) = json_meta_value(&v) else {
            continue;
        };
        let key = k.to_ascii_lowercase();
        out.retain(|(existing, _)| existing != &key);
        out.push((key, text));
    }
    out
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
    qi_grpc_call_meta(conn_id, method, request_bytes, timeout_ms, std::ptr::null())
}

/// 带调用级元数据的一元调用。元数据是 JSON 对象；同名的盖掉连接级默认。
#[no_mangle]
pub extern "C" fn qi_grpc_call_meta(
    conn_id: i64,
    method: *const c_char,
    request_bytes: i64,
    timeout_ms: i64,
    metadata_json: *const c_char,
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
    let metadata = merge_metadata(&conn.default_metadata, &read_cstr(metadata_json));
    let scheme = conn.scheme;
    let wait = Duration::from_millis(if timeout_ms <= 0 {
        30_000
    } else {
        timeout_ms as u64
    });

    // **只重试 UNAVAILABLE**：那意味着请求根本没被对面处理（连不上、连接断了、
    // 正在滚动重启）。别的错都可能是「已经执行了一半」—— 重试就是重复扣款
    // 那类事故。gRPC 官方的重试策略同理，默认也只对这一类开。
    let mut attempt = 0;
    let (status, message, body) = loop {
        let Some((sender, authority)) = pick_sender(&conn) else {
            break (
                STATUS_UNAVAILABLE,
                "所有后端都连不上".to_string(),
                Vec::new(),
            );
        };
        let payload_try = payload.clone();
        let method_try = method_name.clone();
        let authority_try = authority.clone();
        let meta_try = metadata.clone();
        let outcome = rt.block_on(async move {
            let fut = unary_call(
                sender,
                scheme,
                &authority_try,
                &method_try,
                payload_try,
                wait.as_millis() as u64,
                meta_try,
            );
            match tokio::time::timeout(wait, fut).await {
                Ok(r) => r,
                Err(_) => (
                    STATUS_DEADLINE_EXCEEDED,
                    format!("等 {} 毫秒还没回来", wait.as_millis()),
                    Vec::new(),
                ),
            }
        });
        if outcome.0 != STATUS_UNAVAILABLE || attempt >= conn.retries {
            break outcome;
        }
        // 这条子连接不行了：标坏（下次用之前会重连）、退避、换一条再试
        mark_dead(&conn, &authority);
        attempt += 1;
        std::thread::sleep(Duration::from_millis(50u64 << attempt));
    };

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
    deadline_ms: u64,
    metadata: Vec<(String, String)>,
) -> (i64, String, Vec<u8>) {
    let uri = format!(
        "{}://{}/{}",
        scheme,
        authority,
        method.trim_start_matches('/')
    );
    if payload.len() > max_message_bytes() {
        return (
            STATUS_RESOURCE_EXHAUSTED,
            format!(
                "请求 {} 字节超过上限 {}",
                payload.len(),
                max_message_bytes()
            ),
            Vec::new(),
        );
    }
    // 大请求压着发；无论压不压，都声明我们**收**得下 gzip，
    // 这样服务端可以压着回（两个头是不同方向的）。
    let (framed, did_gzip) = frame_message(&payload, payload.len() >= GZIP_MIN_BYTES);
    let mut builder = Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/grpc")
        .header("te", "trailers") // 规范要求，少了有的实现会拒
        .header("grpc-accept-encoding", "identity,gzip")
        // deadline 往下游传：对面据此掐自己的活儿，而不是傻算到底再发现
        // 没人要了。单位后缀 m = 毫秒（规范里 H/M/S/m/u/n 各有其义，
        // 别跟 M=分钟 搞混）。
        .header("grpc-timeout", format!("{}m", deadline_ms.max(1)))
        .header("user-agent", "qi-grpc");
    if did_gzip {
        builder = builder.header("grpc-encoding", "gzip");
    }
    builder = apply_metadata(builder, &metadata);
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
    let parsed = match take_one_message(&buf) {
        Ok(v) => v,
        Err(claimed) => {
            return (
                STATUS_RESOURCE_EXHAUSTED,
                format!(
                    "响应声称 {} 字节，超过上限 {}",
                    claimed,
                    max_message_bytes()
                ),
                Vec::new(),
            );
        }
    };
    match parsed {
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

// ── 客户端流式 ──────────────────────────────────────────────────
//
// 一元调用是「发一条、收一条、完事」，运行时可以整个包起来阻塞返回。
// 流式不行：qi 侧要自己决定什么时候发、什么时候收、什么时候半关，
// 所以这里跟服务端那侧一样，把 h2 的收发交给一个后台任务，
// qi 侧通过队列和通道跟它打交道。

static NEXT_STREAM_ID: AtomicI64 = AtomicI64::new(7_000_000);
static STREAMS: OnceLock<Mutex<HashMap<i64, Arc<ClientStream>>>> = OnceLock::new();

struct ClientStream {
    /// 收到的响应消息
    inbox: Mutex<std::collections::VecDeque<Vec<u8>>>,
    ready: std::sync::Condvar,
    /// 服务端把流收尾了（不会再有消息）
    done: std::sync::atomic::AtomicBool,
    /// 收尾状态。没收尾之前是 None。
    status: Mutex<Option<(i64, String)>>,
    /// 往服务端发的口子
    outbox: tokio::sync::mpsc::Sender<StreamOut>,
}

enum StreamOut {
    Data(Vec<u8>),
    CloseSend,
}

fn streams() -> &'static Mutex<HashMap<i64, Arc<ClientStream>>> {
    STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn find_stream(id: i64) -> Option<Arc<ClientStream>> {
    streams()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
}

/// 开一条流。返回流句柄；失败 -1。
///
/// 三种流（服务端流/客户端流/双向）在这一层是同一件事 —— 发几条、收几条
/// 由调用方决定。`.proto` 里的 stream 标记只影响对面怎么用。
#[no_mangle]
pub extern "C" fn qi_grpc_open_stream(conn_id: i64, method: *const c_char, timeout_ms: i64) -> i64 {
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
    let deadline_ms = if timeout_ms <= 0 {
        300_000
    } else {
        timeout_ms as u64
    };
    let Some((sender_picked, authority)) = pick_sender(&conn) else {
        return -1;
    };
    let stream_metadata = conn.default_metadata.clone();
    let uri = format!(
        "{}://{}/{}",
        conn.scheme,
        authority,
        method_name.trim_start_matches('/')
    );

    let (out_tx, mut out_rx) = tokio::sync::mpsc::channel::<StreamOut>(OUTBOUND_DEPTH);
    let state = Arc::new(ClientStream {
        inbox: Mutex::new(std::collections::VecDeque::new()),
        ready: std::sync::Condvar::new(),
        done: std::sync::atomic::AtomicBool::new(false),
        status: Mutex::new(None),
        outbox: out_tx,
    });

    let mut sender = sender_picked;
    let for_task = state.clone();
    // 开流本身要同步知道成没成，所以在这儿等握手那一步
    let (started_tx, started_rx) = std::sync::mpsc::channel::<bool>();
    rt.spawn(async move {
        let request = Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/grpc")
            .header("te", "trailers")
            .header("grpc-accept-encoding", "identity,gzip")
            .header("grpc-timeout", format!("{}m", deadline_ms))
            .header("user-agent", "qi-grpc");
        let request = match apply_metadata(request, &stream_metadata).body(()) {
            Ok(r) => r,
            Err(_) => {
                let _ = started_tx.send(false);
                return;
            }
        };
        let ready = match sender.ready().await {
            Ok(s) => s,
            Err(_) => {
                let _ = started_tx.send(false);
                return;
            }
        };
        let mut ready = ready;
        // end_of_stream = false：后面还要接着发（客户端流的前提）
        let (response, mut send_stream) = match ready.send_request(request, false) {
            Ok(p) => p,
            Err(_) => {
                let _ = started_tx.send(false);
                return;
            }
        };
        let _ = started_tx.send(true);

        // 写腿：qi 侧要发什么就发什么
        let writer = async move {
            while let Some(item) = out_rx.recv().await {
                match item {
                    StreamOut::Data(payload) => {
                        let gzip = payload.len() >= GZIP_MIN_BYTES;
                        let (framed, _) = frame_message(&payload, gzip);
                        if send_stream.send_data(Bytes::from(framed), false).is_err() {
                            return;
                        }
                    }
                    StreamOut::CloseSend => {
                        // 半关：告诉对面「我不再发了」，但还在收。
                        // 客户端流的服务端就等这个信号才开始算总账。
                        let _ = send_stream.send_data(Bytes::new(), true);
                        return;
                    }
                }
            }
        };

        // 读腿：把服务端的消息塞进 inbox
        let reader = async move {
            let response = match response.await {
                Ok(r) => r,
                Err(e) => {
                    finish_stream(&for_task, STATUS_UNAVAILABLE, format!("没等到响应: {}", e));
                    return;
                }
            };
            let head_status = read_status(response.headers());
            let gzip = response
                .headers()
                .get("grpc-encoding")
                .and_then(|v| v.to_str().ok())
                .map(|v| v.trim() == "gzip")
                .unwrap_or(false);
            let mut body = response.into_body();
            let mut buf: Vec<u8> = Vec::new();
            while let Some(chunk) = body.data().await {
                let Ok(data) = chunk else { break };
                let _ = body.flow_control().release_capacity(data.len());
                buf.extend_from_slice(&data);
                loop {
                    let parsed = match take_one_message(&buf) {
                        Ok(v) => v,
                        Err(claimed) => {
                            finish_stream(
                                &for_task,
                                STATUS_RESOURCE_EXHAUSTED,
                                format!("响应声称 {} 字节，超过上限", claimed),
                            );
                            return;
                        }
                    };
                    let Some((compressed, raw)) = parsed else {
                        break;
                    };
                    let used = 5 + raw.len().max(frame_len(&buf));
                    buf.drain(..used.min(buf.len()));
                    let plain = if compressed {
                        if !gzip {
                            break;
                        }
                        match gzip_decode(&raw) {
                            Some(p) => p,
                            None => break,
                        }
                    } else {
                        raw
                    };
                    let mut inbox = for_task.inbox.lock().unwrap_or_else(|e| e.into_inner());
                    inbox.push_back(plain);
                    drop(inbox);
                    for_task.ready.notify_all();
                }
            }
            let trailer_status = match body.trailers().await {
                Ok(Some(t)) => read_status(&t),
                _ => None,
            };
            let (code, msg) = trailer_status
                .or(head_status)
                .unwrap_or((STATUS_UNKNOWN, "对面没给 grpc-status".to_string()));
            finish_stream(&for_task, code, msg);
        };

        tokio::join!(writer, reader);
    });

    match started_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(true) => {}
        _ => return -1,
    }

    let id = NEXT_STREAM_ID.fetch_add(1, Ordering::SeqCst);
    streams()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, state);
    id
}

fn frame_len(buf: &[u8]) -> usize {
    if buf.len() < 5 {
        return 0;
    }
    u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize
}

fn finish_stream(state: &Arc<ClientStream>, code: i64, message: String) {
    *state.status.lock().unwrap_or_else(|e| e.into_inner()) = Some((code, message));
    state.done.store(true, std::sync::atomic::Ordering::SeqCst);
    state.ready.notify_all();
}

/// 往流上发一条消息。
#[no_mangle]
pub extern "C" fn qi_grpc_stream_send(stream_id: i64, bytes_handle: i64) -> i64 {
    let Some(state) = find_stream(stream_id) else {
        return -1;
    };
    let payload = clone_bytes(bytes_handle).unwrap_or_default();
    if payload.len() > max_message_bytes() {
        return -1;
    }
    match state.outbox.blocking_send(StreamOut::Data(payload)) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// 半关：不再发了，但还收。**客户端流必须调这个**，
/// 否则服务端一直在等下一条，两边干瞪眼。
#[no_mangle]
pub extern "C" fn qi_grpc_stream_close_send(stream_id: i64) -> i64 {
    let Some(state) = find_stream(stream_id) else {
        return -1;
    };
    match state.outbox.blocking_send(StreamOut::CloseSend) {
        Ok(()) => 0,
        Err(_) => -1,
    }
}

/// 收一条。返回字节切片句柄；**0 = 这轮没有（超时）**，
/// **-1 = 服务端已收尾**（去查 流状态）。
#[no_mangle]
pub extern "C" fn qi_grpc_stream_recv(stream_id: i64, timeout_ms: i64) -> i64 {
    let Some(state) = find_stream(stream_id) else {
        return -1;
    };
    let mut inbox = state.inbox.lock().unwrap_or_else(|e| e.into_inner());
    if inbox.is_empty() && !state.done.load(std::sync::atomic::Ordering::SeqCst) {
        let (q, _) = state
            .ready
            .wait_timeout(inbox, Duration::from_millis(timeout_ms.max(1) as u64))
            .unwrap_or_else(|e| e.into_inner());
        inbox = q;
    }
    match inbox.pop_front() {
        Some(msg) => register_bytes(msg),
        None => {
            if state.done.load(std::sync::atomic::Ordering::SeqCst) {
                -1
            } else {
                0
            }
        }
    }
}

/// 流的收尾状态码。还没收尾返回 -1。
#[no_mangle]
pub extern "C" fn qi_grpc_stream_status(stream_id: i64) -> i64 {
    match find_stream(stream_id) {
        Some(state) => match &*state.status.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((code, _)) => *code,
            None => -1,
        },
        None => -1,
    }
}

/// 流的收尾说明。
#[no_mangle]
pub extern "C" fn qi_grpc_stream_message(stream_id: i64) -> *mut c_char {
    match find_stream(stream_id) {
        Some(state) => match &*state.status.lock().unwrap_or_else(|e| e.into_inner()) {
            Some((_, msg)) => out_str(msg.clone()),
            None => out_str(String::new()),
        },
        None => out_str("流句柄无效".to_string()),
    }
}

/// 关掉流句柄。没半关过的话顺手半关，免得对面一直等。
#[no_mangle]
pub extern "C" fn qi_grpc_stream_free(stream_id: i64) -> i64 {
    match streams()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&stream_id)
    {
        Some(state) => {
            let _ = state.outbox.try_send(StreamOut::CloseSend);
            0
        }
        None => -1,
    }
}
