//! Web framework runtime helpers
//!
//! Provides panic-safe helpers used by qi-web's `recover` middleware so a
//! crashing handler returns a 500 response instead of taking down the goroutine.

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr};
use std::io::Write;
use std::sync::OnceLock;

/// Call a Qi handler `fn(*const Ctx) -> *const Response` with panic isolation.
/// Returns the handler's response pointer on success, or null on panic.
/// The qi-web recover middleware checks for null and synthesizes a 500.
/// Uses C-unwind so panics from the called Qi/Rust code can unwind here.
#[no_mangle]
pub extern "C-unwind" fn qi_web_call_handler_safe(
    handler_fn: *const c_void,
    ctx_ptr: *const c_void,
) -> *const c_void {
    if handler_fn.is_null() {
        return std::ptr::null();
    }
    let handler_addr = handler_fn as usize;
    let ctx_addr = ctx_ptr as usize;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let func = std::mem::transmute::<usize, extern "C-unwind" fn(*const c_void) -> *const c_void>(
            handler_addr,
        );
        func(ctx_addr as *const c_void)
    }));

    match result {
        Ok(ptr) => ptr,
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            eprintln!("[qi-web] handler panic recovered: {}", msg);
            std::ptr::null()
        }
    }
}

/// 调用 (app_ptr, raw_request_ptr) -> response_string_ptr 的处理函数，panic 兜底。
/// 返回 *mut c_char（C 字符串）；qi 侧把它当 字符串 接收。
/// panic 时返回一个固定的 "HTTP/1.1 500 ..." 字符串。
/// C-unwind ABI 让 panic 能从被调用方传到这里被 catch_unwind 抓到。
#[no_mangle]
pub extern "C-unwind" fn qi_web_safe_process_request(
    process_fn: *const c_void,
    app_ptr: *const c_void,
    raw_request_ptr: *const c_char,
) -> *const c_char {
    if process_fn.is_null() {
        return fallback_500();
    }
    let process_addr = process_fn as usize;
    let app_addr = app_ptr as usize;
    let raw_addr = raw_request_ptr as usize;

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
        let func = std::mem::transmute::<
            usize,
            extern "C-unwind" fn(*const c_void, *const c_char) -> *const c_char,
        >(process_addr);
        func(app_addr as *const c_void, raw_addr as *const c_char)
    }));

    match result {
        Ok(ptr) if !ptr.is_null() => ptr,
        Ok(_) => fallback_500(),
        Err(payload) => {
            let msg = if let Some(s) = payload.downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = payload.downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic".to_string()
            };
            eprintln!("[qi-web] request panic recovered: {}", msg);
            fallback_500()
        }
    }
}

/// 测试用：故意 panic 让 recover 能演示
/// 用 "C-unwind" ABI 才能让 panic 越过 FFI 边界传递到上游的 catch_unwind
#[no_mangle]
pub extern "C-unwind" fn qi_web_panic_for_test() -> i64 {
    panic!("intentional panic for recover demo");
}

#[inline]
fn cstr_bytes<'a>(p: *const c_char) -> &'a [u8] {
    if p.is_null() {
        &[]
    } else {
        unsafe { CStr::from_ptr(p).to_bytes() }
    }
}

// ── sendfile：二进制文件 Rust 侧读取直发 ──────────────────────────────
// qi-web `发送文件` 构造带内部标记头 `X-Qi-Sendfile: <绝对路径>`（body 留空、
// Content-Type 已由扩展名映射设好）。这里的响应序列化检测到该头就：剥掉内部头 →
// 校验路径 → `std::fs::read` 读原始字节 → 用真实字节数当 Content-Length 直接写进
// 响应体。这样非法 UTF-8 的图片/字体/下载不再经 C 字符串管道（会被 NUL 截断 / 读空
// 误报 404），彻底二进制安全。
//
// 覆盖：同步 `处理连接` 与 M:N 异步服务两条路径（都走这里序列化）。TLS / HTTP2
// 直连走 qi 端 `输出响应` 字符串路径（见 响应.qi 的回退处理），二进制经此受限。

/// 快速判断 headers 是否含 sendfile 标记（case-insensitive）。热路径先过这一关，
/// 无标记时零额外分配，老路径逐字节不变、性能零回退。
#[inline]
fn headers_have_sendfile(headers: &[u8]) -> bool {
    const K: &[u8] = b"x-qi-sendfile";
    if headers.len() < K.len() {
        return false;
    }
    headers.windows(K.len()).any(|w| w.eq_ignore_ascii_case(K))
}

// ── 缓存体：预构建 HTTP 响应零拷贝直发 ─────────────────────────────────
// qi-web `注册缓存体(内容, 类型)` 启动时调一次：这里把**完整 HTTP/1.1 响应**
// （状态行 + Content-Type + Connection + Content-Length + body）预构建成
// keep-alive / close 两个变体，注册为持久字节（负句柄，见 bytes_ffi）。
// handler 返回 `缓存体响应(体id)`（唯一头 `X-Qi-Cached-Body: <id>`、body 空），
// 序列化在此命中标记 → 直接返回对应持久句柄：每请求零分配、零拷贝；
// 写出方按负句柄走 Arc 借用直写，`释放切片` 对负句柄是 no-op。
// 语义约定：缓存体响应恒为 200 OK，且不可再附加其它响应头（附加会退回普通路径）。

static CACHED_RESPONSES: std::sync::OnceLock<dashmap::DashMap<i64, (i64, i64)>> =
    std::sync::OnceLock::new();
static CACHED_ID: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

fn cached_responses() -> &'static dashmap::DashMap<i64, (i64, i64)> {
    CACHED_RESPONSES.get_or_init(dashmap::DashMap::new)
}

/// 注册缓存体：预构建 ka/close 两个完整响应，返回 体id（>0；失败 -1）。
#[no_mangle]
pub extern "C" fn qi_web_cache_body_register(
    body_ptr: *const c_char,
    content_type_ptr: *const c_char,
) -> i64 {
    let body = cstr_bytes(body_ptr);
    let ct = cstr_bytes(content_type_ptr);
    if body.is_empty() {
        return -1;
    }
    let ct: &[u8] = if ct.is_empty() {
        b"application/json; charset=utf-8"
    } else {
        ct
    };
    let mut headers: Vec<u8> = Vec::with_capacity(16 + ct.len());
    headers.extend_from_slice(b"Content-Type: ");
    headers.extend_from_slice(ct);

    let ka = build_http_response(200, b"OK", &headers, Some(b"Connection: keep-alive"), body);
    let close = build_http_response(200, b"OK", &headers, Some(b"Connection: close"), body);
    let ka_h = crate::stdlib::bytes_ffi::register_persistent_bytes(ka);
    let close_h = crate::stdlib::bytes_ffi::register_persistent_bytes(close);

    let id = CACHED_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
    cached_responses().insert(id, (ka_h, close_h));
    id
}

/// 序列化快路径：headers 恰好是单条 `X-Qi-Cached-Body: <id>` 时返回预构建
/// 持久句柄（按 keep_alive 选变体）；否则返回 0 走普通路径。
#[inline]
fn cached_body_fast_path(headers: &[u8], keep_alive: bool) -> i64 {
    const K: &[u8] = b"X-Qi-Cached-Body: ";
    if headers.len() <= K.len() || &headers[..K.len()] != K {
        return 0;
    }
    let rest = &headers[K.len()..];
    // 必须是纯标记（无其它头）：附加了别的头就退回普通路径
    if rest.iter().any(|&b| b == b'\r' || b == b'\n') {
        return 0;
    }
    let id = match std::str::from_utf8(rest).ok().and_then(|s| s.trim().parse::<i64>().ok()) {
        Some(v) => v,
        None => return 0,
    };
    match cached_responses().get(&id) {
        Some(e) => {
            if keep_alive {
                e.0
            } else {
                e.1
            }
        }
        None => 0,
    }
}

/// 从 headers 里剥掉 X-Qi-Sendfile 行，返回 (剥掉标记后的 headers, 文件路径)。
fn strip_sendfile_header(headers: &[u8]) -> (Vec<u8>, Option<Vec<u8>>) {
    const KEY: &[u8] = b"x-qi-sendfile:";
    let mut kept: Vec<u8> = Vec::with_capacity(headers.len());
    let mut path: Option<Vec<u8>> = None;
    let mut first = true;
    for raw in headers.split(|&b| b == b'\n') {
        let line = if raw.last() == Some(&b'\r') {
            &raw[..raw.len() - 1]
        } else {
            raw
        };
        if line.len() >= KEY.len() && line[..KEY.len()].eq_ignore_ascii_case(KEY) {
            let mut v = &line[KEY.len()..];
            while matches!(v.first(), Some(b' ') | Some(b'\t')) {
                v = &v[1..];
            }
            while matches!(v.last(), Some(b' ') | Some(b'\t')) {
                v = &v[..v.len() - 1];
            }
            path = Some(v.to_vec());
        } else if !line.is_empty() {
            if !first {
                kept.extend_from_slice(b"\r\n");
            }
            kept.extend_from_slice(line);
            first = false;
        }
    }
    (kept, path)
}

/// sendfile 路径安全：必须绝对路径（Unix `/…` 或 Windows `X:\` / `X:/`）且不含 `..`。
fn sendfile_path_ok(path: &[u8]) -> bool {
    if path.is_empty() || find_subslice(path, b"..").is_some() {
        return false;
    }
    if path.first() == Some(&b'/') {
        return true;
    }
    path.len() >= 3
        && path[0].is_ascii_alphabetic()
        && path[1] == b':'
        && (path[2] == b'/' || path[2] == b'\\')
}

/// 组装一条 HTTP/1.1 响应到 Vec：状态行 + headers + [Connection] + Content-Length + body。
#[inline]
fn build_http_response(
    status_code: i64,
    status_text: &[u8],
    headers: &[u8],
    conn_header: Option<&[u8]>,
    body: &[u8],
) -> Vec<u8> {
    let cap = 48 + headers.len() + conn_header.map(|c| c.len() + 2).unwrap_or(0) + 32 + body.len();
    let mut out: Vec<u8> = Vec::with_capacity(cap);
    out.extend_from_slice(b"HTTP/1.1 ");
    let _ = write!(out, "{}", status_code);
    out.extend_from_slice(b" ");
    out.extend_from_slice(status_text);
    out.extend_from_slice(b"\r\n");
    if !headers.is_empty() {
        out.extend_from_slice(headers);
        out.extend_from_slice(b"\r\n");
    }
    if let Some(conn) = conn_header {
        out.extend_from_slice(conn);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(b"Content-Length: ");
    let _ = write!(out, "{}", body.len());
    out.extend_from_slice(b"\r\n\r\n");
    out.extend_from_slice(body);
    out
}

/// sendfile 慢路径：剥标记头 → 校验路径 → 读文件字节 → 组装。
/// 路径不安全 → 500；文件读不到 → 404。conn_header 透传 keep-alive 语义。
fn serialize_sendfile_response(
    status_code: i64,
    status_text: &[u8],
    headers: &[u8],
    conn_header: Option<&[u8]>,
) -> Vec<u8> {
    let (clean_headers, path) = strip_sendfile_header(headers);
    let path = match path {
        Some(p) => p,
        None => {
            return build_http_response(status_code, status_text, &clean_headers, conn_header, &[])
        }
    };
    const CT_TEXT: &[u8] = b"Content-Type: text/plain; charset=utf-8";
    if !sendfile_path_ok(&path) {
        let msg = b"500 Internal Server Error: invalid sendfile path";
        return build_http_response(500, b"Internal Server Error", CT_TEXT, conn_header, msg);
    }
    let path_str = match std::str::from_utf8(&path) {
        Ok(s) => s,
        Err(_) => {
            let msg = b"500 Internal Server Error: bad sendfile path encoding";
            return build_http_response(500, b"Internal Server Error", CT_TEXT, conn_header, msg);
        }
    };
    match std::fs::read(path_str) {
        Ok(bytes) => build_http_response(
            status_code,
            status_text,
            &clean_headers,
            conn_header,
            &bytes,
        ),
        Err(_) => {
            let body = format!("404 Not Found: {}", path_str);
            build_http_response(404, b"Not Found", CT_TEXT, conn_header, body.as_bytes())
        }
    }
}

/// 一次性 HTTP/1.1 响应序列化：把状态行 + 头部 + Content-Length + body 一锅写完，
/// 一个 alloc，零中间字符串。返回字节切片句柄，调用方负责 free。
///
/// 替代 qi-web 端 `输出响应头部` + `缓冲::从字符串` + `缓冲::追加字符串` 那条 ~10
/// 次小分配的链条。对 hot path 的"快响应"尤其有效（bench_最小 那种）。
/// 检测到 X-Qi-Sendfile 标记头则走 sendfile 二进制直发路径。
#[no_mangle]
pub extern "C" fn qi_runtime_serialize_http_response(
    status_code: i64,
    status_text_ptr: *const c_char,
    headers_ptr: *const c_char,
    body_ptr: *const c_char,
) -> i64 {
    let status_text = cstr_bytes(status_text_ptr);
    let headers = cstr_bytes(headers_ptr);
    let out = if headers_have_sendfile(headers) {
        serialize_sendfile_response(status_code, status_text, headers, None)
    } else {
        let body = cstr_bytes(body_ptr);
        build_http_response(status_code, status_text, headers, None, body)
    };
    crate::stdlib::bytes_ffi::register_bytes(out)
}

/// 跟 qi_runtime_serialize_http_response 同样的功能，额外接 keep_alive 标志，
/// 自动追加 Connection: keep-alive / close 头部。这样 qi-web 不再需要 注入连接头。
#[no_mangle]
pub extern "C" fn qi_runtime_serialize_http_response_ka(
    status_code: i64,
    status_text_ptr: *const c_char,
    headers_ptr: *const c_char,
    body_ptr: *const c_char,
    keep_alive: i64,
) -> i64 {
    let status_text = cstr_bytes(status_text_ptr);
    let headers = cstr_bytes(headers_ptr);

    // 缓存体零拷贝快路径：命中即返回预构建持久句柄（负数），零分配零拷贝
    let cached = cached_body_fast_path(headers, keep_alive != 0);
    if cached != 0 {
        return cached;
    }

    let conn_header: &[u8] = if keep_alive != 0 {
        b"Connection: keep-alive"
    } else {
        b"Connection: close"
    };

    let out = if headers_have_sendfile(headers) {
        serialize_sendfile_response(status_code, status_text, headers, Some(conn_header))
    } else {
        let body = cstr_bytes(body_ptr);
        build_http_response(status_code, status_text, headers, Some(conn_header), body)
    };
    crate::stdlib::bytes_ffi::register_bytes(out)
}

// ============================================================================
// HTTP/1.1 请求解析 fast path —— 替代 qi-web 端 13 次 字符串::子串/查找 链条
// ============================================================================

/// HTTP request parsed into 5 fields. Lives as long as the qi caller holds
/// the opaque pointer; freed via qi_web_request_parts_free.
///
/// 字段用 **RC 分配的 C 串**（rc_cstr_*，隐藏 header），而非裸 CString：
/// accessor 返回的指针语义仍是「借引用」（parts 存活期内有效，parts 持有一个引用），
/// 但 QI_ARC 插桩的 qi_string_retain/free 对它是**有效且平衡**的 ——
/// Qi 侧把它存进变量/结构体时 retain，parts_free 释放自己那份引用，
/// buffer 在最后一个引用消失时才真正回收。零「非 RC 指针」警告，零 UAF。
pub struct RequestParts {
    method: RcStr,
    path: RcStr,
    query: RcStr,
    headers: RcStr,
    body: RcStr,
    /// 1 = keep-alive, 0 = close。HTTP/1.1 默认 keep-alive，除非 Connection: close
    keep_alive: i64,
}

/// 持有一个 RC C 串引用的 RAII 包装：Drop 时 release。
struct RcStr(*mut c_char);

impl RcStr {
    /// 从字节分配（内嵌 NUL 替换为空格，保持 C 字符串约束）。refcount = 1。
    fn from_bytes(b: &[u8]) -> Self {
        if b.contains(&0) {
            let cleaned: Vec<u8> = b.iter().map(|&x| if x == 0 { b' ' } else { x }).collect();
            RcStr(crate::stdlib::qi_str::rc_cstr_from_bytes(&cleaned))
        } else {
            RcStr(crate::stdlib::qi_str::rc_cstr_from_bytes(b))
        }
    }

    #[inline]
    fn as_ptr(&self) -> *const c_char {
        self.0
    }
}

impl Drop for RcStr {
    fn drop(&mut self) {
        crate::stdlib::qi_str::rc_cstr_release(self.0);
    }
}

// RequestParts / MatchResult 以 i64 句柄跨 FFI 传递，可能在 runtime 线程池间移动。
// RcStr 内部是 RC buffer 的 data 指针，引用计数原子递增减，跨线程安全。
unsafe impl Send for RcStr {}
unsafe impl Sync for RcStr {}

/// 从字节切片句柄解析 HTTP/1.1 请求，返回 *mut RequestParts。
/// 失败返回 null。调用方负责调 qi_web_request_parts_free 释放。
#[no_mangle]
pub extern "C" fn qi_web_parse_request_bytes(bytes_handle: i64) -> *mut RequestParts {
    // 借引用字节池数据解析（零拷贝）。RequestParts 内部按需 CString 拷贝出来，
    // 闭包返回后字节池里的 Vec 仍归字节池所有。
    match crate::stdlib::bytes_ffi::with_bytes(bytes_handle, parse_http_request) {
        Some(parts) => Box::into_raw(Box::new(parts)),
        None => std::ptr::null_mut(),
    }
}

/// 从 c_string 解析（兼容旧 qi-web 解析请求 签名）
#[no_mangle]
pub extern "C" fn qi_web_parse_request_cstr(s: *const c_char) -> *mut RequestParts {
    if s.is_null() {
        return std::ptr::null_mut();
    }
    let bytes = unsafe { CStr::from_ptr(s).to_bytes() };
    Box::into_raw(Box::new(parse_http_request(bytes)))
}

fn parse_http_request(bytes: &[u8]) -> RequestParts {
    // 找第一个 \r\n（或 \n）— 请求行结束
    let line_end = find_subslice(bytes, b"\r\n")
        .unwrap_or_else(|| find_subslice(bytes, b"\n").unwrap_or(bytes.len()));
    let request_line = &bytes[..line_end];

    // request_line: METHOD SP PATH SP HTTP-VERSION
    let mut method = &b""[..];
    let mut full_path = &b""[..];
    if let Some(sp1) = request_line.iter().position(|&b| b == b' ') {
        method = &request_line[..sp1];
        let rest = &request_line[sp1 + 1..];
        if let Some(sp2) = rest.iter().position(|&b| b == b' ') {
            full_path = &rest[..sp2];
        } else {
            full_path = rest;
        }
    }

    // path?query
    let (path, query) = match full_path.iter().position(|&b| b == b'?') {
        Some(qmark) => (&full_path[..qmark], &full_path[qmark + 1..]),
        None => (full_path, &b""[..]),
    };

    // 跳过 \r\n（或 \n），找 \r\n\r\n（或 \n\n）
    let after_line_start = if bytes.get(line_end..line_end + 2) == Some(b"\r\n") {
        line_end + 2
    } else if bytes.get(line_end..line_end + 1) == Some(b"\n") {
        line_end + 1
    } else {
        line_end
    };
    let rest = &bytes[after_line_start..];
    let (headers, body) = match find_subslice(rest, b"\r\n\r\n") {
        Some(boundary) => (&rest[..boundary], &rest[boundary + 4..]),
        None => match find_subslice(rest, b"\n\n") {
            Some(boundary) => (&rest[..boundary], &rest[boundary + 2..]),
            None => (rest, &b""[..]),
        },
    };

    // 推导 keep-alive：HTTP/1.1 默认保持，除非 Connection: close。
    // ASCII case-insensitive 找 "connection:" header。
    let keep_alive = parse_connection_keep_alive(headers);

    RequestParts {
        method: RcStr::from_bytes(method),
        path: RcStr::from_bytes(path),
        query: RcStr::from_bytes(query),
        headers: RcStr::from_bytes(headers),
        body: RcStr::from_bytes(body),
        keep_alive,
    }
}

/// 从 headers 字节串里 case-insensitive 解析 Connection 头：
/// 显式 close → 0；显式 keep-alive 或缺省 → 1
fn parse_connection_keep_alive(headers: &[u8]) -> i64 {
    const KEY: &[u8] = b"connection:";
    if headers.len() < KEY.len() {
        return 1;
    }
    let mut i = 0usize;
    while i + KEY.len() <= headers.len() {
        let mut matched = true;
        for k in 0..KEY.len() {
            let b = headers[i + k];
            let bl = if b.is_ascii_uppercase() { b | 0x20 } else { b };
            if bl != KEY[k] {
                matched = false;
                break;
            }
        }
        if matched {
            // 跳过空白
            let mut j = i + KEY.len();
            while j < headers.len() && (headers[j] == b' ' || headers[j] == b'\t') {
                j += 1;
            }
            // 取直到 \r 或 \n
            let mut e = j;
            while e < headers.len() && headers[e] != b'\r' && headers[e] != b'\n' {
                e += 1;
            }
            let val = &headers[j..e];
            // 比 close
            if val.len() >= 5 {
                let lower5: [u8; 5] = [
                    val[0] | 0x20,
                    val[1] | 0x20,
                    val[2] | 0x20,
                    val[3] | 0x20,
                    val[4] | 0x20,
                ];
                if &lower5 == b"close" {
                    return 0;
                }
            }
            return 1;
        }
        // 找下一行起点（可能含 \n 或 \r\n）
        match headers[i..].iter().position(|&b| b == b'\n') {
            Some(rel) => i += rel + 1,
            None => break,
        }
    }
    1
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || needle.len() > haystack.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// 一次扫描判断 HTTP 请求是否完整（headers 终止 \r\n\r\n + body 字节数 >= Content-Length）
/// 返回 1 = 完整，0 = 未完整。零分配，case-insensitive Content-Length 查找。
#[no_mangle]
pub extern "C" fn qi_web_request_is_complete(bytes_handle: i64) -> i64 {
    crate::stdlib::bytes_ffi::with_bytes(bytes_handle, |bytes| {
        // 寻找 headers/body 边界
        let boundary = match find_subslice(bytes, b"\r\n\r\n") {
            Some(b) => b,
            None => return 0i64,
        };
        let headers = &bytes[..boundary];
        // ASCII case-insensitive 找 "content-length:"
        let cl = find_content_length(headers);
        match cl {
            None => 1, // 没 Content-Length 假设无 body（GET/HEAD）
            Some(expected) => {
                let body_len = bytes.len().saturating_sub(boundary + 4);
                if body_len >= expected {
                    1
                } else {
                    0
                }
            }
        }
    })
    .unwrap_or(0)
}

/// 在 ASCII headers 字节串里 case-insensitive 找 Content-Length 数值
/// 返回 None 表示无该头或解析失败
fn find_content_length(headers: &[u8]) -> Option<usize> {
    const NEEDLE: &[u8] = b"content-length:";
    if headers.len() < NEEDLE.len() {
        return None;
    }
    let mut i = 0usize;
    while i + NEEDLE.len() <= headers.len() {
        // ASCII case-insensitive 比较：把每个字节按需 |= 0x20
        let mut matched = true;
        for k in 0..NEEDLE.len() {
            let b = headers[i + k];
            let bl = if b.is_ascii_uppercase() { b | 0x20 } else { b };
            if bl != NEEDLE[k] {
                matched = false;
                break;
            }
        }
        if matched {
            // 跳过冒号后的空白
            let mut j = i + NEEDLE.len();
            while j < headers.len() && (headers[j] == b' ' || headers[j] == b'\t') {
                j += 1;
            }
            // 解析数字
            let mut n: usize = 0;
            let mut got = false;
            while j < headers.len() && headers[j].is_ascii_digit() {
                n = n
                    .saturating_mul(10)
                    .saturating_add((headers[j] - b'0') as usize);
                j += 1;
                got = true;
            }
            if got {
                return Some(n);
            }
            return None;
        }
        i += 1;
    }
    None
}

// 借引用：返回 RequestParts 内部 RC C 串的指针。生命期跟 RequestParts 一致，
// 调用方必须在调 qi_web_request_parts_free 之前别再读这些指针；
// 若调用方（QI_ARC 插桩）retain 过，则 retain 的那份在 parts_free 之后依然有效。
// qi-web 的安全契约：服务器 hot path 在 序列化响应 把 bytes 拷到独立 buffer
// 之后才 free RequestParts，所以即便 handler 把请求字符串原样塞进响应也安全。
//
// 空字符串：accessor 入参为 null 时返回 RC 分配器的 immortal 空串
// （带合法 header，retain/free 皆 no-op —— 不会触发「非 RC 指针」警告）。
#[inline]
fn empty_cptr() -> *const c_char {
    crate::stdlib::qi_str::rc_cstr_from_bytes(b"")
}

#[no_mangle]
pub extern "C" fn qi_web_request_method(p: *const RequestParts) -> *const c_char {
    if p.is_null() {
        return empty_cptr();
    }
    unsafe { (*p).method.as_ptr() }
}

#[no_mangle]
pub extern "C" fn qi_web_request_path(p: *const RequestParts) -> *const c_char {
    if p.is_null() {
        return empty_cptr();
    }
    unsafe { (*p).path.as_ptr() }
}

#[no_mangle]
pub extern "C" fn qi_web_request_query(p: *const RequestParts) -> *const c_char {
    if p.is_null() {
        return empty_cptr();
    }
    unsafe { (*p).query.as_ptr() }
}

#[no_mangle]
pub extern "C" fn qi_web_request_headers(p: *const RequestParts) -> *const c_char {
    if p.is_null() {
        return empty_cptr();
    }
    unsafe { (*p).headers.as_ptr() }
}

#[no_mangle]
pub extern "C" fn qi_web_request_body(p: *const RequestParts) -> *const c_char {
    if p.is_null() {
        return empty_cptr();
    }
    unsafe { (*p).body.as_ptr() }
}

/// 是否保持连接：1 = keep-alive，0 = close。预解析过的字段，O(1)。
#[no_mangle]
pub extern "C" fn qi_web_request_keep_alive(p: *const RequestParts) -> i64 {
    if p.is_null() {
        return 1;
    }
    unsafe { (*p).keep_alive }
}

/// Returns 0 (i64) — qi codegen assigns return values; void breaks at the
/// emission point, so we return a dummy i64 instead.
#[no_mangle]
pub extern "C" fn qi_web_request_parts_free(p: *mut RequestParts) -> i64 {
    if !p.is_null() {
        unsafe {
            drop(Box::from_raw(p));
        }
    }
    0
}

// ============================================================================
// 路由表 Rust 镜像 —— 注册时跟 qi 端 路由树 同步落一份；匹配走 Rust，省掉
// 字符串::子串/查找/等于 链条。
// ============================================================================
//
// 设计：tree of RouteNode，每节点：
//   - static_children: HashMap<seg_bytes, child>
//   - param_child: Option<(name_bytes, child)>
//   - handlers[7]: i64 处理器索引，-1 表示未注册
//
// 方法 → 索引映射：
//   GET=0, HEAD=1, POST=2, PUT=3, PATCH=4, DELETE=5, OPTIONS=6
//
// qi-web 端 应用值.处理器列表 仍持有 fn ptr；本表只存 *index*，handler dispatch
// 仍走 qi 端 列表库::获取指针(应用值.处理器列表, 处理器索引)。
//
// 单全局表：每进程只有一个 应用，匹配时不区分 应用 实例。如果未来想多 router
// 在同进程，把 ROUTER 换成 DashMap<i64, RouteNode> + app_id 入参即可。

#[derive(Default)]
struct RouteNode {
    static_children: HashMap<Vec<u8>, RouteNode>,
    param_child: Option<(Vec<u8>, Box<RouteNode>)>,
    handlers: [i64; 7],
}

impl RouteNode {
    fn new() -> Self {
        Self {
            static_children: HashMap::new(),
            param_child: None,
            handlers: [-1; 7],
        }
    }
}

// Mutex 是写锁路径（注册路由时） + RwLock 想法不直接套，因为修改和读取都
// 走同一棵树。但**注册只在 server 启动期发生，运行期 100% read**。所以：
// 注册：write lock（启动时一次性，无并发竞争）
// 匹配：read lock（多线程并发读，零阻塞）
static ROUTER: OnceLock<std::sync::RwLock<RouteNode>> = OnceLock::new();

fn router() -> &'static std::sync::RwLock<RouteNode> {
    ROUTER.get_or_init(|| std::sync::RwLock::new(RouteNode::new()))
}

#[inline]
fn method_idx(method: &[u8]) -> i32 {
    match method {
        b"GET" => 0,
        b"HEAD" => 1,
        b"POST" => 2,
        b"PUT" => 3,
        b"PATCH" => 4,
        b"DELETE" => 5,
        b"OPTIONS" => 6,
        _ => -1,
    }
}

/// 注册一条路由到 Rust 镜像表
/// path 形如 "/api/users/{id}"；段之间 /，参数段用 {name}
/// handler_index 指向 qi 端 处理器列表 的槽位
/// 返回 0 成功，-1 方法未知，-2 参数路径冲突
#[no_mangle]
pub extern "C" fn qi_web_router_register(
    method_ptr: *const c_char,
    path_ptr: *const c_char,
    handler_index: i64,
) -> i64 {
    if method_ptr.is_null() || path_ptr.is_null() {
        return -1;
    }
    let method = unsafe { CStr::from_ptr(method_ptr).to_bytes() };
    let path = unsafe { CStr::from_ptr(path_ptr).to_bytes() };
    let mi = method_idx(method);
    if mi < 0 {
        return -1;
    }
    let mut router = router().write().unwrap();
    let mut cur: &mut RouteNode = &mut *router;
    for seg in path.split(|&b| b == b'/').filter(|s| !s.is_empty()) {
        if seg.len() >= 2 && seg.first() == Some(&b'{') && seg.last() == Some(&b'}') {
            let name = &seg[1..seg.len() - 1];
            if cur.param_child.is_none() {
                cur.param_child = Some((name.to_vec(), Box::new(RouteNode::new())));
            } else {
                let existing = cur.param_child.as_ref().unwrap().0.as_slice();
                if existing != name {
                    return -2;
                }
            }
            cur = cur.param_child.as_mut().unwrap().1.as_mut();
        } else {
            cur = cur
                .static_children
                .entry(seg.to_vec())
                .or_insert_with(RouteNode::new);
        }
    }
    cur.handlers[mi as usize] = handler_index;
    0
}

/// 匹配结果：有路径命中时返回非 null。qi-web 用 accessor 函数读出来。
/// 注意：handler_index = -1 表示路径命中但方法没注册（→ 405 路径）。
pub struct MatchResult {
    handler_index: i64,
    path_hit: i64,
    /// 参数串（RC 分配，Drop 时 release —— 语义同 RequestParts 字段）
    params: RcStr,
    method_mask: u8,
}

/// 走 Rust 路由表查找。
/// 返回非 null 表示命中（包括 path-only 命中 = 405 候选）；null 表示路径不存在。
#[no_mangle]
pub extern "C" fn qi_web_router_match(
    method_ptr: *const c_char,
    path_ptr: *const c_char,
) -> *mut MatchResult {
    if method_ptr.is_null() || path_ptr.is_null() {
        return std::ptr::null_mut();
    }
    let method = unsafe { CStr::from_ptr(method_ptr).to_bytes() };
    let path = unsafe { CStr::from_ptr(path_ptr).to_bytes() };
    let mi = method_idx(method);

    let router = router().read().unwrap();
    let mut cur: &RouteNode = &*router;
    let mut params: Vec<u8> = Vec::new();
    for seg in path.split(|&b| b == b'/').filter(|s| !s.is_empty()) {
        if let Some(child) = cur.static_children.get(seg) {
            cur = child;
        } else if let Some((name, pchild)) = cur.param_child.as_ref() {
            if !params.is_empty() {
                params.push(b'&');
            }
            params.extend_from_slice(name);
            params.push(b'=');
            params.extend_from_slice(seg);
            cur = pchild.as_ref();
        } else {
            // 路径根本不存在
            return std::ptr::null_mut();
        }
    }
    // 走到底 = 路径命中
    let handler_index = if mi >= 0 {
        cur.handlers[mi as usize]
    } else {
        -1
    };
    let mut method_mask: u8 = 0;
    for i in 0..7 {
        if cur.handlers[i] >= 0 {
            method_mask |= 1u8 << i;
        }
    }
    Box::into_raw(Box::new(MatchResult {
        handler_index,
        path_hit: 1,
        params: RcStr::from_bytes(&params),
        method_mask,
    }))
}

#[no_mangle]
pub extern "C" fn qi_web_match_handler(m: *const MatchResult) -> i64 {
    if m.is_null() {
        return -1;
    }
    unsafe { (*m).handler_index }
}

#[no_mangle]
pub extern "C" fn qi_web_match_path_hit(m: *const MatchResult) -> i64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).path_hit }
}

#[no_mangle]
pub extern "C" fn qi_web_match_params(m: *const MatchResult) -> *const c_char {
    if m.is_null() {
        return empty_cptr();
    }
    unsafe { (*m).params.as_ptr() }
}

/// 方法位掩码：bit i 表示方法 i 是否注册（GET=0..OPTIONS=6）。用于 自动 OPTIONS Allow。
#[no_mangle]
pub extern "C" fn qi_web_match_method_mask(m: *const MatchResult) -> i64 {
    if m.is_null() {
        return 0;
    }
    unsafe { (*m).method_mask as i64 }
}

#[no_mangle]
pub extern "C" fn qi_web_match_free(m: *mut MatchResult) -> i64 {
    if !m.is_null() {
        unsafe {
            drop(Box::from_raw(m));
        }
    }
    0
}

/// 一次 alloc 构建 请求标识文本（替代 qi 端 prefix + "-" + int_to_string(ms) 三步链）
/// prefix 为空 → "qi-{ms}"；否则 "{prefix}-{ms}"
/// 返回 *mut c_char，调用方负责 qi_string_free 释放
#[no_mangle]
pub extern "C" fn qi_web_build_request_id(prefix_ptr: *const c_char, ms: i64) -> *mut c_char {
    let prefix = if prefix_ptr.is_null() {
        b"qi" as &[u8]
    } else {
        unsafe {
            let cs = CStr::from_ptr(prefix_ptr);
            let bytes = cs.to_bytes();
            if bytes.is_empty() {
                b"qi" as &[u8]
            } else {
                bytes
            }
        }
    };
    // 估算容量：prefix + "-" + 20 字节 i64 max
    let mut buf: Vec<u8> = Vec::with_capacity(prefix.len() + 22);
    buf.extend_from_slice(prefix);
    buf.push(b'-');
    let _ = std::io::Write::write_fmt(&mut buf, format_args!("{}", ms));
    // prefix/ms 均不含内部 NUL；rc_cstr 分配（带隐藏 header，qi_string_free 可释放）
    crate::stdlib::qi_str::rc_cstr_from_bytes(&buf)
}

fn fallback_500() -> *const c_char {
    let body = "Internal Server Error";
    let response = format!(
        "HTTP/1.1 500 Internal Server Error\r\n\
         Content-Type: text/plain; charset=utf-8\r\n\
         Connection: close\r\n\
         Content-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    // rc_cstr 分配：qi 侧若走 qi_string_free 正常回收；不 free 也只是原有的
    // "intentional leak" 语义（错误路径，非热路径）
    crate::stdlib::qi_str::rc_cstr_from_string(response) as *const c_char
}
