//! HTTP/2 server FFI
//!
//! 用 `h2` + `tokio-rustls` 把 HTTP/2 over TLS 暴露给 qi 上层。
//! 设计要点：qi 业务侧仍按 HTTP/1.1 文本处理（处理原始请求 函数）。
//! 这里在 runtime 层做 h2 ↔ HTTP/1.1 文本的转换，让 qi-web 不需要改。
//!
//! 同时支持 ALPN 回退到 HTTP/1.1：如果客户端不支持 h2，仍走 1.1 路径。

#![allow(non_snake_case)]

use bytes::Bytes;
use h2::server;
use http::Response;
use std::ffi::{c_void, CStr, CString};
use std::io::BufReader;
use std::os::raw::c_char;
use std::sync::Arc;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;
use tokio_rustls::TlsAcceptor;

/// qi 端 处理原始请求(应用值, 原始请求字符串) -> 响应字符串
/// 是 LLVM IR define ptr @fn(ptr, ptr) — 这里 transmute 时用 C-unwind 让 panic 能传播。
type QiProcessFn = unsafe extern "C-unwind" fn(*const c_void, *const c_char) -> *const c_char;

fn install_default_provider_once() {
    use std::sync::Once;
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn cstr_to_string(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { Some(CStr::from_ptr(p).to_string_lossy().into_owned()) }
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

/// 把 h2 请求拼成 HTTP/1.1 raw 文本，供 qi 端 `处理原始请求` 解析。
async fn build_http1_text(
    method: &str,
    path: &str,
    headers: &http::HeaderMap,
    body: Vec<u8>,
) -> String {
    let mut text = String::new();
    text.push_str(method);
    text.push(' ');
    text.push_str(path);
    text.push_str(" HTTP/1.1\r\n");
    for (name, value) in headers {
        // 跳过 h2 伪头 (:method, :path, :scheme, :authority) — 它们以冒号开头，
        // 但 http::HeaderMap 不允许冒号开头的名字，所以这里只见到普通头。
        let value_str = value.to_str().unwrap_or("");
        text.push_str(name.as_str());
        text.push_str(": ");
        text.push_str(value_str);
        text.push_str("\r\n");
    }
    if !body.is_empty() && headers.get("content-length").is_none() {
        text.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    text.push_str("\r\n");
    if !body.is_empty() {
        // 把字节按 lossy 方式拼到字符串里 — 仅供 qi 端字符串解析使用
        text.push_str(&String::from_utf8_lossy(&body));
    }
    text
}

/// 解析 qi 返回的 HTTP/1.1 响应文本，拆出 status / headers / body
fn parse_http1_response(text: &str) -> Option<(u16, Vec<(String, String)>, Vec<u8>)> {
    let mut lines = text.split("\r\n");
    let first = lines.next()?;
    // "HTTP/1.1 200 OK"
    let mut parts = first.splitn(3, ' ');
    let _version = parts.next()?;
    let status: u16 = parts.next()?.parse().ok()?;
    // status text 忽略

    let mut headers = Vec::new();
    let mut header_done = false;
    let mut consumed_header_chars = 0usize;
    consumed_header_chars += first.len() + 2;

    for line in lines.by_ref() {
        consumed_header_chars += line.len() + 2;
        if line.is_empty() {
            header_done = true;
            break;
        }
        if let Some(idx) = line.find(':') {
            let name = line[..idx].trim().to_string();
            let value = line[idx + 1..].trim().to_string();
            headers.push((name, value));
        }
    }

    if !header_done {
        return None;
    }

    let body = if consumed_header_chars < text.len() {
        text[consumed_header_chars..].as_bytes().to_vec()
    } else {
        Vec::new()
    };
    Some((status, headers, body))
}

async fn read_request_body(mut body: h2::RecvStream) -> Vec<u8> {
    let mut buf = Vec::new();
    while let Some(chunk) = body.data().await {
        if let Ok(b) = chunk {
            let _ = body.flow_control().release_capacity(b.len());
            buf.extend_from_slice(&b);
        }
    }
    buf
}

fn call_qi_handler(process_fn_addr: usize, app_addr: usize, raw_text: String) -> Option<String> {
    // panic 隔离
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let raw_c = CString::new(raw_text).ok()?;
        unsafe {
            let func: QiProcessFn = std::mem::transmute(process_fn_addr);
            let resp_ptr = func(app_addr as *const c_void, raw_c.as_ptr());
            if resp_ptr.is_null() {
                return None;
            }
            // qi 端返回的字符串：复制成 Rust String，不能假设可释放
            Some(CStr::from_ptr(resp_ptr).to_string_lossy().into_owned())
        }
    }));
    match result {
        Ok(opt) => opt,
        Err(_) => {
            eprintln!("[qi-h2] handler panic recovered");
            None
        }
    }
}

fn fallback_500() -> (u16, Vec<(String, String)>, Vec<u8>) {
    (
        500,
        vec![("content-type".into(), "text/plain; charset=utf-8".into())],
        b"Internal Server Error".to_vec(),
    )
}

/// 一次 HTTP/2 流处理：拿到 request → 拼 raw HTTP/1.1 → 调 qi → 解析响应 → 发回
async fn handle_h2_stream(
    request: http::Request<h2::RecvStream>,
    mut respond: server::SendResponse<Bytes>,
    process_fn_addr: usize,
    app_addr: usize,
) {
    let (parts, body) = request.into_parts();
    let method = parts.method.as_str().to_string();
    let path_q = parts
        .uri
        .path_and_query()
        .map(|pq| pq.to_string())
        .unwrap_or_else(|| "/".to_string());

    let body_bytes = read_request_body(body).await;
    let raw_text = build_http1_text(&method, &path_q, &parts.headers, body_bytes).await;

    let qi_resp =
        tokio::task::spawn_blocking(move || call_qi_handler(process_fn_addr, app_addr, raw_text))
            .await
            .ok()
            .flatten();

    let (status, headers, body) = match qi_resp.as_deref().and_then(parse_http1_response) {
        Some(t) => t,
        None => fallback_500(),
    };

    let mut http_resp = Response::builder().status(status);
    for (name, value) in &headers {
        // h2 不允许 connection 相关头
        let lname = name.to_lowercase();
        if lname == "connection"
            || lname == "keep-alive"
            || lname == "proxy-connection"
            || lname == "transfer-encoding"
            || lname == "upgrade"
        {
            continue;
        }
        http_resp = http_resp.header(name, value);
    }
    let http_resp = match http_resp.body(()) {
        Ok(r) => r,
        Err(_) => return,
    };

    let send_result = respond.send_response(http_resp, body.is_empty());
    if let Ok(mut send_stream) = send_result {
        if !body.is_empty() {
            let _ = send_stream.send_data(Bytes::from(body), true);
        }
    }
}

async fn h2_serve_inner(
    cert_path: String,
    key_path: String,
    host: String,
    port: u16,
    process_fn_addr: usize,
    app_addr: usize,
) -> Result<(), String> {
    install_default_provider_once();

    let certs = load_certs(&cert_path)?;
    let key = load_private_key(&key_path)?;

    let mut config = ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("ServerConfig: {}", e))?;
    // ALPN：优先 h2，回退 http/1.1
    config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind((host.as_str(), port))
        .await
        .map_err(|e| format!("bind {}:{} 失败: {}", host, port, e))?;

    loop {
        let (tcp, _addr) = match listener.accept().await {
            Ok(p) => p,
            Err(e) => {
                eprintln!("[qi-h2] accept: {}", e);
                continue;
            }
        };
        let acceptor = acceptor.clone();
        tokio::spawn(async move {
            let tls = match acceptor.accept(tcp).await {
                Ok(t) => t,
                Err(e) => {
                    eprintln!("[qi-h2] tls handshake: {}", e);
                    return;
                }
            };

            let alpn = tls
                .get_ref()
                .1
                .alpn_protocol()
                .map(|s| s.to_vec())
                .unwrap_or_default();

            if alpn == b"h2" {
                let mut conn = match server::handshake(tls).await {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("[qi-h2] h2 handshake: {}", e);
                        return;
                    }
                };
                while let Some(result) = conn.accept().await {
                    match result {
                        Ok((request, respond)) => {
                            tokio::spawn(handle_h2_stream(
                                request,
                                respond,
                                process_fn_addr,
                                app_addr,
                            ));
                        }
                        Err(e) => {
                            eprintln!("[qi-h2] stream error: {}", e);
                            break;
                        }
                    }
                }
            } else {
                // 客户端不支持 h2 — 回退 HTTP/1.1，把 tokio TLS 流交给同步代码
                // 用 tokio::task::spawn_blocking + tokio runtime 的 TLS handshake 已经完成，
                // 但底层 TcpStream 可能在 nodelay 模式。这里以同步方式读写。
                fallback_http1(tls, process_fn_addr, app_addr).await;
            }
        });
    }
}

/// HTTP/1.1 over TLS 回退路径 — 复用 qi 端的处理逻辑
async fn fallback_http1(
    mut tls: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    process_fn_addr: usize,
    app_addr: usize,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    loop {
        let mut buf = vec![0u8; 4096];
        let n = match tls.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        let raw = String::from_utf8_lossy(&buf[..n]).into_owned();
        let resp = tokio::task::spawn_blocking(move || {
            call_qi_handler(process_fn_addr, app_addr, raw)
        })
        .await
        .ok()
        .flatten()
        .unwrap_or_else(|| {
            "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 21\r\nConnection: close\r\n\r\nInternal Server Error".to_string()
        });

        if tls.write_all(resp.as_bytes()).await.is_err() {
            break;
        }
        if resp.contains("Connection: close") {
            break;
        }
    }
    let _ = tls.shutdown().await;
}

/// 启动一个 HTTP/2 over TLS 服务器
/// 这是阻塞调用：内部用 tokio runtime 跑事件循环。
/// 返回 0 表示成功结束；非 0 表示启动失败。
#[no_mangle]
pub extern "C-unwind" fn qi_h2_serve(
    cert_path: *const c_char,
    key_path: *const c_char,
    host: *const c_char,
    port: i64,
    process_fn: *const c_void,
    app_ptr: *const c_void,
) -> i64 {
    let cert_path = match cstr_to_string(cert_path) {
        Some(s) => s,
        None => return -1,
    };
    let key_path = match cstr_to_string(key_path) {
        Some(s) => s,
        None => return -1,
    };
    let host = match cstr_to_string(host) {
        Some(s) => s,
        None => return -1,
    };
    if process_fn.is_null() {
        return -1;
    }
    let port = port as u16;
    let process_fn_addr = process_fn as usize;
    let app_addr = app_ptr as usize;

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[qi-h2] tokio runtime: {}", e);
            return -1;
        }
    };

    let res = rt.block_on(async move {
        h2_serve_inner(cert_path, key_path, host, port, process_fn_addr, app_addr).await
    });

    if let Err(e) = res {
        eprintln!("[qi-h2] {}", e);
        return -1;
    }
    0
}
