//! protobuf 动态编解码 FFI
//!
//! ── 为什么是「动态描述符」而不是代码生成 ────────────────────────
//!
//! 别的语言接 protobuf 的常规路子是编译期代码生成：`.proto` → 一堆结构体。
//! qi 走不了这条路，也不该走：
//!   1. qi 没有 build.rs 那一环，要么给编译器加一整套 `.proto → .qi` 的
//!      代码生成子系统，要么让用户手动跑命令再把生成物入库；
//!   2. 生成出来的 qi 结构体还得有反射才能填字段 —— 绕回原点。
//!
//! 所以走运行时描述符：`protox` 在**进程里**编译 `.proto`（纯 Rust，不需要
//! 装 protoc 二进制），`prost-reflect` 的 DynamicMessage 负责 JSON ↔ 线格式。
//! qi 那边从头到尾只跟 JSON 打交道 —— 跟数据库层「JSON 行当细腰」是同一个
//! 选择，理由也一样：细腰窄一点，两边都不用认识对方的类型系统。
//!
//! 代价是每次编解码要查一次描述符，比生成代码慢。对 RPC 来说这点开销淹没在
//! 网络往返里，不值得为它引入一整套代码生成。
//!
//! ── JSON 映射按 protobuf 官方规范 ────────────────────────────────
//!
//! 用 prost-reflect 的 serde 集成，遵循 proto3 JSON mapping：字段名
//! lowerCamelCase、64 位整数是字符串、枚举是名字。**不自己发明**一套映射 ——
//! 对面可能是 Go/Java 写的服务，映射对不上就是最难查的那种错。

use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, SerializeOptions};

use crate::stdlib::qi_str::rc_cstr_from_string;

/// 描述符句柄从 4_000_000 起 —— 跟 Redis(3_000_000)、邮箱(900_000)、
/// tokio TCP(1_000_000/2_000_000) 的段位错开。
static NEXT_POOL_ID: AtomicI64 = AtomicI64::new(4_000_000);
static POOLS: OnceLock<Mutex<HashMap<i64, Arc<ProtoPool>>>> = OnceLock::new();

struct ProtoPool {
    pool: DescriptorPool,
    last_error: Mutex<String>,
}

fn pools() -> &'static Mutex<HashMap<i64, Arc<ProtoPool>>> {
    POOLS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lookup(id: i64) -> Option<Arc<ProtoPool>> {
    pools()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
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

impl ProtoPool {
    fn set_error(&self, message: impl Into<String>) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = message.into();
    }

    fn clear_error(&self) {
        self.last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

/// 编译一批 `.proto`，返回描述符句柄；失败 -1。
///
/// `files` 是逗号分隔的路径，`include_dirs` 同理（`import` 按它解析）。
/// 导入目录留空时按每个文件自己的目录算 —— 单文件的小服务不用配。
#[no_mangle]
pub extern "C" fn qi_pb_load(files: *const c_char, include_dirs: *const c_char) -> i64 {
    let files_text = read_cstr(files);
    if files_text.is_empty() {
        return -1;
    }
    let file_list: Vec<String> = files_text
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if file_list.is_empty() {
        return -1;
    }

    let mut dirs: Vec<String> = read_cstr(include_dirs)
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if dirs.is_empty() {
        // 没给导入目录就用文件所在目录
        for one in &file_list {
            if let Some(parent) = std::path::Path::new(one).parent() {
                let parent_str = parent.to_string_lossy().to_string();
                let parent_str = if parent_str.is_empty() {
                    ".".to_string()
                } else {
                    parent_str
                };
                if !dirs.contains(&parent_str) {
                    dirs.push(parent_str);
                }
            }
        }
    }

    let fd_set = match protox::compile(&file_list, &dirs) {
        Ok(set) => set,
        Err(_) => return -1,
    };
    let pool = match DescriptorPool::from_file_descriptor_set(fd_set) {
        Ok(p) => p,
        Err(_) => return -1,
    };

    let id = NEXT_POOL_ID.fetch_add(1, Ordering::SeqCst);
    pools().lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        Arc::new(ProtoPool {
            pool,
            last_error: Mutex::new(String::new()),
        }),
    );
    id
}

/// 释放描述符。
#[no_mangle]
pub extern "C" fn qi_pb_free(id: i64) -> i64 {
    match pools()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id)
    {
        Some(_) => 0,
        None => -1,
    }
}

/// 最近一次失败的说明；没有错误返回空串。
///
/// 编解码失败只能返回空串/-1（返回值没别的位置放错误码），而空串同时是
/// 「空消息」的合法编码结果 —— 分不清的时候查这个。
#[no_mangle]
pub extern "C" fn qi_pb_last_error(id: i64) -> *mut c_char {
    match lookup(id) {
        Some(p) => out_str(
            p.last_error
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        ),
        None => out_str("描述符句柄无效".to_string()),
    }
}

/// 描述符里有没有这个消息类型 —— 注册路由前自查，免得把类型名拼错了
/// 却要等对面来调用才发现。
#[no_mangle]
pub extern "C" fn qi_pb_has_message(id: i64, full_name: *const c_char) -> i64 {
    let name = read_cstr(full_name);
    match lookup(id) {
        Some(p) => {
            if p.pool.get_message_by_name(&name).is_some() {
                1
            } else {
                0
            }
        }
        None => 0,
    }
}

/// 列出所有服务及其方法，JSON：
/// `[{"服务":"greet.Greeter","方法":[{"名":"SayHello","请求":"…","响应":"…","客户端流":0,"服务端流":0}]}]`
///
/// 键名用中文是因为**这份 JSON 是给 qi 侧读的**（qi-grpc 按它建路由表），
/// 不是发到线上的协议 —— 线上那份由 protobuf 决定，跟这里无关。
#[no_mangle]
pub extern "C" fn qi_pb_services(id: i64) -> *mut c_char {
    let Some(p) = lookup(id) else {
        return out_str("[]".to_string());
    };
    let mut out = Vec::new();
    for service in p.pool.services() {
        let mut methods = Vec::new();
        for method in service.methods() {
            methods.push(serde_json::json!({
                "名": method.name(),
                "请求": method.input().full_name(),
                "响应": method.output().full_name(),
                "客户端流": if method.is_client_streaming() { 1 } else { 0 },
                "服务端流": if method.is_server_streaming() { 1 } else { 0 },
            }));
        }
        out.push(serde_json::json!({
            "服务": service.full_name(),
            "方法": methods,
        }));
    }
    out_str(serde_json::Value::Array(out).to_string())
}

/// JSON → protobuf 线格式，返回**字节切片句柄**（标准库.字节切片 那一套），
/// 失败 -1。用完照常 释放切片。
///
/// 不用「指针 + 出参长度」：qi 没有取地址那一套，硬做出参会逼着上层写一堆
/// 样板；字节切片句柄本来就是 qi 里搬二进制的正规形式，gRPC 那层直接把这个
/// 句柄丢给发送函数，中间一次拷贝都不用。
#[no_mangle]
pub extern "C" fn qi_pb_json_to_bytes(
    id: i64,
    message_name: *const c_char,
    json: *const c_char,
) -> i64 {
    let Some(p) = lookup(id) else {
        return -1;
    };
    let name = read_cstr(message_name);
    let text = read_cstr(json);
    let text = if text.trim().is_empty() {
        "{}".to_string()
    } else {
        text
    };

    let Some(desc) = p.pool.get_message_by_name(&name) else {
        p.set_error(format!("描述符里没有消息类型 {}", name));
        return -1;
    };
    let mut de = serde_json::Deserializer::from_str(&text);
    let message = match DynamicMessage::deserialize(desc, &mut de) {
        Ok(m) => m,
        Err(e) => {
            p.set_error(format!("JSON 填进 {} 失败: {}", name, e));
            return -1;
        }
    };
    if let Err(e) = de.end() {
        p.set_error(format!("JSON 尾部有多余内容: {}", e));
        return -1;
    }

    p.clear_error();
    crate::stdlib::bytes_ffi::register_bytes(message.encode_to_vec())
}

/// protobuf 线格式 → JSON。收字节切片句柄，失败返回空串（查 last_error）。
#[no_mangle]
pub extern "C" fn qi_pb_bytes_to_json(
    id: i64,
    message_name: *const c_char,
    bytes_handle: i64,
) -> *mut c_char {
    let Some(p) = lookup(id) else {
        return out_str(String::new());
    };
    let name = read_cstr(message_name);
    let Some(desc) = p.pool.get_message_by_name(&name) else {
        p.set_error(format!("描述符里没有消息类型 {}", name));
        return out_str(String::new());
    };

    // 空句柄当空消息：proto3 里「所有字段都是默认值」的编码就是零字节
    let data = crate::stdlib::bytes_ffi::clone_bytes(bytes_handle).unwrap_or_default();

    let message = match DynamicMessage::decode(desc, data.as_slice()) {
        Ok(m) => m,
        Err(e) => {
            p.set_error(format!("解 {} 失败: {}", name, e));
            return out_str(String::new());
        }
    };

    // 默认值也序列化出来：qi 侧拿到的 JSON 字段齐全，不用为「这个键在不在」
    // 分情况写代码。这与 proto3 JSON 默认「省略默认值」不同，是**故意**的，
    // 只影响我们给 qi 的那一面，不影响线格式。
    let options = SerializeOptions::new().skip_default_fields(false);
    let mut buf = Vec::new();
    let mut ser = serde_json::Serializer::new(&mut buf);
    if let Err(e) = message.serialize_with_options(&mut ser, &options) {
        p.set_error(format!("{} 转 JSON 失败: {}", name, e));
        return out_str(String::new());
    }
    p.clear_error();
    out_str(String::from_utf8_lossy(&buf).to_string())
}
