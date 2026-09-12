//! wasm 上的 HTTP —— 由**宿主**提供，不是运行时自己实现的。
//!
//! 原生的 HTTP 走 reqwest，那棵依赖树（rustls → aws-lc-sys 的 C 构建）在
//! wasm32 上根本编不过，所以 `qi-runtime/wasm` 一直没有 HTTP 符号，用了就是
//! 链接期 `undefined symbol: qi_http_get`。
//!
//! 但浏览器**自己就有网络**。所以这里不实现协议，只声明一个宿主导入，把请求
//! 递出去让宿主（wasi-shim.js）用 fetch / XHR 去发。
//!
//! ## 同步的代价
//!
//! qi 的 HTTP 调用是同步的（`变量 正文: 字符串 = HTTP.获取(网址);`），而浏览器
//! 的 fetch 是异步的。playground 把 wasm 跑在 **Worker** 里，Worker 里同步
//! XMLHttpRequest 是允许的（只在主线程上被废弃），所以宿主那边用同步 XHR。
//! 主线程上跑就没辙 —— 宿主会返回失败而不是假装成功。
//!
//! ## 缓冲区协议
//!
//! 宿主把响应写进调用方给的缓冲区，返回**响应总长度**：
//!   - `0 <= 返回值 <= 容量`：成功，已写入这么多字节
//!   - `返回值 > 容量`：没写，缓冲区不够大，按这个长度重开一块再来一次
//!   - `返回值 < 0`：失败（-1 网络错 / -2 宿主不支持同步请求）
//!
//! 这么设计是为了不让宿主往 wasm 线性内存里分配 —— 分配这件事留在 wasm 这边，
//! 宿主只管往给定区间里写。

use std::ffi::{c_char, CStr};

#[link(wasm_import_module = "qi_host")]
extern "C" {
    /// 见模块头的缓冲区协议。方法名 / URL / 请求体 / 附加头都是 (指针, 字节数)。
    fn qi_host_http_call(
        method_ptr: *const u8,
        method_len: usize,
        url_ptr: *const u8,
        url_len: usize,
        body_ptr: *const u8,
        body_len: usize,
        headers_ptr: *const u8,
        headers_len: usize,
        out_ptr: *mut u8,
        out_cap: usize,
    ) -> i32;

    /// 上一次 qi_host_http_call 的 HTTP 状态码（0 = 没发出去）。
    fn qi_host_http_status() -> i32;

    /// 上一次响应的头，小写键的 JSON 对象，同一套缓冲区协议。
    fn qi_host_http_last_headers(out_ptr: *mut u8, out_cap: usize) -> i32;
}

fn cstr_to_string(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p).to_string_lossy().to_string() }
}

/// 发一次请求，返回响应体。失败返回带 `HTTP错误:` 前缀的说明 —— 与原生一侧
/// 「返回字符串、不抛异常」的形状保持一致（wasm 上本来也没有 尝试/捕获）。
fn send_request(method: &str, url: &str, body: &str, headers: &str) -> String {
    let mut cap: usize = 64 * 1024;
    for _ in 0..3 {
        let mut buf = vec![0u8; cap];
        let n = unsafe {
            qi_host_http_call(
                method.as_ptr(),
                method.len(),
                url.as_ptr(),
                url.len(),
                body.as_ptr(),
                body.len(),
                headers.as_ptr(),
                headers.len(),
                buf.as_mut_ptr(),
                cap,
            )
        };
        if n < 0 {
            return match n {
                -2 => "HTTP错误: 宿主不支持同步请求（浏览器主线程上的 fetch 是异步的，\
                       把 wasm 放进 Worker 里跑）"
                    .to_string(),
                _ => format!("HTTP错误: 请求失败（{} {}）", method, url),
            };
        }
        let n = n as usize;
        if n <= cap {
            buf.truncate(n);
            return String::from_utf8_lossy(&buf).to_string();
        }
        // 不够大，按宿主报的长度再来一次
        cap = n;
    }
    "HTTP错误: 响应太大，三次扩容仍装不下".to_string()
}

macro_rules! verb_without_body {
    ($name:ident, $method:literal) => {
        /// # Safety
        /// `url` 必须是调用方持有的、以 NUL 结尾的 C 字符串。
        #[no_mangle]
        pub unsafe extern "C" fn $name(url: *const c_char) -> *mut c_char {
            let reply = send_request($method, &cstr_to_string(url), "", "");
            crate::stdlib::qi_str::rc_cstr_from_string(reply)
        }
    };
}

macro_rules! verb_with_body {
    ($name:ident, $method:literal) => {
        /// # Safety
        /// `url` / `body` 必须是调用方持有的、以 NUL 结尾的 C 字符串。
        #[no_mangle]
        pub unsafe extern "C" fn $name(url: *const c_char, body: *const c_char) -> *mut c_char {
            let reply = send_request($method, &cstr_to_string(url), &cstr_to_string(body), "");
            crate::stdlib::qi_str::rc_cstr_from_string(reply)
        }
    };
}

verb_without_body!(qi_http_get, "GET");
verb_without_body!(qi_http_delete, "DELETE");
verb_without_body!(qi_http_options, "OPTIONS");

/// `HTTP.请求头(网址)` —— 与原生一致：回 `Status: <状态码>`，失败回 `HTTP错误: …`。
/// HEAD 没有正文，回正文就是回空串，那跟原生对不上。
///
/// # Safety
/// `url` 必须是调用方持有的、以 NUL 结尾的 C 字符串。
#[no_mangle]
pub unsafe extern "C" fn qi_http_head(url: *const c_char) -> *mut c_char {
    let reply = send_request("HEAD", &cstr_to_string(url), "", "");
    let text = if reply.starts_with("HTTP错误:") {
        reply
    } else {
        format!("Status: {}", qi_host_http_status())
    };
    crate::stdlib::qi_str::rc_cstr_from_string(text)
}
verb_with_body!(qi_http_post, "POST");
verb_with_body!(qi_http_put, "PUT");
verb_with_body!(qi_http_patch, "PATCH");

fn last_response_headers() -> serde_json::Value {
    let mut cap: usize = 8 * 1024;
    for _ in 0..3 {
        let mut buf = vec![0u8; cap];
        let n = unsafe { qi_host_http_last_headers(buf.as_mut_ptr(), cap) };
        if n < 0 {
            break;
        }
        let n = n as usize;
        if n <= cap {
            buf.truncate(n);
            return serde_json::from_slice(&buf).unwrap_or_else(|_| serde_json::json!({}));
        }
        cap = n;
    }
    serde_json::json!({})
}

/// `HTTP.请求(方法, 网址, 请求头JSON, 请求体)` —— **参数顺序和返回形状都跟原生
/// 一致**（qi-runtime/src/io/http_ffi.rs 的 qi_http_request）：第三个是请求头
/// JSON 对象，第四个才是请求体；返回 `{"status":…,"headers":{…},"body":"…"}`，
/// 失败时 status 为 0、body 带 `HTTP错误:` 前缀。第一版把这两个参数写反了、
/// 还只回正文 —— tests/wasm/http断言.sh 原生/wasm 对拍当场抓出来。
///
/// # Safety
/// 四个参数都必须是调用方持有的、以 NUL 结尾的 C 字符串。
#[no_mangle]
pub unsafe extern "C" fn qi_http_request(
    method: *const c_char,
    url: *const c_char,
    headers_json: *const c_char,
    body: *const c_char,
) -> *mut c_char {
    let reply = send_request(
        &cstr_to_string(method).to_uppercase(),
        &cstr_to_string(url),
        &cstr_to_string(body),
        &cstr_to_string(headers_json),
    );
    let envelope = if reply.starts_with("HTTP错误:") {
        serde_json::json!({"status": 0, "headers": {}, "body": reply})
    } else {
        serde_json::json!({
            "status": qi_host_http_status(),
            "headers": last_response_headers(),
            "body": reply,
        })
    };
    crate::stdlib::qi_str::rc_cstr_from_string(envelope.to_string())
}

/// `HTTP.获取状态码(网址)` —— 与原生同签名：**收网址、自己发一次 GET**、
/// 返回状态码，失败 -1。（原生也是每调一次真发一次请求，不是复用上一次的。）
///
/// # Safety
/// `url` 必须是调用方持有的、以 NUL 结尾的 C 字符串。
#[no_mangle]
pub unsafe extern "C" fn qi_http_get_status(url: *const c_char) -> i64 {
    if url.is_null() {
        return -1;
    }
    let reply = send_request("GET", &cstr_to_string(url), "", "");
    if reply.starts_with("HTTP错误:") {
        return -1;
    }
    qi_host_http_status() as i64
}
