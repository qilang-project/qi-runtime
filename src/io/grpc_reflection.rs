//! gRPC 服务端反射（grpc.reflection.v1 与 v1alpha）
//!
//! 开了它，`grpcurl` 不用带 `-proto` 就能 list / describe / 直接调；
//! Postman、grpcui 这类工具也是靠它才认得出服务长什么样。
//!
//! ── 为什么整个实现在运行时里，不交给 qi ────────────────────────
//!
//! 反射是**双向流**：客户端在一条流上连着问好几件事（先 list_services，
//! 再按符号要描述符，可能还要跟着依赖一路要下去），服务端逐条回。
//! 而它要的东西全是描述符池里现成的，业务代码一点都插不上手 ——
//! 交给 qi 只会逼着 qi 侧先有一套流式 API 才能开反射。所以这条路由被
//! 运行时**截胡**：见到反射方法就地答复，根本不进 qi 的队列。
//!
//! ── 描述符要连依赖一起给 ────────────────────────────────────────
//!
//! 客户端拿到一个 FileDescriptorProto 之后，还要能解开它 import 的那些文件
//! 才能构造完整类型。规范允许一次返回多个文件，**所以把依赖递归收齐一起发**。
//! 只发一个的话，凡是 .proto 里有 import 的服务，grpcurl 都会报
//! 「could not resolve」，而错误信息完全不指向真正的原因。
//!
//! ── 反射用的那份 .proto 是内嵌的 ────────────────────────────────
//!
//! 用 protox_parse 在进程里从字符串解析出来（不落盘、不依赖 protoc），
//! 再用 DynamicMessage 收发 —— 跟业务消息走的是同一套动态编解码，
//! 没有第二份手写的 protobuf 编解码代码。手写 varint 那条路省不了多少事，
//! 却要在字段号上赌自己记得对。

use bytes::Bytes;
use h2::server::SendResponse;
use http::{HeaderMap, Response};
use prost::Message;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, Value};
use std::sync::OnceLock;

/// 反射服务的两个版本。v1alpha 是老名字，很多工具（含旧版 grpcurl）只认它，
/// 所以两个都挂上；消息结构完全一样，只有包名不同。
pub(crate) const V1_METHOD: &str = "grpc.reflection.v1.ServerReflection/ServerReflectionInfo";
pub(crate) const V1ALPHA_METHOD: &str =
    "grpc.reflection.v1alpha.ServerReflection/ServerReflectionInfo";

pub(crate) fn is_reflection_method(method: &str) -> bool {
    method == V1_METHOD || method == V1ALPHA_METHOD
}

/// 官方 reflection.proto 的最小可用子集：只保留我们答得上来的那几种请求。
/// 字段号与官方一致 —— 线格式认的是号不是名，改号就不通了。
const REFLECTION_PROTO: &str = r#"
syntax = "proto3";
package grpc.reflection.v1;

message ServerReflectionRequest {
  string host = 1;
  oneof message_request {
    string file_by_filename = 3;
    string file_containing_symbol = 4;
    ExtensionRequest file_containing_extension = 5;
    string all_extension_numbers_of_type = 6;
    string list_services = 7;
  }
}

message ExtensionRequest {
  string containing_type = 1;
  int32 extension_number = 2;
}

message ServerReflectionResponse {
  string valid_host = 1;
  ServerReflectionRequest original_request = 2;
  oneof message_response {
    FileDescriptorResponse file_descriptor_response = 4;
    ExtensionNumberResponse all_extension_numbers_response = 5;
    ListServiceResponse list_services_response = 6;
    ErrorResponse error_response = 7;
  }
}

message FileDescriptorResponse {
  repeated bytes file_descriptor_proto = 1;
}

message ExtensionNumberResponse {
  string base_type_name = 1;
  repeated int32 extension_number = 2;
}

message ListServiceResponse {
  repeated ServiceResponse service = 1;
}

message ServiceResponse {
  string name = 1;
}

message ErrorResponse {
  int32 error_code = 1;
  string error_message = 2;
}
"#;

static REFLECTION_POOL: OnceLock<Option<DescriptorPool>> = OnceLock::new();

fn reflection_pool() -> Option<&'static DescriptorPool> {
    REFLECTION_POOL
        .get_or_init(|| {
            let fd = protox_parse::parse("grpc/reflection/v1/reflection.proto", REFLECTION_PROTO)
                .ok()?;
            let mut pool = DescriptorPool::new();
            pool.add_file_descriptor_proto(fd).ok()?;
            Some(pool)
        })
        .as_ref()
}

fn request_descriptor() -> Option<MessageDescriptor> {
    reflection_pool()?.get_message_by_name("grpc.reflection.v1.ServerReflectionRequest")
}

fn response_descriptor() -> Option<MessageDescriptor> {
    reflection_pool()?.get_message_by_name("grpc.reflection.v1.ServerReflectionResponse")
}

/// 一条消息的分帧（跟 grpc_ffi 里那套一样，不压缩）
fn frame(msg: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(5 + msg.len());
    out.push(0u8);
    out.extend_from_slice(&(msg.len() as u32).to_be_bytes());
    out.extend_from_slice(msg);
    out
}

fn take_one(buf: &[u8]) -> Option<(usize, Vec<u8>)> {
    if buf.len() < 5 {
        return None;
    }
    let len = u32::from_be_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
    if buf.len() < 5 + len {
        return None;
    }
    Some((5 + len, buf[5..5 + len].to_vec()))
}

/// 把一个文件及其 import 的依赖递归收齐，编码成 FileDescriptorProto 字节。
/// 见文件顶部「描述符要连依赖一起给」。
fn collect_with_deps(
    pool: &DescriptorPool,
    file_name: &str,
    seen: &mut Vec<String>,
    out: &mut Vec<Vec<u8>>,
) {
    if seen.iter().any(|s| s == file_name) {
        return;
    }
    let Some(file) = pool.get_file_by_name(file_name) else {
        return;
    };
    seen.push(file_name.to_string());
    out.push(file.file_descriptor_proto().encode_to_vec());
    for dep in file.dependencies() {
        collect_with_deps(pool, dep.name(), seen, out);
    }
}

/// 处理一条反射流：客户端在同一条流上连着问，我们逐条答，直到它半关。
pub(crate) async fn serve(
    mut body: h2::RecvStream,
    mut respond: SendResponse<Bytes>,
    pool: DescriptorPool,
) {
    let (Some(req_desc), Some(resp_desc)) = (request_descriptor(), response_descriptor()) else {
        finish_with_error(&mut respond, "反射服务自身的描述符建不起来");
        return;
    };

    let resp = match Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .body(())
    {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut stream = match respond.send_response(resp, false) {
        Ok(s) => s,
        Err(_) => return,
    };

    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = body.data().await {
        let Ok(data) = chunk else { break };
        let _ = body.flow_control().release_capacity(data.len());
        buf.extend_from_slice(&data);

        // 一次可能收到多条请求，逐条答
        while let Some((used, msg)) = take_one(&buf) {
            buf.drain(..used);
            let reply = answer_one(&pool, &req_desc, &resp_desc, &msg);
            if let Some(bytes) = reply {
                if stream.send_data(Bytes::from(frame(&bytes)), false).is_err() {
                    return;
                }
            }
        }
    }

    let mut trailers = HeaderMap::new();
    trailers.insert("grpc-status", "0".parse().unwrap());
    let _ = stream.send_trailers(trailers);
}

fn finish_with_error(respond: &mut SendResponse<Bytes>, message: &str) {
    let Ok(resp) = Response::builder()
        .status(200)
        .header("content-type", "application/grpc")
        .body(())
    else {
        return;
    };
    if let Ok(mut stream) = respond.send_response(resp, false) {
        let mut trailers = HeaderMap::new();
        trailers.insert("grpc-status", "13".parse().unwrap());
        if let Ok(v) = message.parse() {
            trailers.insert("grpc-message", v);
        }
        let _ = stream.send_trailers(trailers);
    }
}

/// 答一条反射请求。返回编码好的 ServerReflectionResponse 字节。
fn answer_one(
    pool: &DescriptorPool,
    req_desc: &MessageDescriptor,
    resp_desc: &MessageDescriptor,
    raw: &[u8],
) -> Option<Vec<u8>> {
    let request = DynamicMessage::decode(req_desc.clone(), raw).ok()?;
    let mut response = DynamicMessage::new(resp_desc.clone());

    // 规范要求把原请求带回去，客户端靠它把响应对上号
    if let Some(field) = resp_desc.get_field_by_name("original_request") {
        response.set_field(&field, Value::Message(request.clone()));
    }

    let get_str = |name: &str| -> Option<String> {
        let field = req_desc.get_field_by_name(name)?;
        if !request.has_field(&field) {
            return None;
        }
        Some(request.get_field(&field).as_str()?.to_string())
    };

    // list_services：列出所有服务
    if get_str("list_services").is_some() {
        let list_desc =
            reflection_pool()?.get_message_by_name("grpc.reflection.v1.ListServiceResponse")?;
        let service_desc =
            reflection_pool()?.get_message_by_name("grpc.reflection.v1.ServiceResponse")?;
        let mut list = DynamicMessage::new(list_desc.clone());
        let mut services = Vec::new();
        for service in pool.services() {
            let mut one = DynamicMessage::new(service_desc.clone());
            if let Some(f) = service_desc.get_field_by_name("name") {
                one.set_field(&f, Value::String(service.full_name().to_string()));
            }
            services.push(Value::Message(one));
        }
        if let Some(f) = list_desc.get_field_by_name("service") {
            list.set_field(&f, Value::List(services));
        }
        let f = resp_desc.get_field_by_name("list_services_response")?;
        response.set_field(&f, Value::Message(list));
        return Some(response.encode_to_vec());
    }

    // file_containing_symbol：给个符号（服务名/方法全名/消息名），要它所在的文件
    let mut wanted_file: Option<String> = None;
    if let Some(symbol) = get_str("file_containing_symbol") {
        // 方法全名（包.服务.方法 或 包.服务/方法）先退回到服务名
        let symbol = symbol.replace('/', ".");
        wanted_file = pool
            .get_service_by_name(&symbol)
            .map(|s| s.parent_file().name().to_string())
            .or_else(|| {
                pool.get_message_by_name(&symbol)
                    .map(|m| m.parent_file().name().to_string())
            })
            .or_else(|| {
                // 是方法全名的话，砍掉最后一段再试服务
                symbol.rsplit_once('.').and_then(|(head, _)| {
                    pool.get_service_by_name(head)
                        .map(|s| s.parent_file().name().to_string())
                })
            });
        if wanted_file.is_none() {
            return Some(error_response(
                &mut response,
                resp_desc,
                5,
                &format!("找不到符号 {}", symbol),
            ));
        }
    } else if let Some(name) = get_str("file_by_filename") {
        if pool.get_file_by_name(&name).is_none() {
            return Some(error_response(
                &mut response,
                resp_desc,
                5,
                &format!("找不到文件 {}", name),
            ));
        }
        wanted_file = Some(name);
    }

    if let Some(file_name) = wanted_file {
        let fd_desc =
            reflection_pool()?.get_message_by_name("grpc.reflection.v1.FileDescriptorResponse")?;
        let mut fd_resp = DynamicMessage::new(fd_desc.clone());
        let mut seen = Vec::new();
        let mut files = Vec::new();
        collect_with_deps(pool, &file_name, &mut seen, &mut files);
        if let Some(f) = fd_desc.get_field_by_name("file_descriptor_proto") {
            fd_resp.set_field(
                &f,
                Value::List(files.into_iter().map(|b| Value::Bytes(b.into())).collect()),
            );
        }
        let f = resp_desc.get_field_by_name("file_descriptor_response")?;
        response.set_field(&f, Value::Message(fd_resp));
        return Some(response.encode_to_vec());
    }

    // 扩展相关的两种请求我们不支持（proto3 里基本用不上）
    Some(error_response(
        &mut response,
        resp_desc,
        12,
        "本服务只支持 list_services / file_by_filename / file_containing_symbol",
    ))
}

fn error_response(
    response: &mut DynamicMessage,
    resp_desc: &MessageDescriptor,
    code: i32,
    message: &str,
) -> Vec<u8> {
    if let (Some(pool), Some(field)) = (
        reflection_pool(),
        resp_desc.get_field_by_name("error_response"),
    ) {
        if let Some(err_desc) = pool.get_message_by_name("grpc.reflection.v1.ErrorResponse") {
            let mut err = DynamicMessage::new(err_desc.clone());
            if let Some(f) = err_desc.get_field_by_name("error_code") {
                err.set_field(&f, Value::I32(code));
            }
            if let Some(f) = err_desc.get_field_by_name("error_message") {
                err.set_field(&f, Value::String(message.to_string()));
            }
            response.set_field(&field, Value::Message(err));
        }
    }
    response.encode_to_vec()
}
