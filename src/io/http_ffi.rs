//! HTTP 模块 FFI 接口
//!
//! 为 Qi 语言提供 C 接口的 HTTP 客户端操作

use super::http::{HttpClient, HttpMethod, HttpRequest};
use std::collections::HashMap;
use std::ffi::CStr;
use std::io::{Read, Write};
use std::os::raw::c_char;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;

// 全局 HTTP 客户端
static HTTP客户端: OnceLock<Mutex<HttpClient>> = OnceLock::new();

// HTTP 请求池（用于异步请求管理）
static HTTP请求池: OnceLock<Mutex<HashMap<i64, HttpRequest>>> = OnceLock::new();
static 请求句柄计数器: OnceLock<Mutex<i64>> = OnceLock::new();

#[allow(non_snake_case)]
fn 获取HTTP客户端() -> &'static Mutex<HttpClient> {
    HTTP客户端.get_or_init(|| Mutex::new(HttpClient::new()))
}

fn 获取请求池() -> &'static Mutex<HashMap<i64, HttpRequest>> {
    HTTP请求池.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 获取请求句柄计数器() -> &'static Mutex<i64> {
    请求句柄计数器.get_or_init(|| Mutex::new(0))
}

/// 初始化 HTTP 模块
#[no_mangle]
pub extern "C" fn qi_http_init() -> i64 {
    let _客户端 = 获取HTTP客户端();
    1 // 成功
}

/// 真实的阻塞 HTTP 请求（基于 reqwest，编译器已依赖）。
/// 失败时返回 Err(错误信息)，调用方转成字符串返回给 Qi。
#[allow(non_snake_case)]
fn 执行HTTP请求(
    方法: reqwest::Method,
    地址: &str,
    JSON体: Option<String>,
) -> Result<String, String> {
    use reqwest::blocking::Client;
    let 客户端 = Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .map_err(|e| format!("构建客户端失败: {}", e))?;
    let mut 构建器 = 客户端.request(方法, 地址);
    if let Some(体) = JSON体 {
        构建器 = 构建器.header("Content-Type", "application/json").body(体);
    }
    let 响应 = 构建器.send().map_err(|e| format!("请求失败: {}", e))?;
    响应.text().map_err(|e| format!("读取响应失败: {}", e))
}

#[allow(non_snake_case)]
fn 转为C字符串(文本: String) -> *mut c_char {
    crate::stdlib::qi_str::rc_cstr_from_string(文本)
}

/// HTTP GET 请求
/// 返回响应体字符串（需要调用 qi_http_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_http_get(url: *const c_char) -> *mut c_char {
    if url.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();
        match 执行HTTP请求(reqwest::Method::GET, &地址, None) {
            Ok(响应体) => 转为C字符串(响应体),
            Err(错误) => 转为C字符串(format!("HTTP错误: {}", 错误)),
        }
    }
}

/// HTTP POST 请求
/// 返回响应体字符串（需要调用 qi_http_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_http_post(url: *const c_char, body: *const c_char) -> *mut c_char {
    if url.is_null() || body.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();
        let 请求体 = CStr::from_ptr(body).to_string_lossy().to_string();
        match 执行HTTP请求(reqwest::Method::POST, &地址, Some(请求体)) {
            Ok(响应体) => 转为C字符串(响应体),
            Err(错误) => 转为C字符串(format!("HTTP错误: {}", 错误)),
        }
    }
}

/// HTTP PUT 请求
#[no_mangle]
pub extern "C" fn qi_http_put(url: *const c_char, body: *const c_char) -> *mut c_char {
    if url.is_null() || body.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();
        let 请求体 = CStr::from_ptr(body).to_string_lossy().to_string();
        match 执行HTTP请求(reqwest::Method::PUT, &地址, Some(请求体)) {
            Ok(响应体) => 转为C字符串(响应体),
            Err(错误) => 转为C字符串(format!("HTTP错误: {}", 错误)),
        }
    }
}

/// HTTP DELETE 请求
#[no_mangle]
pub extern "C" fn qi_http_delete(url: *const c_char) -> *mut c_char {
    if url.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();
        match 执行HTTP请求(reqwest::Method::DELETE, &地址, None) {
            Ok(响应体) => 转为C字符串(响应体),
            Err(错误) => 转为C字符串(format!("HTTP错误: {}", 错误)),
        }
    }
}

/// HTTP HEAD 请求
/// 返回响应头信息（需要调用 qi_http_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_http_head(url: *const c_char) -> *mut c_char {
    if url.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();

        let mut 请求 = HttpRequest::get(地址);
        请求.method = HttpMethod::Head;

        let 客户端 = 获取HTTP客户端().lock().unwrap();
        match 客户端.execute(请求) {
            Ok(响应) => {
                // HEAD 请求返回状态码和响应头信息
                let 状态信息 = format!("Status: {}", 响应.status_code);
                crate::stdlib::qi_str::rc_cstr_from_string(状态信息)
            }
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// HTTP PATCH 请求
/// 返回响应体字符串（需要调用 qi_http_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_http_patch(url: *const c_char, body: *const c_char) -> *mut c_char {
    if url.is_null() || body.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();
        let 请求体 = CStr::from_ptr(body).to_string_lossy().to_string();
        match 执行HTTP请求(reqwest::Method::PATCH, &地址, Some(请求体)) {
            Ok(响应体) => 转为C字符串(响应体),
            Err(错误) => 转为C字符串(format!("HTTP错误: {}", 错误)),
        }
    }
}

/// HTTP OPTIONS 请求
/// 返回响应体字符串（需要调用 qi_http_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_http_options(url: *const c_char) -> *mut c_char {
    if url.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();
        match 执行HTTP请求(reqwest::Method::OPTIONS, &地址, None) {
            Ok(响应体) => 转为C字符串(响应体),
            Err(错误) => 转为C字符串(format!("HTTP错误: {}", 错误)),
        }
    }
}

/// 通用 HTTP 请求：method, url, 头(JSON对象字符串 {"K":"V"}), body。
/// 返回 JSON 字符串：{"status":200,"headers":{...小写键...},"body":"..."}。
/// SSE/text 体原样放进 body，由调用方（Qi）自行解析 data: 行。
#[no_mangle]
pub extern "C" fn qi_http_request(
    method: *const c_char,
    url: *const c_char,
    headers_json: *const c_char,
    body: *const c_char,
) -> *mut c_char {
    if method.is_null() || url.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        let 方法字符串 = CStr::from_ptr(method).to_string_lossy().to_uppercase();
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();
        let 头文本 = if headers_json.is_null() {
            String::new()
        } else {
            CStr::from_ptr(headers_json).to_string_lossy().to_string()
        };
        let 体文本 = if body.is_null() {
            String::new()
        } else {
            CStr::from_ptr(body).to_string_lossy().to_string()
        };

        let 方法 = match reqwest::Method::from_bytes(方法字符串.as_bytes()) {
            Ok(m) => m,
            Err(_) => reqwest::Method::GET,
        };
        let 结果 = (|| -> Result<serde_json::Value, String> {
            let 客户端 = reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(300))
                .build()
                .map_err(|e| e.to_string())?;
            let mut 构建器 = 客户端.request(方法, &地址);
            // 自定义头
            if !头文本.is_empty() {
                if let Ok(serde_json::Value::Object(m)) =
                    serde_json::from_str::<serde_json::Value>(&头文本)
                {
                    for (k, v) in m {
                        if let Some(vs) = v.as_str() {
                            构建器 = 构建器.header(k, vs);
                        }
                    }
                }
            }
            if !体文本.is_empty() {
                构建器 = 构建器.body(体文本);
            }
            let 响应 = 构建器.send().map_err(|e| e.to_string())?;
            let 状态 = 响应.status().as_u16();
            let mut 头对象 = serde_json::Map::new();
            for (k, v) in 响应.headers().iter() {
                头对象.insert(
                    k.as_str().to_lowercase(),
                    serde_json::Value::String(v.to_str().unwrap_or("").to_string()),
                );
            }
            let 体 = 响应.text().map_err(|e| e.to_string())?;
            Ok(serde_json::json!({"status": 状态, "headers": 头对象, "body": 体}))
        })();
        let 输出 = match 结果 {
            Ok(v) => v.to_string(),
            Err(e) => {
                serde_json::json!({"status":0,"headers":{},"body":format!("HTTP错误: {}", e)})
                    .to_string()
            }
        };
        转为C字符串(输出)
    }
}

/// 创建 HTTP 请求（返回请求句柄）
#[no_mangle]
pub extern "C" fn qi_http_request_create(method: *const c_char, url: *const c_char) -> i64 {
    if method.is_null() || url.is_null() {
        return -1;
    }

    unsafe {
        let 方法名 = CStr::from_ptr(method).to_string_lossy().to_string();
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();

        let 方法 = match 方法名.to_uppercase().as_str() {
            "GET" => HttpMethod::Get,
            "POST" => HttpMethod::Post,
            "PUT" => HttpMethod::Put,
            "DELETE" => HttpMethod::Delete,
            "HEAD" => HttpMethod::Head,
            "PATCH" => HttpMethod::Patch,
            "OPTIONS" => HttpMethod::Options,
            _ => HttpMethod::Get,
        };

        let mut 请求 = HttpRequest::get(地址);
        请求.method = 方法;

        let mut 句柄计数 = 获取请求句柄计数器().lock().unwrap();
        *句柄计数 += 1;
        let 句柄 = *句柄计数;

        let mut 请求池 = 获取请求池().lock().unwrap();
        请求池.insert(句柄, 请求);

        句柄
    }
}

/// 设置请求头
#[no_mangle]
pub extern "C" fn qi_http_request_set_header(
    handle: i64,
    name: *const c_char,
    value: *const c_char,
) -> i64 {
    if name.is_null() || value.is_null() {
        return 0;
    }

    unsafe {
        let 头名称 = CStr::from_ptr(name).to_string_lossy().to_string();
        let 头值 = CStr::from_ptr(value).to_string_lossy().to_string();

        let mut 请求池 = 获取请求池().lock().unwrap();
        if let Some(请求) = 请求池.get_mut(&handle) {
            请求.headers.insert(头名称, 头值);
            1
        } else {
            0
        }
    }
}

/// 设置请求体
#[no_mangle]
pub extern "C" fn qi_http_request_set_body(handle: i64, body: *const c_char) -> i64 {
    if body.is_null() {
        return 0;
    }

    unsafe {
        let 请求体 = CStr::from_ptr(body).to_string_lossy().to_string();

        let mut 请求池 = 获取请求池().lock().unwrap();
        if let Some(请求) = 请求池.get_mut(&handle) {
            请求.body = Some(请求体.into_bytes());
            1
        } else {
            0
        }
    }
}

/// 设置请求超时（毫秒）
#[no_mangle]
pub extern "C" fn qi_http_request_set_timeout(handle: i64, timeout_ms: i64) -> i64 {
    if timeout_ms <= 0 {
        return 0;
    }

    let mut 请求池 = 获取请求池().lock().unwrap();
    if let Some(请求) = 请求池.get_mut(&handle) {
        请求.timeout = Duration::from_millis(timeout_ms as u64);
        1
    } else {
        0
    }
}

/// 执行 HTTP 请求
/// 返回响应体字符串（需要调用 qi_http_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_http_request_execute(handle: i64) -> *mut c_char {
    let mut 请求池 = 获取请求池().lock().unwrap();
    if let Some(请求) = 请求池.remove(&handle) {
        let 客户端 = 获取HTTP客户端().lock().unwrap();
        match 客户端.execute(请求) {
            Ok(响应) => match 响应.body_as_string() {
                Ok(响应体) => crate::stdlib::qi_str::rc_cstr_from_string(响应体),
                Err(_) => std::ptr::null_mut(),
            },
            Err(_) => std::ptr::null_mut(),
        }
    } else {
        std::ptr::null_mut()
    }
}

/// 获取 HTTP 状态码（简化版，返回 200 表示成功）
#[no_mangle]
pub extern "C" fn qi_http_get_status(url: *const c_char) -> i64 {
    if url.is_null() {
        return -1;
    }

    unsafe {
        let 地址 = CStr::from_ptr(url).to_string_lossy().to_string();
        let 请求 = HttpRequest::get(地址);

        let 客户端 = 获取HTTP客户端().lock().unwrap();
        match 客户端.execute(请求) {
            Ok(响应) => 响应.status_code as i64,
            Err(_) => -1,
        }
    }
}

/// 释放 HTTP 响应字符串（委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃）
#[no_mangle]
pub extern "C" fn qi_http_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_http_init() {
        let result = qi_http_init();
        assert_eq!(result, 1);
    }

    #[test]
    #[ignore] // 需要联网：qi_http_get 现已是真实 HTTP 请求（不再是 Hello,World 模拟）
    fn test_http_get() {
        qi_http_init();

        let url = CString::new("https://example.com").unwrap();
        let response = qi_http_get(url.as_ptr());

        assert!(!response.is_null());
        let response_str = unsafe { CStr::from_ptr(response).to_string_lossy().into_owned() };
        qi_http_free_string(response);
        // 真实响应应是 example.com 的页面内容
        assert!(
            response_str.contains("Example Domain"),
            "got: {}",
            &response_str[..response_str.len().min(120)]
        );
    }

    #[test]
    fn test_http_request_builder() {
        qi_http_init();

        let method = CString::new("POST").unwrap();
        let url = CString::new("https://api.example.com").unwrap();
        let handle = qi_http_request_create(method.as_ptr(), url.as_ptr());
        assert!(handle > 0);

        let header_name = CString::new("Content-Type").unwrap();
        let header_value = CString::new("application/json").unwrap();
        let result =
            qi_http_request_set_header(handle, header_name.as_ptr(), header_value.as_ptr());
        assert_eq!(result, 1);

        let body = CString::new("{\"test\":\"data\"}").unwrap();
        let result = qi_http_request_set_body(handle, body.as_ptr());
        assert_eq!(result, 1);

        let result = qi_http_request_set_timeout(handle, 5000);
        assert_eq!(result, 1);
    }
}

// ==================== HTTP 服务器功能 ====================

use std::net::TcpListener;

// HTTP 服务器池
static HTTP服务器池: OnceLock<Mutex<HashMap<i64, TcpListener>>> = OnceLock::new();
static 服务器句柄计数器: OnceLock<Mutex<i64>> = OnceLock::new();

fn 获取服务器池() -> &'static Mutex<HashMap<i64, TcpListener>> {
    HTTP服务器池.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 获取服务器句柄计数器() -> &'static Mutex<i64> {
    服务器句柄计数器.get_or_init(|| Mutex::new(0))
}

/// 创建 HTTP 服务器并开始监听
/// 返回服务器句柄（<0 表示错误）
#[no_mangle]
pub extern "C" fn qi_http_server_create(host: *const c_char, port: i64) -> i64 {
    if host.is_null() {
        return -1;
    }

    unsafe {
        let 主机 = CStr::from_ptr(host).to_string_lossy().to_string();
        let 端口 = port as u16; // Convert i64 to u16 for TCP port
        let 地址 = format!("{}:{}", 主机, 端口);

        match TcpListener::bind(&地址) {
            Ok(监听器) => {
                let mut 计数器 = 获取服务器句柄计数器().lock().unwrap();
                *计数器 += 1;
                let 句柄 = *计数器;

                let mut 服务器池 = 获取服务器池().lock().unwrap();
                服务器池.insert(句柄, 监听器);

                句柄
            }
            Err(_) => -1,
        }
    }
}

/// 接受客户端连接并返回请求信息（阻塞）
/// 返回格式："METHOD /path HTTP/1.1\nHeader: value\n\nbody"
/// 返回字符串需要调用 qi_http_free_string 释放
#[no_mangle]
pub extern "C" fn qi_http_server_accept(server_handle: i64) -> *mut c_char {
    let mut 服务器池 = 获取服务器池().lock().unwrap();

    if let Some(监听器) = 服务器池.get_mut(&server_handle) {
        match 监听器.accept() {
            Ok((mut 流, _地址)) => {
                // 读取HTTP请求
                let mut 缓冲 = vec![0u8; 8192];
                match 流.read(&mut 缓冲) {
                    Ok(大小) => {
                        if 大小 > 0 {
                            if let Ok(请求文本) = String::from_utf8(缓冲[..大小].to_vec()) {
                                // 返回完整的HTTP请求文本
                                return crate::stdlib::qi_str::rc_cstr_from_string(请求文本);
                            }
                        }
                    }
                    Err(_) => {}
                }
            }
            Err(_) => {}
        }
    }

    std::ptr::null_mut()
}

/// 接受客户端连接并处理（带响应）
/// 返回格式："METHOD|/path|body" （使用|分隔）
/// 响应会自动发送
#[no_mangle]
pub extern "C" fn qi_http_server_handle_request(
    server_handle: i64,
    response_body: *const c_char,
    status_code: i64,
) -> *mut c_char {
    if response_body.is_null() {
        return std::ptr::null_mut();
    }

    let mut 服务器池 = 获取服务器池().lock().unwrap();

    if let Some(监听器) = 服务器池.get_mut(&server_handle) {
        match 监听器.accept() {
            Ok((mut 流, _地址)) => {
                // 读取HTTP请求
                let mut 缓冲 = vec![0u8; 8192];
                let 请求信息 = match 流.read(&mut 缓冲) {
                    Ok(大小) if 大小 > 0 => {
                        if let Ok(请求文本) = String::from_utf8(缓冲[..大小].to_vec()) {
                            // 解析请求行
                            if let Some(首行) = 请求文本.lines().next() {
                                let 部分: Vec<&str> = 首行.split_whitespace().collect();
                                if 部分.len() >= 2 {
                                    let 方法 = 部分[0];
                                    let 路径 = 部分[1];

                                    // 查找请求体（在空行之后）
                                    let 请求体 = if let Some(位置) = 请求文本.find("\r\n\r\n")
                                    {
                                        请求文本[位置 + 4..].to_string()
                                    } else if let Some(位置) = 请求文本.find("\n\n") {
                                        请求文本[位置 + 2..].to_string()
                                    } else {
                                        String::new()
                                    };

                                    Some(format!("{}|{}|{}", 方法, 路径, 请求体))
                                } else {
                                    None
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    }
                    _ => None,
                };

                // 发送响应
                unsafe {
                    let 响应体 = CStr::from_ptr(response_body).to_string_lossy();
                    let 状态文本 = match status_code {
                        200 => "OK",
                        201 => "Created",
                        400 => "Bad Request",
                        404 => "Not Found",
                        500 => "Internal Server Error",
                        _ => "Unknown",
                    };

                    let 响应 = format!(
                        "HTTP/1.1 {} {}\r\nContent-Type: text/plain; charset=utf-8\r\nContent-Length: {}\r\n\r\n{}",
                        status_code,
                        状态文本,
                        响应体.len(),
                        响应体
                    );

                    let _ = 流.write_all(响应.as_bytes());
                    let _ = 流.flush();
                }

                // 返回请求信息
                if let Some(信息) = 请求信息 {
                    return crate::stdlib::qi_str::rc_cstr_from_string(信息);
                }
            }
            Err(_) => {}
        }
    }

    std::ptr::null_mut()
}

/// 发送HTTP响应到客户端
/// 简化版本：只需要服务器句柄和响应内容
#[no_mangle]
pub extern "C" fn qi_http_server_send_response(
    server_handle: i64,
    response_body: *const c_char,
) -> i64 {
    if response_body.is_null() {
        return -1;
    }

    // 这个函数需要与 accept 配合使用
    // 实际场景中，我们需要保存每个连接的流
    // 这里返回成功，实际发送在 handle_request 中完成
    1
}

/// 关闭 HTTP 服务器
/// 返回 1 成功，0 失败
#[no_mangle]
pub extern "C" fn qi_http_server_close(server_handle: i64) -> i64 {
    let mut 服务器池 = 获取服务器池().lock().unwrap();
    if 服务器池.remove(&server_handle).is_some() {
        1
    } else {
        0
    }
}
