//! MCP 服务器模块 FFI 接口
//!
//! 为 Qi 语言提供 C 接口的 MCP 服务器调用函数

#![allow(non_snake_case)]

use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

use super::mcp::{
    MCP工具, MCP提示, MCP服务器, MCP服务器模块, MCP服务器配置, MCP资源, 工具参数, 资源类型,
};

// MCP 服务器池
static MCP服务器池: OnceLock<Mutex<HashMap<i64, MCP服务器>>> = OnceLock::new();
static 服务器计数器: OnceLock<Mutex<i64>> = OnceLock::new();

// ─────────────────────────────────────────────────────────────────────────────
// P2: 服务器→客户端推送通道
// ─────────────────────────────────────────────────────────────────────────────

/// stdio 写锁：serve 循环运行时安装，通知函数从此写通知行。
/// 保护 stdout 不被并发写穿插。
static STDIO_WRITER: OnceLock<Mutex<Option<Box<dyn Write + Send>>>> = OnceLock::new();

fn 获取stdio写锁() -> &'static Mutex<Option<Box<dyn Write + Send>>> {
    STDIO_WRITER.get_or_init(|| Mutex::new(None))
}

/// HTTP SSE 推送通道注册表：session_id → mpsc::Sender<String>
/// 客户端 GET /mcp 时注册；连接断开时移除。
static HTTP_SSE_CHANNELS: OnceLock<Mutex<HashMap<String, mpsc::Sender<String>>>> = OnceLock::new();

fn 获取sse通道注册表() -> &'static Mutex<HashMap<String, mpsc::Sender<String>>> {
    HTTP_SSE_CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 向 stdio 客户端发送一条 JSON-RPC 通知（无 id）
fn 发送stdio通知(notification: &JsonValue) -> bool {
    let line = notification.to_string();
    let mut guard = 获取stdio写锁().lock().unwrap();
    if let Some(w) = guard.as_mut() {
        if writeln!(w, "{}", line).is_ok() {
            let _ = w.flush();
            eprintln!(
                "[qi-mcp-notify] stdio 通知已发送: {}",
                &line[..line.len().min(120)]
            );
            return true;
        }
    }
    eprintln!("[qi-mcp-notify] stdio 写锁无活跃会话，通知丢弃");
    false
}

/// 向所有活跃 SSE 会话推送一条通知
fn 发送http通知(notification: &JsonValue) {
    let line = notification.to_string();
    let sse_data = format!("event: message\ndata: {}\n\n", line);
    let mut reg = 获取sse通道注册表().lock().unwrap();
    let mut dead: Vec<String> = Vec::new();
    for (sid, tx) in reg.iter() {
        if tx.send(sse_data.clone()).is_err() {
            dead.push(sid.clone());
        } else {
            eprintln!("[qi-mcp-notify] HTTP SSE 通知已推送至会话 {}", sid);
        }
    }
    for sid in dead {
        reg.remove(&sid);
    }
}

/// 向所有传输推送通知
fn 广播通知(notification: &JsonValue) {
    发送stdio通知(notification);
    发送http通知(notification);
}

fn 获取服务器池() -> &'static Mutex<HashMap<i64, MCP服务器>> {
    MCP服务器池.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 获取服务器计数器() -> &'static Mutex<i64> {
    服务器计数器.get_or_init(|| Mutex::new(0))
}

/// 创建MCP服务器
///
/// 参数:
/// - name: 服务器名称
/// - version: 服务器版本
/// - description: 服务器描述 (可选，传入空字符串表示无描述)
///
/// 返回: 服务器句柄 (>0 成功, <0 失败)
#[no_mangle]
pub extern "C" fn qi_mcp_create_server(
    name: *const c_char,
    version: *const c_char,
    description: *const c_char,
) -> i64 {
    if name.is_null() || version.is_null() {
        return -1;
    }

    unsafe {
        let 名称 = CStr::from_ptr(name).to_string_lossy().to_string();
        let 版本 = CStr::from_ptr(version).to_string_lossy().to_string();
        let 描述 = if description.is_null() || CStr::from_ptr(description).to_bytes().is_empty() {
            None
        } else {
            Some(CStr::from_ptr(description).to_string_lossy().to_string())
        };

        let 配置 = MCP服务器配置 {
            名称,
            版本,
            描述,
            协议版本: "2025-06-18".to_string(),
        };

        let 模块 = MCP服务器模块::创建();
        let 服务器 = 模块.创建服务器(Some(配置));

        // 生成新的服务器ID
        let mut 计数器 = 获取服务器计数器().lock().unwrap();
        *计数器 += 1;
        let 服务器ID = *计数器;

        // 存储服务器
        let mut 服务器池 = 获取服务器池().lock().unwrap();
        服务器池.insert(服务器ID, 服务器);

        服务器ID
    }
}

/// 注册工具到MCP服务器
///
/// 参数:
/// - server_id: 服务器句柄
/// - tool_name: 工具名称
/// - tool_description: 工具描述
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_register_tool(
    server_id: i64,
    tool_name: *const c_char,
    tool_description: *const c_char,
) -> i32 {
    if tool_name.is_null() || tool_description.is_null() {
        return -1;
    }

    unsafe {
        let 名称 = CStr::from_ptr(tool_name).to_string_lossy().to_string();
        let 描述 = CStr::from_ptr(tool_description)
            .to_string_lossy()
            .to_string();

        let 工具 = MCP工具::创建(名称, 描述);

        let mut 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get_mut(&server_id) {
            match 服务器.注册工具(工具) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// 添加工具参数
///
/// 参数:
/// - server_id: 服务器句柄
/// - tool_name: 工具名称
/// - param_name: 参数名称
/// - param_type: 参数类型 ("string", "number", "boolean", "object", "array")
/// - param_description: 参数描述
/// - required: 是否必需 (1=必需, 0=可选)
///
/// 返回: 0 成功, -1 失败
///
/// 注意: 必须先调用 qi_mcp_register_tool 注册工具，再调用此函数添加参数
#[no_mangle]
pub extern "C" fn qi_mcp_add_tool_parameter(
    server_id: i64,
    tool_name: *const c_char,
    param_name: *const c_char,
    param_type: *const c_char,
    param_description: *const c_char,
    required: i32,
) -> i32 {
    if tool_name.is_null()
        || param_name.is_null()
        || param_type.is_null()
        || param_description.is_null()
    {
        return -1;
    }

    unsafe {
        let 工具名 = CStr::from_ptr(tool_name).to_string_lossy().to_string();
        let 参数名 = CStr::from_ptr(param_name).to_string_lossy().to_string();
        let 参数类型 = CStr::from_ptr(param_type).to_string_lossy().to_string();
        let 参数描述 = CStr::from_ptr(param_description)
            .to_string_lossy()
            .to_string();
        let 是否必需 = required != 0;

        let 参数 = 工具参数::创建(参数名, 参数类型, 参数描述, 是否必需);

        let mut 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get_mut(&server_id) {
            match 服务器.为工具添加参数(&工具名, 参数) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// 注册资源到MCP服务器
///
/// 参数:
/// - server_id: 服务器句柄
/// - resource_uri: 资源URI
/// - resource_name: 资源名称
/// - resource_description: 资源描述
/// - resource_type: 资源类型 (0=文本, 1=二进制, 2=JSON)
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_register_resource(
    server_id: i64,
    resource_uri: *const c_char,
    resource_name: *const c_char,
    resource_description: *const c_char,
    resource_type: i32,
) -> i32 {
    if resource_uri.is_null() || resource_name.is_null() || resource_description.is_null() {
        return -1;
    }

    unsafe {
        let uri = CStr::from_ptr(resource_uri).to_string_lossy().to_string();
        let 名称 = CStr::from_ptr(resource_name).to_string_lossy().to_string();
        let 描述 = CStr::from_ptr(resource_description)
            .to_string_lossy()
            .to_string();

        let 类型 = match resource_type {
            0 => 资源类型::文本,
            1 => 资源类型::二进制,
            2 => 资源类型::JSON,
            _ => return -1,
        };

        let 资源 = MCP资源::创建(uri, 名称, 描述, 类型);

        let mut 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get_mut(&server_id) {
            match 服务器.注册资源(资源) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// 注册提示到MCP服务器
///
/// 参数:
/// - server_id: 服务器句柄
/// - prompt_name: 提示名称
/// - prompt_description: 提示描述
/// - prompt_template: 提示模板 (使用 {变量名} 作为占位符)
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_register_prompt(
    server_id: i64,
    prompt_name: *const c_char,
    prompt_description: *const c_char,
    prompt_template: *const c_char,
) -> i32 {
    if prompt_name.is_null() || prompt_description.is_null() || prompt_template.is_null() {
        return -1;
    }

    unsafe {
        let 名称 = CStr::from_ptr(prompt_name).to_string_lossy().to_string();
        let 描述 = CStr::from_ptr(prompt_description)
            .to_string_lossy()
            .to_string();
        let 模板 = CStr::from_ptr(prompt_template)
            .to_string_lossy()
            .to_string();

        let 提示 = MCP提示::创建(名称, 描述, 模板);

        let mut 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get_mut(&server_id) {
            match 服务器.注册提示(提示) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// 启动MCP服务器
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_start_server(server_id: i64) -> i32 {
    let mut 服务器池 = 获取服务器池().lock().unwrap();
    if let Some(服务器) = 服务器池.get_mut(&server_id) {
        match 服务器.启动() {
            Ok(_) => 0,
            Err(_) => -1,
        }
    } else {
        -1
    }
}

/// 停止MCP服务器
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_stop_server(server_id: i64) -> i32 {
    let mut 服务器池 = 获取服务器池().lock().unwrap();
    if let Some(服务器) = 服务器池.get_mut(&server_id) {
        match 服务器.停止() {
            Ok(_) => 0,
            Err(_) => -1,
        }
    } else {
        -1
    }
}

/// 获取服务器信息 (JSON格式)
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: JSON字符串 (需要调用 qi_mcp_free_string 释放), NULL 失败
#[no_mangle]
pub extern "C" fn qi_mcp_get_server_info(server_id: i64) -> *mut c_char {
    let 服务器池 = 获取服务器池().lock().unwrap();
    if let Some(服务器) = 服务器池.get(&server_id) {
        let 信息 = 服务器.获取服务器信息();
        let json_str = 信息.to_string();
        match CString::new(json_str) {
            Ok(c_str) => c_str.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    } else {
        std::ptr::null_mut()
    }
}

/// 获取工具列表 (JSON格式)
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: JSON字符串 (需要调用 qi_mcp_free_string 释放), NULL 失败
#[no_mangle]
pub extern "C" fn qi_mcp_list_tools(server_id: i64) -> *mut c_char {
    let 服务器池 = 获取服务器池().lock().unwrap();
    if let Some(服务器) = 服务器池.get(&server_id) {
        let 工具列表 = 服务器.获取工具列表();
        let json_str = json!(工具列表).to_string();
        match CString::new(json_str) {
            Ok(c_str) => c_str.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    } else {
        std::ptr::null_mut()
    }
}

/// 获取资源列表 (JSON格式)
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: JSON字符串 (需要调用 qi_mcp_free_string 释放), NULL 失败
#[no_mangle]
pub extern "C" fn qi_mcp_list_resources(server_id: i64) -> *mut c_char {
    let 服务器池 = 获取服务器池().lock().unwrap();
    if let Some(服务器) = 服务器池.get(&server_id) {
        let 资源列表 = 服务器.获取资源列表();
        let json_str = json!(资源列表).to_string();
        match CString::new(json_str) {
            Ok(c_str) => c_str.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    } else {
        std::ptr::null_mut()
    }
}

/// 获取提示列表 (JSON格式)
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: JSON字符串 (需要调用 qi_mcp_free_string 释放), NULL 失败
#[no_mangle]
pub extern "C" fn qi_mcp_list_prompts(server_id: i64) -> *mut c_char {
    let 服务器池 = 获取服务器池().lock().unwrap();
    if let Some(服务器) = 服务器池.get(&server_id) {
        let 提示列表 = 服务器.获取提示列表();
        let json_str = json!(提示列表).to_string();
        match CString::new(json_str) {
            Ok(c_str) => c_str.into_raw(),
            Err(_) => std::ptr::null_mut(),
        }
    } else {
        std::ptr::null_mut()
    }
}

/// 执行工具
///
/// 参数:
/// - server_id: 服务器句柄
/// - tool_name: 工具名称
/// - params_json: 参数 JSON 字符串
///
/// 返回: 执行结果 JSON 字符串 (需要调用 qi_mcp_free_string 释放), NULL 失败
#[no_mangle]
pub extern "C" fn qi_mcp_call_tool(
    server_id: i64,
    tool_name: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    if tool_name.is_null() || params_json.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 工具名 = CStr::from_ptr(tool_name).to_string_lossy().to_string();
        let json_str = CStr::from_ptr(params_json).to_string_lossy().to_string();

        // 解析参数JSON
        let 参数: HashMap<String, JsonValue> = match serde_json::from_str(&json_str) {
            Ok(params) => params,
            Err(_) => return std::ptr::null_mut(),
        };

        let 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get(&server_id) {
            match 服务器.执行工具(&工具名, &参数) {
                Ok(结果) => {
                    let 结果字符串 = 结果.to_string();
                    match CString::new(结果字符串) {
                        Ok(c_str) => c_str.into_raw(),
                        Err(_) => std::ptr::null_mut(),
                    }
                }
                Err(_) => std::ptr::null_mut(),
            }
        } else {
            std::ptr::null_mut()
        }
    }
}

/// 填充提示模板
///
/// 参数:
/// - server_id: 服务器句柄
/// - prompt_name: 提示名称
/// - params_json: 参数 JSON 字符串 (键值对)
///
/// 返回: 填充后的提示文本 (需要调用 qi_mcp_free_string 释放), NULL 失败
#[no_mangle]
pub extern "C" fn qi_mcp_get_prompt(
    server_id: i64,
    prompt_name: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    if prompt_name.is_null() || params_json.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 提示名 = CStr::from_ptr(prompt_name).to_string_lossy().to_string();
        let json_str = CStr::from_ptr(params_json).to_string_lossy().to_string();

        // 解析参数JSON
        let 参数: HashMap<String, String> = match serde_json::from_str(&json_str) {
            Ok(params) => params,
            Err(_) => return std::ptr::null_mut(),
        };

        let 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get(&server_id) {
            match 服务器.获取提示(&提示名) {
                Ok(提示) => match 提示.填充(&参数) {
                    Ok(结果文本) => match CString::new(结果文本) {
                        Ok(c_str) => c_str.into_raw(),
                        Err(_) => std::ptr::null_mut(),
                    },
                    Err(_) => std::ptr::null_mut(),
                },
                Err(_) => std::ptr::null_mut(),
            }
        } else {
            std::ptr::null_mut()
        }
    }
}

/// 检查服务器是否正在运行
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: 1 运行中, 0 未运行, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_is_running(server_id: i64) -> i32 {
    let 服务器池 = 获取服务器池().lock().unwrap();
    if let Some(服务器) = 服务器池.get(&server_id) {
        if 服务器.是否运行中() {
            1
        } else {
            0
        }
    } else {
        -1
    }
}

/// 释放MCP服务器
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_destroy_server(server_id: i64) -> i32 {
    let mut 服务器池 = 获取服务器池().lock().unwrap();
    if 服务器池.remove(&server_id).is_some() {
        0
    } else {
        -1
    }
}

/// 设置资源文本内容
///
/// 参数:
/// - server_id: 服务器句柄
/// - resource_uri: 资源URI
/// - content: 文本内容
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_set_resource_text_content(
    server_id: i64,
    resource_uri: *const c_char,
    content: *const c_char,
) -> i32 {
    if resource_uri.is_null() || content.is_null() {
        return -1;
    }

    unsafe {
        let uri = CStr::from_ptr(resource_uri).to_string_lossy().to_string();
        let 内容 = CStr::from_ptr(content).to_string_lossy().to_string();

        let mut 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get_mut(&server_id) {
            match 服务器.设置资源文本内容(&uri, 内容) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// 设置资源JSON内容
///
/// 参数:
/// - server_id: 服务器句柄
/// - resource_uri: 资源URI
/// - json_content: JSON字符串内容
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_set_resource_json_content(
    server_id: i64,
    resource_uri: *const c_char,
    json_content: *const c_char,
) -> i32 {
    if resource_uri.is_null() || json_content.is_null() {
        return -1;
    }

    unsafe {
        let uri = CStr::from_ptr(resource_uri).to_string_lossy().to_string();
        let json_str = CStr::from_ptr(json_content).to_string_lossy().to_string();

        // 解析JSON
        let json_value = match serde_json::from_str(&json_str) {
            Ok(v) => v,
            Err(_) => return -1,
        };

        let mut 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get_mut(&server_id) {
            match 服务器.设置资源JSON内容(&uri, json_value) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// 读取资源文本内容
///
/// 参数:
/// - server_id: 服务器句柄
/// - resource_uri: 资源URI
///
/// 返回: 文本内容的C字符串指针，失败返回NULL
/// 注意: 调用者需要使用 qi_mcp_free_string 释放返回的字符串
#[no_mangle]
pub extern "C" fn qi_mcp_read_resource_text(
    server_id: i64,
    resource_uri: *const c_char,
) -> *mut c_char {
    if resource_uri.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let uri = CStr::from_ptr(resource_uri).to_string_lossy().to_string();

        let 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get(&server_id) {
            match 服务器.读取资源文本(&uri) {
                Ok(text) => CString::new(text)
                    .unwrap_or_else(|_| CString::new("").unwrap())
                    .into_raw(),
                Err(_) => std::ptr::null_mut(),
            }
        } else {
            std::ptr::null_mut()
        }
    }
}

/// 读取资源JSON内容
///
/// 参数:
/// - server_id: 服务器句柄
/// - resource_uri: 资源URI
///
/// 返回: JSON内容的C字符串指针，失败返回NULL
/// 注意: 调用者需要使用 qi_mcp_free_string 释放返回的字符串
#[no_mangle]
pub extern "C" fn qi_mcp_read_resource_json(
    server_id: i64,
    resource_uri: *const c_char,
) -> *mut c_char {
    if resource_uri.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let uri = CStr::from_ptr(resource_uri).to_string_lossy().to_string();

        let 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get(&server_id) {
            match 服务器.读取资源JSON(&uri) {
                Ok(json) => {
                    let json_str = json.to_string();
                    CString::new(json_str)
                        .unwrap_or_else(|_| CString::new("{}").unwrap())
                        .into_raw()
                }
                Err(_) => std::ptr::null_mut(),
            }
        } else {
            std::ptr::null_mut()
        }
    }
}

/// 设置工具回调ID
///
/// 参数:
/// - server_id: 服务器句柄
/// - tool_name: 工具名称
/// - callback_id: 回调标识符
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_set_tool_callback(
    server_id: i64,
    tool_name: *const c_char,
    callback_id: *const c_char,
) -> i32 {
    if tool_name.is_null() || callback_id.is_null() {
        return -1;
    }

    unsafe {
        let 工具名 = CStr::from_ptr(tool_name).to_string_lossy().to_string();
        let 回调ID = CStr::from_ptr(callback_id).to_string_lossy().to_string();

        let mut 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get_mut(&server_id) {
            match 服务器.设置工具回调ID(&工具名, 回调ID) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// 设置工具回调闭包对象指针 (Qi closure 对象版本)
///
/// 参数:
/// - server_id: 服务器句柄
/// - tool_name: 工具名称
/// - closure_obj: Qi 闭包对象指针 (布局: [fn_ptr, env_slots...])
///   调用时: fn_ptr(closure_obj, args_json) → result_str
///
/// 返回: 0 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_mcp_set_tool_callback_ptr(
    server_id: i64,
    tool_name: *const c_char,
    closure_obj: *const std::ffi::c_void,
) -> i32 {
    if tool_name.is_null() || closure_obj.is_null() {
        return -1;
    }

    unsafe {
        let 工具名 = CStr::from_ptr(tool_name).to_string_lossy().to_string();
        let ptr_val = closure_obj as usize;

        let mut 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get_mut(&server_id) {
            match 服务器.设置工具回调指针(&工具名, ptr_val) {
                Ok(_) => 0,
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// JSON-RPC 2.0 stdio 服务器主循环
///
/// 从 stdin 读 newline-delimited JSON-RPC 请求，处理后写到 stdout。
/// 所有诊断信息发往 stderr，stdout 仅走协议。
/// 阻塞直到 stdin EOF。
///
/// 参数:
/// - server_id: 服务器句柄 (需已通过 qi_mcp_create_server 创建)
///
/// 返回: 0 正常退出, -1 服务器不存在
#[no_mangle]
pub extern "C" fn qi_mcp_serve_stdio(server_id: i64) -> i32 {
    // 验证服务器存在
    {
        let 服务器池 = 获取服务器池().lock().unwrap();
        if !服务器池.contains_key(&server_id) {
            eprintln!("[qi-mcp] 服务器 {} 不存在", server_id);
            return -1;
        }
    }

    eprintln!("[qi-mcp] stdio 服务器启动 (server_id={})", server_id);

    // P2: 将 stdout 安装到推送写锁中，供 qi_mcp_notify_* 使用。
    // 注意：BufWriter<StdoutLock> 无法 'static；改为直接用 Arc<Mutex<Stdout>>
    // 以便工具回调内（同线程）也能写通知。
    // 实现：在 serve 循环本体里，所有写操作都通过 STDIO_WRITER 锁完成，
    // 这样工具回调（也在此线程）同样拿锁写通知，不会穿插。
    {
        let mut w = 获取stdio写锁().lock().unwrap();
        // 使用一个 channel 方式：写入 BufWriter 包装 stdout
        // 因为 StdoutLock 有生命期限制，使用 std::io::stdout()（没有锁，每次flush前拿临时锁）
        *w = Some(Box::new(std::io::stdout()));
    }

    let stdin = std::io::stdin();
    let mut reader = std::io::BufReader::new(stdin.lock());

    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break, // EOF
            Ok(_) => {}
            Err(e) => {
                eprintln!("[qi-mcp] stdin 读取错误: {}", e);
                break;
            }
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // 解析 JSON-RPC 2.0
        let req: JsonValue = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("[qi-mcp] JSON 解析错误: {}", e);
                let err_resp = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {"code": -32700, "message": format!("Parse error: {}", e)}
                });
                let err_str = err_resp.to_string();
                {
                    let mut w = 获取stdio写锁().lock().unwrap();
                    if let Some(writer) = w.as_mut() {
                        let _ = writeln!(writer, "{}", err_str);
                        let _ = writer.flush();
                    }
                }
                continue;
            }
        };

        // 提取 id — notifications 无 id
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));

        eprintln!("[qi-mcp] 收到请求 method={} id={:?}", method, id);

        // notifications (no id): 处理 notifications/cancelled 等, 静默忽略其他
        if id.is_none() {
            match method {
                "notifications/cancelled" => {
                    // client 取消了一个请求; 服务器忽略即可
                    eprintln!("[qi-mcp] 收到 notifications/cancelled, 忽略");
                }
                "notifications/initialized" => {
                    eprintln!("[qi-mcp] 客户端已初始化");
                }
                _ => {
                    eprintln!("[qi-mcp] 收到通知 method={}, 忽略", method);
                }
            }
            continue;
        }

        let response = handle_request(server_id, method, &params, &id);

        if let Some(resp) = response {
            let resp_str = resp.to_string();
            let mut w = 获取stdio写锁().lock().unwrap();
            if let Some(writer) = w.as_mut() {
                if let Err(e) = writeln!(writer, "{}", resp_str) {
                    eprintln!("[qi-mcp] stdout 写入错误: {}", e);
                    break;
                }
                if let Err(e) = writer.flush() {
                    eprintln!("[qi-mcp] stdout flush 错误: {}", e);
                    break;
                }
            }
            eprintln!("[qi-mcp] 已响应 method={}", method);
        }
    }

    // 清理写锁
    {
        let mut w = 获取stdio写锁().lock().unwrap();
        *w = None;
    }

    eprintln!("[qi-mcp] stdio 服务器正常退出");
    0
}

// ─────────────────────────────────────────────────────────────────────────────
// Streamable HTTP transport  (POST /mcp + Mcp-Session-Id + SSE response)
// ─────────────────────────────────────────────────────────────────────────────

/// 全局会话 ID 计数器（atomic，无需锁）
static HTTP_SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// 生成唯一会话 ID（不依赖 uuid crate）
fn 生成会话id() -> String {
    let n = HTTP_SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    format!("mcp-{:x}-{:x}", ts, n)
}

/// 从原始 HTTP 请求字节流中解析出请求体
/// 返回 (method, path, body_bytes)
fn parse_http_request(buf: &[u8]) -> Option<(String, String, Vec<u8>)> {
    // 找头/体分割
    let header_end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| (p, p + 4))
        .or_else(|| {
            buf.windows(2)
                .position(|w| w == b"\n\n")
                .map(|p| (p, p + 2))
        });

    let (header_len, body_start) = header_end?;
    let header_str = std::str::from_utf8(&buf[..header_len]).ok()?;

    let first_line = header_str.lines().next()?;
    let mut parts = first_line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    // Content-Length
    let content_length: usize = header_str
        .lines()
        .find(|l| l.to_lowercase().starts_with("content-length:"))
        .and_then(|l| l.splitn(2, ':').nth(1))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);

    let body_available = &buf[body_start..];
    let body = if content_length > 0 {
        body_available[..content_length.min(body_available.len())].to_vec()
    } else {
        body_available.to_vec()
    };

    // Extract Mcp-Session-Id header if present
    Some((method, path, body))
}

/// 从 HTTP 请求头文本中提取某个请求头的值（不区分大小写）
fn extract_header<'a>(header_text: &'a str, name: &str) -> Option<&'a str> {
    let lower_name = name.to_lowercase();
    for line in header_text.lines().skip(1) {
        if let Some(colon) = line.find(':') {
            let key = line[..colon].trim().to_lowercase();
            if key == lower_name {
                return Some(line[colon + 1..].trim());
            }
        }
    }
    None
}

/// 从原始 HTTP 请求字节中提取头部文本
fn extract_headers_text(buf: &[u8]) -> String {
    let end = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .or_else(|| buf.windows(2).position(|w| w == b"\n\n"))
        .unwrap_or(buf.len());
    std::str::from_utf8(&buf[..end]).unwrap_or("").to_string()
}

/// 发送 HTTP 响应（SSE 格式）
fn send_sse_response(stream: &mut TcpStream, status: u16, session_id: &str, sse_body: &str) {
    let response = format!(
        "HTTP/1.1 {} {}\r\n\
         Content-Type: text/event-stream\r\n\
         Cache-Control: no-cache\r\n\
         Mcp-Session-Id: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Content-Length: {}\r\n\
         \r\n\
         {}",
        status,
        if status == 200 { "OK" } else { "Accepted" },
        session_id,
        sse_body.len(),
        sse_body
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// 发送空 202 响应（用于 notifications，无响应体）
fn send_202(stream: &mut TcpStream, session_id: &str) {
    let response = format!(
        "HTTP/1.1 202 Accepted\r\n\
         Content-Length: 0\r\n\
         Mcp-Session-Id: {}\r\n\
         \r\n",
        session_id
    );
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.flush();
}

/// 发送 405 Method Not Allowed
fn send_405(stream: &mut TcpStream) {
    let _ = stream.write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n");
    let _ = stream.flush();
}

/// 发送 404 Not Found
fn send_404(stream: &mut TcpStream) {
    let _ = stream.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
    let _ = stream.flush();
}

/// 处理单个 HTTP 连接（在调用线程上同步执行）
fn handle_http_connection(
    server_id: i64,
    mut stream: TcpStream,
    session_registry: &Mutex<HashMap<String, bool>>,
) {
    // 读取请求（最多 64 KB）
    let mut buf = vec![0u8; 65536];
    let mut total = 0usize;

    // 尝试读足够数据
    loop {
        match stream.read(&mut buf[total..]) {
            Ok(0) => break,
            Ok(n) => {
                total += n;
                // 若已收到头+体分隔符并且 Content-Length 满足则停止
                let hdr_text = extract_headers_text(&buf[..total]);
                let content_length: usize = extract_header(&hdr_text, "content-length")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0);
                let header_end = buf[..total]
                    .windows(4)
                    .position(|w| w == b"\r\n\r\n")
                    .map(|p| p + 4)
                    .or_else(|| {
                        buf[..total]
                            .windows(2)
                            .position(|w| w == b"\n\n")
                            .map(|p| p + 2)
                    });
                if let Some(body_start) = header_end {
                    if total >= body_start + content_length {
                        break;
                    }
                }
                if total >= buf.len() {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    if total == 0 {
        return;
    }

    let (method, path, body_bytes) = match parse_http_request(&buf[..total]) {
        Some(r) => r,
        None => return,
    };

    eprintln!("[qi-mcp-http] {} {}", method, path);

    // OPTIONS preflight — CORS
    if method == "OPTIONS" {
        let _ = stream.write_all(
            b"HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: POST, GET, DELETE, OPTIONS\r\nAccess-Control-Allow-Headers: Content-Type, Mcp-Session-Id, Accept\r\nContent-Length: 0\r\n\r\n"
        );
        let _ = stream.flush();
        return;
    }

    // Only handle /mcp path
    if path != "/mcp" {
        send_404(&mut stream);
        return;
    }

    // GET /mcp — P2: 真实 SSE 推送通道 (server→client)
    // 为该连接分配会话 ID，将 mpsc::Sender 注册到全局表，
    // 然后阻塞本线程（循环读 receiver），把每条推送消息写到连接上。
    if method == "GET" {
        // 读取请求头文本中的 Mcp-Session-Id（如有）
        let header_text_get = extract_headers_text(&buf[..total]);
        let session_id_get = extract_header(&header_text_get, "mcp-session-id")
            .map(|s| s.to_string())
            .unwrap_or_else(|| 生成会话id());

        eprintln!(
            "[qi-mcp-http] GET /mcp SSE 推送通道已开启, session={}",
            session_id_get
        );

        // 发送 SSE 头（不含 Content-Length，chunked / keep-alive）
        let sse_header = format!(
            "HTTP/1.1 200 OK\r\n\
             Content-Type: text/event-stream\r\n\
             Cache-Control: no-cache\r\n\
             Mcp-Session-Id: {}\r\n\
             Access-Control-Allow-Origin: *\r\n\
             Transfer-Encoding: chunked\r\n\
             \r\n",
            session_id_get
        );
        if stream.write_all(sse_header.as_bytes()).is_err() {
            return;
        }
        // 发送一个初始 keep-alive 注释
        let ka = ": keep-alive\n\n";
        let chunk_ka = format!("{:x}\r\n{}\r\n", ka.len(), ka);
        let _ = stream.write_all(chunk_ka.as_bytes());
        let _ = stream.flush();

        // 创建推送通道并注册
        let (tx, rx) = mpsc::channel::<String>();
        {
            let mut reg = 获取sse通道注册表().lock().unwrap();
            reg.insert(session_id_get.clone(), tx);
        }

        // 设置读超时（不支持 set_read_timeout 影响 write；只用于 keep-alive）
        let _ = stream.set_write_timeout(Some(std::time::Duration::from_secs(30)));

        // 循环把消息写出去（chunk encoding）
        loop {
            match rx.recv_timeout(std::time::Duration::from_secs(15)) {
                Ok(data) => {
                    let chunk = format!("{:x}\r\n{}\r\n", data.len(), data);
                    if stream.write_all(chunk.as_bytes()).is_err() || stream.flush().is_err() {
                        eprintln!("[qi-mcp-http] SSE 客户端断开 session={}", session_id_get);
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // 发送 keep-alive
                    let ka2 = ": keep-alive\n\n";
                    let chunk2 = format!("{:x}\r\n{}\r\n", ka2.len(), ka2);
                    if stream.write_all(chunk2.as_bytes()).is_err() || stream.flush().is_err() {
                        break;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    eprintln!("[qi-mcp-http] SSE 通道已关闭 session={}", session_id_get);
                    break;
                }
            }
        }

        // 清理注册表
        {
            let mut reg = 获取sse通道注册表().lock().unwrap();
            reg.remove(&session_id_get);
        }
        return;
    }

    if method != "POST" {
        send_405(&mut stream);
        return;
    }

    // Extract or assign session ID
    let header_text = extract_headers_text(&buf[..total]);
    let incoming_session = extract_header(&header_text, "mcp-session-id").map(|s| s.to_string());

    let session_id = incoming_session.unwrap_or_else(|| 生成会话id());

    // Register session (idempotent)
    {
        let mut reg = session_registry.lock().unwrap();
        reg.entry(session_id.clone()).or_insert(true);
    }

    // Parse JSON-RPC body
    let body_str = match std::str::from_utf8(&body_bytes) {
        Ok(s) => s.trim().to_string(),
        Err(_) => {
            eprintln!("[qi-mcp-http] 请求体非 UTF-8");
            send_404(&mut stream);
            return;
        }
    };

    if body_str.is_empty() {
        send_202(&mut stream, &session_id);
        return;
    }

    let req: JsonValue = match serde_json::from_str(&body_str) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[qi-mcp-http] JSON 解析错误: {}", e);
            let err_resp = json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32700, "message": format!("Parse error: {}", e)}
            });
            let sse = format!("event: message\ndata: {}\n\n", err_resp);
            send_sse_response(&mut stream, 200, &session_id, &sse);
            return;
        }
    };

    let id = req.get("id").cloned();
    let method_name = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(json!({}));

    eprintln!("[qi-mcp-http] 收到请求 method={} id={:?}", method_name, id);

    // notifications (no id) — 202 (P2: handle notifications/cancelled silently)
    if id.is_none() {
        if method_name == "notifications/cancelled" {
            eprintln!("[qi-mcp-http] 收到 notifications/cancelled, 忽略");
        }
        send_202(&mut stream, &session_id);
        return;
    }

    let response = handle_request(server_id, method_name, &params, &id);

    if let Some(resp) = response {
        let sse = format!("event: message\ndata: {}\n\n", resp);
        send_sse_response(&mut stream, 200, &session_id, &sse);
        eprintln!("[qi-mcp-http] 已响应 method={}", method_name);
    } else {
        send_202(&mut stream, &session_id);
    }
}

/// Streamable HTTP 传输主循环（阻塞）
///
/// 参数:
/// - server_id: 服务器句柄
/// - host:      绑定主机（如 "127.0.0.1" 或 "0.0.0.0"）
/// - port:      监听端口
///
/// 返回: 0 正常退出, -1 服务器不存在, -2 bind 失败
#[no_mangle]
pub extern "C" fn qi_mcp_serve_http(server_id: i64, host: *const c_char, port: i64) -> i32 {
    // 验证服务器存在
    {
        let 服务器池 = 获取服务器池().lock().unwrap();
        if !服务器池.contains_key(&server_id) {
            eprintln!("[qi-mcp-http] 服务器 {} 不存在", server_id);
            return -1;
        }
    }

    let bind_host = if host.is_null() {
        "127.0.0.1".to_string()
    } else {
        unsafe { CStr::from_ptr(host).to_string_lossy().to_string() }
    };
    let bind_port = port as u16;
    let addr = format!("{}:{}", bind_host, bind_port);

    let listener = match TcpListener::bind(&addr) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[qi-mcp-http] bind {} 失败: {}", addr, e);
            return -2;
        }
    };

    eprintln!(
        "[qi-mcp-http] HTTP MCP 服务器启动 http://{}:{}/mcp (server_id={})",
        bind_host, bind_port, server_id
    );

    // 会话注册表（Arc<Mutex<...>> 以便跨线程共享）
    let session_registry = std::sync::Arc::new(Mutex::new(HashMap::<String, bool>::new()));

    for stream_result in listener.incoming() {
        match stream_result {
            Ok(stream) => {
                let reg = session_registry.clone();
                // 每个连接独立线程：使 GET /mcp SSE 长连接不阻塞后续 POST
                std::thread::spawn(move || {
                    handle_http_connection(server_id, stream, &reg);
                });
            }
            Err(e) => {
                eprintln!("[qi-mcp-http] accept 错误: {}", e);
                break;
            }
        }
    }

    eprintln!("[qi-mcp-http] HTTP 服务器退出");
    0
}

/// 处理单个 JSON-RPC 2.0 请求，返回响应 JSON (None = no response)
fn handle_request(
    server_id: i64,
    method: &str,
    params: &JsonValue,
    id: &Option<JsonValue>,
) -> Option<JsonValue> {
    let id_val = id.as_ref().unwrap(); // safe: caller checked

    match method {
        "initialize" => {
            let 服务器池 = 获取服务器池().lock().unwrap();
            let 服务器 = 服务器池.get(&server_id)?;

            let has_tools = !服务器.获取工具列表().is_empty();
            let has_resources = !服务器.获取资源列表().is_empty();
            let has_prompts = !服务器.获取提示列表().is_empty();

            let mut caps = serde_json::Map::new();
            if has_tools {
                caps.insert("tools".to_string(), json!({"listChanged": true}));
            }
            if has_resources {
                caps.insert(
                    "resources".to_string(),
                    json!({"listChanged": true, "subscribe": false}),
                );
            }
            if has_prompts {
                caps.insert("prompts".to_string(), json!({"listChanged": true}));
            }
            // P2: advertise logging capability
            caps.insert("logging".to_string(), json!({}));

            let info = 服务器.获取服务器信息();
            let name = info["name"].as_str().unwrap_or("Qi MCP Server");
            let version = info["version"].as_str().unwrap_or("0.1.0");

            Some(json!({
                "jsonrpc": "2.0",
                "id": id_val,
                "result": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": caps,
                    "serverInfo": {
                        "name": name,
                        "version": version
                    }
                }
            }))
        }

        "ping" => Some(json!({
            "jsonrpc": "2.0",
            "id": id_val,
            "result": {}
        })),

        "tools/list" => {
            let 服务器池 = 获取服务器池().lock().unwrap();
            let 服务器 = match 服务器池.get(&server_id) {
                Some(s) => s,
                None => return Some(error_response(id_val, -32603, "Server not found")),
            };
            let tools = 服务器.获取工具列表();
            Some(json!({
                "jsonrpc": "2.0",
                "id": id_val,
                "result": {"tools": tools}
            }))
        }

        "tools/call" => {
            let tool_name = match params.get("name").and_then(|n| n.as_str()) {
                Some(n) => n.to_string(),
                None => return Some(error_response(id_val, -32602, "Missing tool name")),
            };
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            // Try Qi fn-ptr callback first, then fall back to Rust 执行函数
            let result_text = invoke_tool(server_id, &tool_name, &arguments);

            match result_text {
                Ok(text) => Some(json!({
                    "jsonrpc": "2.0",
                    "id": id_val,
                    "result": {
                        "content": [{"type": "text", "text": text}],
                        "isError": false
                    }
                })),
                Err(e) => Some(error_response(id_val, -32602, &e)),
            }
        }

        "resources/list" => {
            let 服务器池 = 获取服务器池().lock().unwrap();
            let 服务器 = match 服务器池.get(&server_id) {
                Some(s) => s,
                None => return Some(error_response(id_val, -32603, "Server not found")),
            };
            let resources = 服务器.获取资源列表();
            Some(json!({
                "jsonrpc": "2.0",
                "id": id_val,
                "result": {"resources": resources}
            }))
        }

        "resources/read" => {
            let uri = match params.get("uri").and_then(|u| u.as_str()) {
                Some(u) => u.to_string(),
                None => return Some(error_response(id_val, -32602, "Missing uri")),
            };

            let 服务器池 = 获取服务器池().lock().unwrap();
            let 服务器 = match 服务器池.get(&server_id) {
                Some(s) => s,
                None => return Some(error_response(id_val, -32603, "Server not found")),
            };

            match 服务器.获取资源(&uri) {
                Ok(res) => {
                    let content = match &res.内容 {
                        Some(super::mcp::资源内容::文本(t)) => json!({
                            "uri": res.uri,
                            "text": t,
                            "mimeType": res.mime类型.as_deref().unwrap_or("text/plain")
                        }),
                        Some(super::mcp::资源内容::JSON(j)) => json!({
                            "uri": res.uri,
                            "text": j.to_string(),
                            "mimeType": "application/json"
                        }),
                        Some(super::mcp::资源内容::二进制(b)) => {
                            use base64::Engine;
                            json!({
                                "uri": res.uri,
                                "blob": base64::engine::general_purpose::STANDARD.encode(b),
                                "mimeType": res.mime类型.as_deref().unwrap_or("application/octet-stream")
                            })
                        }
                        None => json!({"uri": res.uri, "text": "", "mimeType": "text/plain"}),
                    };
                    Some(json!({
                        "jsonrpc": "2.0",
                        "id": id_val,
                        "result": {"contents": [content]}
                    }))
                }
                Err(e) => Some(error_response(id_val, -32602, &e.to_string())),
            }
        }

        "prompts/list" => {
            let 服务器池 = 获取服务器池().lock().unwrap();
            let 服务器 = match 服务器池.get(&server_id) {
                Some(s) => s,
                None => return Some(error_response(id_val, -32603, "Server not found")),
            };
            let prompts = 服务器.获取提示列表();
            Some(json!({
                "jsonrpc": "2.0",
                "id": id_val,
                "result": {"prompts": prompts}
            }))
        }

        "prompts/get" => {
            let name = match params.get("name").and_then(|n| n.as_str()) {
                Some(n) => n.to_string(),
                None => return Some(error_response(id_val, -32602, "Missing prompt name")),
            };
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

            // Convert arguments to HashMap<String, String>
            let mut args_map: HashMap<String, String> = HashMap::new();
            if let Some(obj) = arguments.as_object() {
                for (k, v) in obj {
                    args_map.insert(k.clone(), v.as_str().unwrap_or(&v.to_string()).to_string());
                }
            }

            let 服务器池 = 获取服务器池().lock().unwrap();
            let 服务器 = match 服务器池.get(&server_id) {
                Some(s) => s,
                None => return Some(error_response(id_val, -32603, "Server not found")),
            };

            match 服务器.获取提示(&name) {
                Ok(提示) => match 提示.填充(&args_map) {
                    Ok(filled) => Some(json!({
                        "jsonrpc": "2.0",
                        "id": id_val,
                        "result": {
                            "messages": [{
                                "role": "user",
                                "content": {"type": "text", "text": filled}
                            }]
                        }
                    })),
                    Err(e) => Some(error_response(id_val, -32602, &e.to_string())),
                },
                Err(e) => Some(error_response(id_val, -32602, &e.to_string())),
            }
        }

        // P2: logging/setLevel — store level on server, respond {}
        "logging/setLevel" => {
            let level = params
                .get("level")
                .and_then(|l| l.as_str())
                .unwrap_or("info")
                .to_string();
            {
                let mut pool = 获取服务器池().lock().unwrap();
                if let Some(srv) = pool.get_mut(&server_id) {
                    srv.日志级别 = level.clone();
                }
            }
            eprintln!("[qi-mcp] logging/setLevel → {}", level);
            Some(json!({
                "jsonrpc": "2.0",
                "id": id_val,
                "result": {}
            }))
        }

        _ => {
            eprintln!("[qi-mcp] 未知方法: {}", method);
            Some(error_response(
                id_val,
                -32601,
                &format!("Method not found: {}", method),
            ))
        }
    }
}

/// 构造 JSON-RPC 2.0 错误响应
fn error_response(id: &JsonValue, code: i64, message: &str) -> JsonValue {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message}
    })
}

/// 调用工具: 优先使用 Qi 闭包对象回调，其次使用 Rust 执行函数
///
/// Qi 闭包调用约定:
///   1. closure_obj 是 Qi 闭包对象 (布局: [fn_ptr_at_offset_0, env_slots...])
///   2. offset 0 存的是 trampoline 函数指针
///   3. trampoline ABI: extern "C" fn(env: *const c_void, args_json: *const c_char) -> *mut c_char
///   4. 调用时传 env=closure_obj, args_json=JSON字符串
fn invoke_tool(server_id: i64, tool_name: &str, arguments: &JsonValue) -> Result<String, String> {
    // 获取工具的回调指针 (Qi 闭包对象地址)
    let closure_ptr = {
        let 服务器池 = 获取服务器池().lock().unwrap();
        if let Some(服务器) = 服务器池.get(&server_id) {
            match 服务器.获取工具(tool_name) {
                Ok(工具) => 工具.回调指针,
                Err(_) => return Err(format!("Tool not found: {}", tool_name)),
            }
        } else {
            return Err(format!("Server not found: {}", server_id));
        }
    };

    if let Some(obj_addr) = closure_ptr {
        // Qi 闭包调用约定:
        //   obj_addr 是闭包对象地址
        //   对象第一个 word (offset 0) 是 trampoline 函数指针
        //   trampoline: extern "C" fn(env: *const c_void, args_json: *const c_char) -> *mut c_char
        let args_str = arguments.to_string();
        let c_args = match CString::new(args_str) {
            Ok(s) => s,
            Err(e) => return Err(format!("Arguments encoding error: {}", e)),
        };

        let result_ptr = unsafe {
            // 从闭包对象的 offset 0 读出 trampoline 函数指针
            let obj_ptr = obj_addr as *const *const std::ffi::c_void;
            let trampoline_raw = *obj_ptr;
            eprintln!(
                "[qi-mcp] 调用 Qi 工具回调: obj=0x{:x} trampoline=0x{:x}",
                obj_addr, trampoline_raw as usize
            );

            // trampoline ABI: fn(env: *const c_void, args: *const c_char) -> *mut c_char
            let trampoline = std::mem::transmute::<
                *const std::ffi::c_void,
                extern "C" fn(*const std::ffi::c_void, *const c_char) -> *mut c_char,
            >(trampoline_raw);

            let env_ptr = obj_addr as *const std::ffi::c_void;
            trampoline(env_ptr, c_args.as_ptr())
        };

        if result_ptr.is_null() {
            return Ok("null".to_string());
        }

        let result_str = unsafe { CStr::from_ptr(result_ptr).to_string_lossy().to_string() };

        // NOTE: Do NOT free result_ptr here.
        // Qi runtime strings are managed by the Qi GC/allocator;
        // calling CString::from_raw would cause double-free/SIGBUS.

        eprintln!("[qi-mcp] 工具回调结果: {}", result_str);
        return Ok(result_str);
    }

    // Fall back to Rust 执行函数
    let params_map: HashMap<String, JsonValue> = if let Some(obj) = arguments.as_object() {
        obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
    } else {
        HashMap::new()
    };

    let 服务器池 = 获取服务器池().lock().unwrap();
    if let Some(服务器) = 服务器池.get(&server_id) {
        match 服务器.执行工具(tool_name, &params_map) {
            Ok(result) => Ok(result.to_string()),
            Err(e) => Err(e.to_string()),
        }
    } else {
        Err(format!("Server not found: {}", server_id))
    }
}

/// 释放字符串
///
/// 参数:
/// - s: 由 MCP FFI 函数返回的字符串指针
#[no_mangle]
pub extern "C" fn qi_mcp_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// P2: 服务器→客户端推送通知 FFI
// ─────────────────────────────────────────────────────────────────────────────

/// 通知客户端工具列表已变更 (notifications/tools/list_changed)
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: 0 成功（服务器存在）, -1 服务器不存在
#[no_mangle]
pub extern "C" fn qi_mcp_notify_tools_changed(server_id: i64) -> i32 {
    {
        let pool = 获取服务器池().lock().unwrap();
        if !pool.contains_key(&server_id) {
            return -1;
        }
    }
    let notif = json!({
        "jsonrpc": "2.0",
        "method": "notifications/tools/list_changed"
    });
    广播通知(&notif);
    0
}

/// 通知客户端资源列表已变更 (notifications/resources/list_changed)
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: 0 成功, -1 服务器不存在
#[no_mangle]
pub extern "C" fn qi_mcp_notify_resources_changed(server_id: i64) -> i32 {
    {
        let pool = 获取服务器池().lock().unwrap();
        if !pool.contains_key(&server_id) {
            return -1;
        }
    }
    let notif = json!({
        "jsonrpc": "2.0",
        "method": "notifications/resources/list_changed"
    });
    广播通知(&notif);
    0
}

/// 通知客户端提示列表已变更 (notifications/prompts/list_changed)
///
/// 参数:
/// - server_id: 服务器句柄
///
/// 返回: 0 成功, -1 服务器不存在
#[no_mangle]
pub extern "C" fn qi_mcp_notify_prompts_changed(server_id: i64) -> i32 {
    {
        let pool = 获取服务器池().lock().unwrap();
        if !pool.contains_key(&server_id) {
            return -1;
        }
    }
    let notif = json!({
        "jsonrpc": "2.0",
        "method": "notifications/prompts/list_changed"
    });
    广播通知(&notif);
    0
}

/// 发送日志消息通知 (notifications/message)
///
/// 参数:
/// - server_id: 服务器句柄
/// - level:     日志级别字符串 ("debug"/"info"/"notice"/"warning"/"error"/"critical"/"alert"/"emergency")
/// - message:   日志内容
///
/// 返回: 0 成功, -1 服务器不存在, -2 参数错误
#[no_mangle]
pub extern "C" fn qi_mcp_log_message(
    server_id: i64,
    level: *const c_char,
    message: *const c_char,
) -> i32 {
    if level.is_null() || message.is_null() {
        return -2;
    }
    {
        let pool = 获取服务器池().lock().unwrap();
        if !pool.contains_key(&server_id) {
            return -1;
        }
    }
    let level_str = unsafe { CStr::from_ptr(level).to_string_lossy().to_string() };
    let msg_str = unsafe { CStr::from_ptr(message).to_string_lossy().to_string() };

    let notif = json!({
        "jsonrpc": "2.0",
        "method": "notifications/message",
        "params": {
            "level": level_str,
            "data": msg_str
        }
    });
    广播通知(&notif);
    0
}

/// 发送进度通知 (notifications/progress)
///
/// 参数:
/// - server_id: 服务器句柄
/// - token:     进度令牌 (字符串)
/// - progress:  当前进度值 (整数)
/// - total:     总进度值 (0 表示未知)
///
/// 返回: 0 成功, -1 服务器不存在, -2 参数错误
#[no_mangle]
pub extern "C" fn qi_mcp_notify_progress(
    server_id: i64,
    token: *const c_char,
    progress: i64,
    total: i64,
) -> i32 {
    if token.is_null() {
        return -2;
    }
    {
        let pool = 获取服务器池().lock().unwrap();
        if !pool.contains_key(&server_id) {
            return -1;
        }
    }
    let token_str = unsafe { CStr::from_ptr(token).to_string_lossy().to_string() };

    let mut params = serde_json::Map::new();
    params.insert("progressToken".to_string(), json!(token_str));
    params.insert("progress".to_string(), json!(progress));
    if total > 0 {
        params.insert("total".to_string(), json!(total));
    }

    let notif = json!({
        "jsonrpc": "2.0",
        "method": "notifications/progress",
        "params": params
    });
    广播通知(&notif);
    0
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn 测试创建服务器() {
        let 名称 = CString::new("测试服务器").unwrap();
        let 版本 = CString::new("1.0.0").unwrap();
        let 描述 = CString::new("测试描述").unwrap();

        let 服务器ID = qi_mcp_create_server(名称.as_ptr(), 版本.as_ptr(), 描述.as_ptr());

        assert!(服务器ID > 0);

        // 清理
        qi_mcp_destroy_server(服务器ID);
    }

    #[test]
    fn 测试注册工具() {
        let 名称 = CString::new("测试服务器").unwrap();
        let 版本 = CString::new("1.0.0").unwrap();
        let 描述 = CString::new("").unwrap();

        let 服务器ID = qi_mcp_create_server(名称.as_ptr(), 版本.as_ptr(), 描述.as_ptr());

        let 工具名 = CString::new("echo").unwrap();
        let 工具描述 = CString::new("回显工具").unwrap();

        let 结果 = qi_mcp_register_tool(服务器ID, 工具名.as_ptr(), 工具描述.as_ptr());

        assert_eq!(结果, 0);

        // 清理
        qi_mcp_destroy_server(服务器ID);
    }

    #[test]
    fn 测试启动停止服务器() {
        let 名称 = CString::new("测试服务器").unwrap();
        let 版本 = CString::new("1.0.0").unwrap();
        let 描述 = CString::new("").unwrap();

        let 服务器ID = qi_mcp_create_server(名称.as_ptr(), 版本.as_ptr(), 描述.as_ptr());

        // 启动服务器
        let 启动结果 = qi_mcp_start_server(服务器ID);
        assert_eq!(启动结果, 0);
        assert_eq!(qi_mcp_is_running(服务器ID), 1);

        // 停止服务器
        let 停止结果 = qi_mcp_stop_server(服务器ID);
        assert_eq!(停止结果, 0);
        assert_eq!(qi_mcp_is_running(服务器ID), 0);

        // 清理
        qi_mcp_destroy_server(服务器ID);
    }
}
