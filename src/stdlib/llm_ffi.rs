//! LLM 模块 FFI 接口
//!
//! 为 Qi 语言提供 C 接口的 LLM 调用函数

#![allow(non_snake_case)]

use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::io::Read;
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};

// LLM 会话池
static LLM会话池: OnceLock<Mutex<HashMap<i64, LLM会话>>> = OnceLock::new();
static 会话计数器: OnceLock<Mutex<i64>> = OnceLock::new();
static LLM流池: OnceLock<Mutex<HashMap<i64, LLM流>>> = OnceLock::new();
static 流计数器: OnceLock<Mutex<i64>> = OnceLock::new();

fn 获取会话池() -> &'static Mutex<HashMap<i64, LLM会话>> {
    LLM会话池.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 获取会话计数器() -> &'static Mutex<i64> {
    会话计数器.get_or_init(|| Mutex::new(0))
}

fn 获取流池() -> &'static Mutex<HashMap<i64, LLM流>> {
    LLM流池.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 获取流计数器() -> &'static Mutex<i64> {
    流计数器.get_or_init(|| Mutex::new(0))
}

/// LLM 会话结构
#[derive(Debug, Clone)]
struct LLM会话 {
    /// API 端点
    端点: String,
    /// API 密钥
    密钥: Option<String>,
    /// 模型名称
    模型: String,
    /// 对话历史，使用 OpenAI-compatible message JSON 表示，便于保存 tool_calls/tool 结果
    历史: Vec<Value>,
    /// 可用工具定义
    工具列表: Vec<Value>,
    /// provider-safe 工具名 -> Qi 原始工具名
    工具名称映射: HashMap<String, String>,
    /// 配置参数
    配置: HashMap<String, String>,
    /// 最近一次**非流式**请求的 token 用量 (prompt, completion, total)；未知/未请求为 0。
    最近用量: (i64, i64, i64),
    /// 会话预算：token 上限（0 = 不限）与累计已用 total。超限后再调用直接拒绝（不打 API）。
    预算上限: i64,
    累计用量: i64,
}

impl LLM会话 {
    fn 标准化端点(端点: String) -> String {
        let 去尾端点 = 端点.trim_end_matches('/').to_string();
        if 去尾端点.ends_with("/chat/completions") {
            去尾端点
        } else {
            format!("{}/chat/completions", 去尾端点)
        }
    }

    fn 创建(端点: String, 模型: String, 密钥: Option<String>) -> Self {
        Self {
            端点: Self::标准化端点(端点),
            密钥,
            模型,
            历史: Vec::new(),
            工具列表: Vec::new(),
            工具名称映射: HashMap::new(),
            配置: HashMap::new(),
            最近用量: (0, 0, 0),
            预算上限: 0,
            累计用量: 0,
        }
    }

    /// 预算闸：超限返回 Err（不打 API）。每次非流式调用前查。
    fn 预算检查(&self) -> Result<(), String> {
        if self.预算上限 > 0 && self.累计用量 >= self.预算上限 {
            return Err(format!(
                "预算超限: 已用 {} / 上限 {} tokens",
                self.累计用量, self.预算上限
            ));
        }
        Ok(())
    }

    /// 调用后记账：最近用量.total 累进 累计用量。
    fn 预算记账(&mut self) {
        self.累计用量 += self.最近用量.2;
    }

    /// 从响应体的 usage 字段提取 (prompt, completion, total) token 数；缺失为 0。
    fn 提取用量(响应体: &Value) -> (i64, i64, i64) {
        let u = 响应体.get("usage");
        let 取 = |k: &str| {
            u.and_then(|u| u.get(k))
                .and_then(|v| v.as_i64())
                .unwrap_or(0)
        };
        (取("prompt_tokens"), 取("completion_tokens"), 取("total_tokens"))
    }

    /// 结构化输出：按配置注入 response_format。
    /// 配置值 "json"/"json_object" → JSON 模式；以 `{` 开头 → 当作完整 response_format 对象
    /// （如 {"type":"json_schema","json_schema":{...}}，供支持 strict schema 的 provider）。
    fn 注入响应格式(&self, 请求体: &mut Value) {
        if let Some(rf) = self.配置.get("response_format") {
            let rf = rf.trim();
            if rf.is_empty() {
                return;
            }
            if rf == "json" || rf == "json_object" {
                请求体["response_format"] = json!({"type": "json_object"});
            } else if rf.starts_with('{') {
                if let Ok(v) = serde_json::from_str::<Value>(rf) {
                    请求体["response_format"] = v;
                }
            }
        }
    }

    /// 把配置里的 system 提示作为首条 system 消息插到 messages 最前面。
    /// 之前 set_config("system", ...) 只存进配置 map 却从不发出去 —— 系统提示
    /// 形同虚设。这里在构建请求体时统一注入（历史已含 system 时不重复）。
    fn 注入系统消息(&self, 消息列表: &mut Vec<Value>) {
        if let Some(系统) = self.配置.get("system") {
            if !系统.is_empty() {
                let 已有系统 = 消息列表
                    .first()
                    .and_then(|m| m.get("role"))
                    .and_then(|r| r.as_str())
                    == Some("system");
                if !已有系统 {
                    消息列表.insert(0, json!({ "role": "system", "content": 系统 }));
                }
            }
        }
    }

    fn 构建请求体(&self, 提示: &str, 流式: bool, 使用工具: bool) -> Value {
        let mut 消息列表 = self.历史.clone();
        消息列表.push(json!({
            "role": "user",
            "content": 提示
        }));
        self.注入系统消息(&mut 消息列表);

        let mut 请求体 = json!({
            "model": self.模型,
            "messages": 消息列表,
            "temperature": self.配置.get("temperature")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.7),
            "max_tokens": self.配置.get("max_tokens")
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(2000),
        });

        if 流式 {
            请求体["stream"] = json!(true);
        }

        if 使用工具 && !self.工具列表.is_empty() {
            请求体["tools"] = Value::Array(self.工具列表.clone());
            请求体["tool_choice"] = json!("auto");
        }
        self.注入响应格式(&mut 请求体);

        请求体
    }

    fn 构建继续请求体(&self, 使用工具: bool) -> Value {
        let mut 消息列表 = self.历史.clone();
        self.注入系统消息(&mut 消息列表);
        let mut 请求体 = json!({
            "model": self.模型,
            "messages": 消息列表,
            "temperature": self.配置.get("temperature")
                .and_then(|s| s.parse::<f64>().ok())
                .unwrap_or(0.7),
            "max_tokens": self.配置.get("max_tokens")
                .and_then(|s| s.parse::<i32>().ok())
                .unwrap_or(2000),
        });

        if 使用工具 && !self.工具列表.is_empty() {
            请求体["tools"] = Value::Array(self.工具列表.clone());
            请求体["tool_choice"] = json!("auto");
        }
        self.注入响应格式(&mut 请求体);

        请求体
    }

    fn 发送请求体(&self, 请求体: Value) -> Result<reqwest::blocking::Response, String> {
        use reqwest::blocking::Client;

        let 客户端 = Client::new();
        let mut 请求构建器 = 客户端
            .post(&self.端点)
            .header("Content-Type", "application/json");

        if let Some(ref 密钥) = self.密钥 {
            请求构建器 = 请求构建器.header("Authorization", format!("Bearer {}", 密钥));
        }

        let 响应 = 请求构建器
            .json(&请求体)
            .send()
            .map_err(|e| format!("HTTP请求失败: {}", e))?;

        if !响应.status().is_success() {
            let 状态码 = 响应.status();
            let 错误文本 = 响应
                .text()
                .unwrap_or_else(|_| "无法读取错误响应".to_string());
            return Err(format!("API返回错误 {}: {}", 状态码, 错误文本));
        }

        Ok(响应)
    }

    fn 提取消息(响应体: &Value) -> Result<Value, String> {
        响应体
            .get("choices")
            .and_then(|choices| choices.get(0))
            .and_then(|choice| choice.get("message"))
            .cloned()
            .ok_or_else(|| "响应格式错误：无法提取 message".to_string())
    }

    fn 提取文本(消息: &Value) -> Result<String, String> {
        Ok(消息
            .get("content")
            .and_then(|content| content.as_str())
            .unwrap_or("")
            .to_string())
    }

    /// 发送HTTP请求到LLM API（&mut：顺带记录本次 token 用量）
    fn 调用API(&mut self, 提示: &str) -> Result<String, String> {
        self.预算检查()?;
        let 请求体 = self.构建请求体(提示, false, false);
        let 响应体: Value = self
            .发送请求体(请求体)?
            .json()
            .map_err(|e| format!("解析响应失败: {}", e))?;

        self.最近用量 = Self::提取用量(&响应体);
        self.预算记账();
        let 消息 = Self::提取消息(&响应体)?;
        Self::提取文本(&消息)
    }

    /// 带工具定义发送请求，返回完整 assistant message JSON（&mut：记录用量）
    fn 调用工具API(&mut self, 提示: &str) -> Result<Value, String> {
        self.预算检查()?;
        let 请求体 = self.构建请求体(提示, false, true);
        let 响应体: Value = self
            .发送请求体(请求体)?
            .json()
            .map_err(|e| format!("解析响应失败: {}", e))?;

        self.最近用量 = Self::提取用量(&响应体);
        self.预算记账();
        Self::提取消息(&响应体)
    }

    /// 继续工具对话，通常在添加 tool 结果后调用（&mut：记录用量）
    fn 继续工具API(&mut self) -> Result<Value, String> {
        self.预算检查()?;
        let 请求体 = self.构建继续请求体(true);
        let 响应体: Value = self
            .发送请求体(请求体)?
            .json()
            .map_err(|e| format!("解析响应失败: {}", e))?;

        self.最近用量 = Self::提取用量(&响应体);
        self.预算记账();
        Self::提取消息(&响应体)
    }

    /// 打开流式响应
    fn 打开流(&self, 提示: &str) -> Result<reqwest::blocking::Response, String> {
        let 请求体 = self.构建请求体(提示, true, false);
        self.发送请求体(请求体)
    }

    /// 流式 + 工具：流式请求里带上 tools，模型可在流中吐 tool_calls。
    fn 打开流带工具(&self, 提示: &str) -> Result<reqwest::blocking::Response, String> {
        let 请求体 = self.构建请求体(提示, true, true);
        self.发送请求体(请求体)
    }

    /// 工具结果回写后，流式继续推理（仍带 tools，可继续调工具或给最终答复）。
    fn 打开续传流(&self) -> Result<reqwest::blocking::Response, String> {
        let mut 请求体 = self.构建继续请求体(true);
        请求体["stream"] = json!(true);
        self.发送请求体(请求体)
    }
}

struct LLM流 {
    会话句柄: i64,
    提示: String,
    响应: reqwest::blocking::Response,
    缓冲: String,
    待返回: VecDeque<String>,
    完成: bool,
    累计: String,
    // 流式 + 工具调用：带工具时把分块到达的 tool_calls 按 index 累积成 (id, name, arguments)；
    // 是续传 = 这条流是 tool 结果后的 continue（历史里已有 user 消息，不再重复 push）。
    带工具: bool,
    是续传: bool,
    工具分块: Vec<(String, String, String)>,
}

impl LLM流 {
    fn 创建(
        会话句柄: i64,
        提示: String,
        响应: reqwest::blocking::Response,
        带工具: bool,
        是续传: bool,
    ) -> Self {
        Self {
            会话句柄,
            提示,
            响应,
            缓冲: String::new(),
            待返回: VecDeque::new(),
            完成: false,
            累计: String::new(),
            带工具,
            是续传,
            工具分块: Vec::new(),
        }
    }

    fn 读取下个片段(&mut self) -> Result<Option<String>, String> {
        loop {
            if let Some(片段) = self.待返回.pop_front() {
                self.累计.push_str(&片段);
                return Ok(Some(片段));
            }

            if self.完成 {
                return Ok(None);
            }

            let mut 字节 = [0u8; 4096];
            let 数量 = self
                .响应
                .read(&mut 字节)
                .map_err(|e| format!("读取流失败: {}", e))?;

            if 数量 == 0 {
                self.完成 = true;
                return Ok(None);
            }

            self.缓冲.push_str(&String::from_utf8_lossy(&字节[..数量]));
            self.解析缓冲();
        }
    }

    fn 查找事件分隔符(文本: &str) -> Option<(usize, usize)> {
        let 双换行 = 文本.find("\n\n").map(|pos| (pos, 2));
        let 双回车换行 = 文本.find("\r\n\r\n").map(|pos| (pos, 4));
        match (双换行, 双回车换行) {
            (Some(a), Some(b)) => Some(if a.0 < b.0 { a } else { b }),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    fn 解析缓冲(&mut self) {
        while let Some((位置, 分隔长度)) = Self::查找事件分隔符(&self.缓冲) {
            let 事件 = self.缓冲[..位置].to_string();
            self.缓冲.drain(..位置 + 分隔长度);

            let mut 数据 = String::new();
            for 行 in 事件.lines() {
                let 去空白 = 行.trim_start();
                if let Some(内容) = 去空白.strip_prefix("data:") {
                    数据.push_str(内容.trim());
                }
            }

            if 数据.is_empty() {
                continue;
            }

            if 数据 == "[DONE]" {
                self.完成 = true;
                continue;
            }

            if let Ok(JSON) = serde_json::from_str::<Value>(&数据) {
                let delta = JSON
                    .get("choices")
                    .and_then(|choices| choices.get(0))
                    .and_then(|choice| choice.get("delta"));

                if let Some(内容) = delta
                    .and_then(|d| d.get("content"))
                    .and_then(|content| content.as_str())
                {
                    if !内容.is_empty() {
                        self.待返回.push_back(内容.to_string());
                    }
                }

                // 流式 tool_calls 是增量的：第一块给 index/id/name，后续块拼 arguments。
                // 按 index 累积进 工具分块，结束后由 组装助手消息 拼成完整 assistant 消息。
                if let Some(数组) = delta
                    .and_then(|d| d.get("tool_calls"))
                    .and_then(|v| v.as_array())
                {
                    for tc in 数组 {
                        let idx = tc.get("index").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
                        while self.工具分块.len() <= idx {
                            self.工具分块
                                .push((String::new(), String::new(), String::new()));
                        }
                        if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                            if !id.is_empty() {
                                self.工具分块[idx].0 = id.to_string();
                            }
                        }
                        if let Some(函数) = tc.get("function") {
                            if let Some(名) = 函数.get("name").and_then(|v| v.as_str()) {
                                if !名.is_empty() {
                                    self.工具分块[idx].1 = 名.to_string();
                                }
                            }
                            if let Some(参) = 函数.get("arguments").and_then(|v| v.as_str()) {
                                self.工具分块[idx].2.push_str(参);
                            }
                        }
                    }
                }
            }
        }
    }

    /// 把累积的内容 + 工具分块拼成完整 assistant 消息（OpenAI 格式），供上层派发工具/入历史。
    fn 组装助手消息(&self) -> Value {
        let mut 消息 = json!({ "role": "assistant" });
        if self.累计.is_empty() {
            消息["content"] = Value::Null;
        } else {
            消息["content"] = json!(self.累计);
        }
        if !self.工具分块.is_empty() {
            let 调用: Vec<Value> = self
                .工具分块
                .iter()
                .map(|(id, 名, 参)| {
                    json!({
                        "id": id,
                        "type": "function",
                        "function": { "name": 名, "arguments": 参 }
                    })
                })
                .collect();
            消息["tool_calls"] = Value::Array(调用);
        }
        消息
    }
}

fn 转为C字符串指针(文本: String) -> *mut c_char {
    // 去掉内部 NUL（与旧 CString 语义一致，避免 C 侧 strlen 截断歧义）
    if 文本.contains('\0') {
        crate::stdlib::qi_str::rc_cstr_from_string(文本.replace('\0', ""))
    } else {
        crate::stdlib::qi_str::rc_cstr_from_string(文本)
    }
}

fn 工具安全名称(名称: &str) -> String {
    if !名称.is_empty()
        && 名称
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return 名称.to_string();
    }

    let mut 安全名称 = String::from("qi_tool_");
    for 字节 in 名称.as_bytes() {
        安全名称.push_str(&format!("{:02x}", 字节));
    }
    安全名称
}

fn 保存流历史(流: &LLM流) {
    let mut 会话池 = 获取会话池().lock().unwrap();
    if let Some(会话) = 会话池.get_mut(&流.会话句柄) {
        if 流.带工具 {
            // 工具流：assistant 消息要带 tool_calls，否则后续 continue / tool 结果无法对齐。
            // 续传流历史里已有 user 消息，不再重复 push。
            if !流.是续传 {
                会话.历史.push(json!({ "role": "user", "content": 流.提示 }));
            }
            会话.历史.push(流.组装助手消息());
        } else {
            // 纯文本流（流式问）：只存内容。
            if 流.累计.is_empty() {
                return;
            }
            会话.历史.push(json!({ "role": "user", "content": 流.提示 }));
            会话.历史.push(json!({ "role": "assistant", "content": 流.累计 }));
        }
    }
}

/// 创建LLM会话
///
/// 参数:
/// - endpoint: API端点 (如 "https://api.openai.com/v1/chat/completions")
/// - model: 模型名称 (如 "gpt-3.5-turbo")
/// - api_key: API密钥 (可选，传入空字符串表示不需要)
///
/// 返回: 会话句柄 (>0 成功, <0 失败)
#[no_mangle]
pub extern "C" fn qi_llm_create_session(
    endpoint: *const c_char,
    model: *const c_char,
    api_key: *const c_char,
) -> i64 {
    if endpoint.is_null() || model.is_null() {
        return -1;
    }

    unsafe {
        let 端点 = CStr::from_ptr(endpoint).to_string_lossy().to_string();
        let 模型 = CStr::from_ptr(model).to_string_lossy().to_string();
        let 密钥 = if api_key.is_null() {
            None
        } else {
            let key = CStr::from_ptr(api_key).to_string_lossy().to_string();
            if key.is_empty() {
                None
            } else {
                Some(key)
            }
        };

        let 会话 = LLM会话::创建(端点, 模型, 密钥);

        let mut 计数器 = 获取会话计数器().lock().unwrap();
        *计数器 += 1;
        let 句柄 = *计数器;

        let mut 会话池 = 获取会话池().lock().unwrap();
        会话池.insert(句柄, 会话);

        句柄
    }
}

/// 发送消息到LLM
///
/// 参数:
/// - session_handle: 会话句柄
/// - prompt: 用户提示
///
/// 返回: LLM响应文本 (需要调用 qi_llm_free_string 释放)
#[no_mangle]
pub extern "C" fn qi_llm_chat(session_handle: i64, prompt: *const c_char) -> *mut c_char {
    if prompt.is_null() {
        return 转为C字符串指针("LLM调用失败: 提示为空".to_string());
    }

    let mut 会话池 = 获取会话池().lock().unwrap();

    if let Some(会话) = 会话池.get_mut(&session_handle) {
        unsafe {
            let 提示 = CStr::from_ptr(prompt).to_string_lossy().to_string();

            match 会话.调用API(&提示) {
                Ok(响应) => {
                    // 添加到历史
                    会话.历史.push(json!({
                        "role": "user",
                        "content": 提示.clone()
                    }));
                    会话.历史.push(json!({
                        "role": "assistant",
                        "content": 响应.clone()
                    }));

                    return 转为C字符串指针(响应);
                }
                Err(错误) => {
                    let 错误信息 = format!("LLM调用失败: {}", 错误);
                    return 转为C字符串指针(错误信息);
                }
            }
        }
    }

    转为C字符串指针("LLM调用失败: 无效会话句柄".to_string())
}

/// 打开流式 LLM 对话。
///
/// 返回: 流句柄 (>0 成功, <0 失败)
#[no_mangle]
pub extern "C" fn qi_llm_stream_chat(session_handle: i64, prompt: *const c_char) -> i64 {
    if prompt.is_null() {
        return -1;
    }

    unsafe {
        let 提示 = CStr::from_ptr(prompt).to_string_lossy().to_string();

        let 会话克隆 = {
            let 会话池 = 获取会话池().lock().unwrap();
            match 会话池.get(&session_handle) {
                Some(会话) => 会话.clone(),
                None => return -1,
            }
        };

        let 响应 = match 会话克隆.打开流(&提示) {
            Ok(响应) => 响应,
            Err(_) => return -1,
        };

        let mut 计数器 = 获取流计数器().lock().unwrap();
        *计数器 += 1;
        let 流句柄 = *计数器;

        let mut 流池 = 获取流池().lock().unwrap();
        流池.insert(流句柄, LLM流::创建(session_handle, 提示, 响应, false, false));

        流句柄
    }
}

/// 流式 + 工具：开一个带 tools 的流式对话。内容片段照常通过 读取流 流出，
/// tool_calls 在 runtime 侧按 index 累积，结束后用 流取助手消息 取完整 assistant 消息。
#[no_mangle]
pub extern "C" fn qi_llm_stream_chat_with_tools(session_handle: i64, prompt: *const c_char) -> i64 {
    if prompt.is_null() {
        return -1;
    }
    unsafe {
        let 提示 = CStr::from_ptr(prompt).to_string_lossy().to_string();
        let 会话克隆 = {
            let 会话池 = 获取会话池().lock().unwrap();
            match 会话池.get(&session_handle) {
                Some(会话) => 会话.clone(),
                None => return -1,
            }
        };
        let 响应 = match 会话克隆.打开流带工具(&提示) {
            Ok(响应) => 响应,
            Err(_) => return -1,
        };
        let 流句柄 = {
            let mut 计数器 = 获取流计数器().lock().unwrap();
            *计数器 += 1;
            *计数器
        };
        let mut 流池 = 获取流池().lock().unwrap();
        流池.insert(流句柄, LLM流::创建(session_handle, 提示, 响应, true, false));
        流句柄
    }
}

/// 工具结果回写后，流式继续推理。返回新的流句柄。
#[no_mangle]
pub extern "C" fn qi_llm_stream_continue_with_tools(session_handle: i64) -> i64 {
    let 会话克隆 = {
        let 会话池 = 获取会话池().lock().unwrap();
        match 会话池.get(&session_handle) {
            Some(会话) => 会话.clone(),
            None => return -1,
        }
    };
    let 响应 = match 会话克隆.打开续传流() {
        Ok(响应) => 响应,
        Err(_) => return -1,
    };
    let 流句柄 = {
        let mut 计数器 = 获取流计数器().lock().unwrap();
        *计数器 += 1;
        *计数器
    };
    let mut 流池 = 获取流池().lock().unwrap();
    流池.insert(流句柄, LLM流::创建(session_handle, String::new(), 响应, true, true));
    流句柄
}

/// 取本轮流式对话拼好的 assistant 消息 JSON（含 content + tool_calls）。
/// 须在该流读到底（读取流 返回空）后调用；可直接喂给 has_tool_call / get_tool_call_* 派发。
#[no_mangle]
pub extern "C" fn qi_llm_stream_assistant_message(stream_handle: i64) -> *mut c_char {
    let 流池 = 获取流池().lock().unwrap();
    if let Some(流) = 流池.get(&stream_handle) {
        return 转为C字符串指针(流.组装助手消息().to_string());
    }
    转为C字符串指针(String::new())
}

/// 读取流式对话的下一个片段。结束时返回空字符串。
#[no_mangle]
pub extern "C" fn qi_llm_stream_next(stream_handle: i64) -> *mut c_char {
    let mut 流池 = 获取流池().lock().unwrap();

    if let Some(流) = 流池.get_mut(&stream_handle) {
        match 流.读取下个片段() {
            Ok(Some(片段)) => return 转为C字符串指针(片段),
            Ok(None) => return 转为C字符串指针(String::new()),
            Err(错误) => return 转为C字符串指针(format!("流式读取失败: {}", 错误)),
        }
    }

    转为C字符串指针("".to_string())
}

/// 关闭流式对话，并把已经收到的内容写入会话历史。
#[no_mangle]
pub extern "C" fn qi_llm_stream_close(stream_handle: i64) -> i64 {
    let 流 = {
        let mut 流池 = 获取流池().lock().unwrap();
        流池.remove(&stream_handle)
    };

    if let Some(流) = 流 {
        保存流历史(&流);
        return 1;
    }

    -1
}

/// 注册工具定义。
///
/// parameters_json 应是 JSON Schema 对象字符串，例如 {"type":"object","properties":{...}}
#[no_mangle]
pub extern "C" fn qi_llm_register_tool(
    session_handle: i64,
    tool_name: *const c_char,
    tool_description: *const c_char,
    parameters_json: *const c_char,
) -> i64 {
    if tool_name.is_null() || tool_description.is_null() || parameters_json.is_null() {
        return -1;
    }

    let mut 会话池 = 获取会话池().lock().unwrap();
    if let Some(会话) = 会话池.get_mut(&session_handle) {
        unsafe {
            let 工具名 = CStr::from_ptr(tool_name).to_string_lossy().to_string();
            let 工具描述 = CStr::from_ptr(tool_description)
                .to_string_lossy()
                .to_string();
            let 参数文本 = CStr::from_ptr(parameters_json)
                .to_string_lossy()
                .to_string();
            let 安全工具名 = 工具安全名称(&工具名);
            let 参数结构 = serde_json::from_str::<Value>(&参数文本)
                .unwrap_or_else(|_| json!({"type": "object", "properties": {}}));

            会话.工具列表.push(json!({
                "type": "function",
                "function": {
                    "name": 安全工具名,
                    "description": format!("{}（Qi工具名：{}）", 工具描述, 工具名),
                    "parameters": 参数结构
                }
            }));
            会话.工具名称映射.insert(安全工具名, 工具名);
            return 1;
        }
    }

    -1
}

/// 清空会话工具定义。
#[no_mangle]
pub extern "C" fn qi_llm_clear_tools(session_handle: i64) -> i64 {
    let mut 会话池 = 获取会话池().lock().unwrap();
    if let Some(会话) = 会话池.get_mut(&session_handle) {
        会话.工具列表.clear();
        会话.工具名称映射.clear();
        return 1;
    }
    -1
}

/// 带工具调用能力的对话，返回 assistant message JSON。
#[no_mangle]
pub extern "C" fn qi_llm_chat_with_tools(
    session_handle: i64,
    prompt: *const c_char,
) -> *mut c_char {
    if prompt.is_null() {
        return 转为C字符串指针("LLM调用失败: 提示为空".to_string());
    }

    let mut 会话池 = 获取会话池().lock().unwrap();
    if let Some(会话) = 会话池.get_mut(&session_handle) {
        unsafe {
            let 提示 = CStr::from_ptr(prompt).to_string_lossy().to_string();
            match 会话.调用工具API(&提示) {
                Ok(消息) => {
                    会话.历史.push(json!({
                        "role": "user",
                        "content": 提示
                    }));
                    会话.历史.push(消息.clone());
                    return 转为C字符串指针(消息.to_string());
                }
                Err(错误) => return 转为C字符串指针(format!("工具对话失败: {}", 错误)),
            }
        }
    }

    转为C字符串指针("LLM调用失败: 无效会话句柄".to_string())
}

/// 添加工具结果后继续模型推理，返回 assistant message JSON。
#[no_mangle]
pub extern "C" fn qi_llm_continue_with_tools(session_handle: i64) -> *mut c_char {
    let mut 会话池 = 获取会话池().lock().unwrap();
    if let Some(会话) = 会话池.get_mut(&session_handle) {
        match 会话.继续工具API() {
            Ok(消息) => {
                会话.历史.push(消息.clone());
                return 转为C字符串指针(消息.to_string());
            }
            Err(错误) => return 转为C字符串指针(format!("继续工具对话失败: {}", 错误)),
        }
    }

    转为C字符串指针("LLM调用失败: 无效会话句柄".to_string())
}

fn 解析工具调用(assistant_message_json: *const c_char) -> Option<Value> {
    解析工具调用按索引(assistant_message_json, 0)
}

/// 按 index 取 tool_calls[index]。支持 parallel tool_calls：模型一次返
/// N 个工具调用时，harness 循环 0..N 各取一个 dispatch。
fn 解析工具调用按索引(
    assistant_message_json: *const c_char,
    index: usize,
) -> Option<Value> {
    if assistant_message_json.is_null() {
        return None;
    }
    unsafe {
        let 文本 = CStr::from_ptr(assistant_message_json)
            .to_string_lossy()
            .to_string();
        let 消息: Value = serde_json::from_str(&文本).ok()?;
        消息
            .get("tool_calls")
            .and_then(|calls| calls.get(index))
            .cloned()
    }
}

/// 取 tool_calls 数组长度。模型一次返多个 parallel tool_calls 时用。
#[no_mangle]
pub extern "C" fn qi_llm_get_tool_call_count(assistant_message_json: *const c_char) -> i64 {
    if assistant_message_json.is_null() {
        return 0;
    }
    unsafe {
        let 文本 = CStr::from_ptr(assistant_message_json)
            .to_string_lossy()
            .to_string();
        let 消息: Value = match serde_json::from_str(&文本) {
            Ok(v) => v,
            Err(_) => return 0,
        };
        消息
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .map(|a| a.len() as i64)
            .unwrap_or(0)
    }
}

/// 按 index 取第 i 个工具调用的 ID
#[no_mangle]
pub extern "C" fn qi_llm_get_tool_call_id_at(
    assistant_message_json: *const c_char,
    index: i64,
) -> *mut c_char {
    let 调用 = match 解析工具调用按索引(assistant_message_json, index as usize) {
        Some(c) => c,
        None => return std::ptr::null_mut(),
    };
    let id = 调用
        .get("id")
        .and_then(|i| i.as_str())
        .unwrap_or("")
        .to_string();
    转为C字符串指针(id)
}

/// 按 index 取第 i 个工具调用的名称（中文化）
#[no_mangle]
pub extern "C" fn qi_llm_get_tool_call_name_at(
    session_handle: i64,
    assistant_message_json: *const c_char,
    index: i64,
) -> *mut c_char {
    let 调用 = match 解析工具调用按索引(assistant_message_json, index as usize) {
        Some(c) => c,
        None => return std::ptr::null_mut(),
    };
    let 安全名称 = 调用
        .get("function")
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("")
        .to_string();
    let 会话池 = 获取会话池().lock().unwrap();
    let 名称 = 会话池
        .get(&session_handle)
        .and_then(|s| s.工具名称映射.get(&安全名称))
        .cloned()
        .unwrap_or(安全名称);
    转为C字符串指针(名称)
}

/// 按 index 取第 i 个工具调用的参数 JSON
#[no_mangle]
pub extern "C" fn qi_llm_get_tool_call_arguments_at(
    assistant_message_json: *const c_char,
    index: i64,
) -> *mut c_char {
    let 调用 = match 解析工具调用按索引(assistant_message_json, index as usize) {
        Some(c) => c,
        None => return std::ptr::null_mut(),
    };
    let 参数 = 调用.get("function").and_then(|f| f.get("arguments"));
    let 文本 = match 参数 {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => String::new(),
    };
    转为C字符串指针(文本)
}

/// 判断 assistant message JSON 是否包含工具调用。
#[no_mangle]
pub extern "C" fn qi_llm_has_tool_call(assistant_message_json: *const c_char) -> i64 {
    if 解析工具调用(assistant_message_json).is_some() {
        1
    } else {
        0
    }
}

/// 获取第一个工具调用 ID。
#[no_mangle]
pub extern "C" fn qi_llm_get_tool_call_id(assistant_message_json: *const c_char) -> *mut c_char {
    let 调用 = match 解析工具调用(assistant_message_json) {
        Some(调用) => 调用,
        None => return std::ptr::null_mut(),
    };

    let 调用ID = 调用
        .get("id")
        .and_then(|id| id.as_str())
        .unwrap_or("")
        .to_string();

    转为C字符串指针(调用ID)
}

/// 获取第一个工具调用名称；返回 Qi 原始中文工具名。
#[no_mangle]
pub extern "C" fn qi_llm_get_tool_call_name(
    session_handle: i64,
    assistant_message_json: *const c_char,
) -> *mut c_char {
    let 调用 = match 解析工具调用(assistant_message_json) {
        Some(调用) => 调用,
        None => return std::ptr::null_mut(),
    };

    let 安全名称 = 调用
        .get("function")
        .and_then(|func| func.get("name"))
        .and_then(|name| name.as_str())
        .unwrap_or("")
        .to_string();

    let 会话池 = 获取会话池().lock().unwrap();
    let 名称 = 会话池
        .get(&session_handle)
        .and_then(|会话| 会话.工具名称映射.get(&安全名称))
        .cloned()
        .unwrap_or(安全名称);

    转为C字符串指针(名称)
}

/// 获取第一个工具调用参数 JSON。
#[no_mangle]
pub extern "C" fn qi_llm_get_tool_call_arguments(
    assistant_message_json: *const c_char,
) -> *mut c_char {
    let 调用 = match 解析工具调用(assistant_message_json) {
        Some(调用) => 调用,
        None => return std::ptr::null_mut(),
    };

    let 参数 = 调用.get("function").and_then(|func| func.get("arguments"));

    let 参数文本 = match 参数 {
        Some(Value::String(s)) => s.clone(),
        Some(value) => value.to_string(),
        None => String::new(),
    };

    转为C字符串指针(参数文本)
}

/// 添加工具执行结果到会话历史，供下一次工具对话继续推理。
#[no_mangle]
pub extern "C" fn qi_llm_add_tool_result(
    session_handle: i64,
    tool_call_id: *const c_char,
    tool_name: *const c_char,
    result: *const c_char,
) -> i64 {
    if tool_call_id.is_null() || result.is_null() {
        return -1;
    }

    let mut 会话池 = 获取会话池().lock().unwrap();
    if let Some(会话) = 会话池.get_mut(&session_handle) {
        unsafe {
            let 调用ID = CStr::from_ptr(tool_call_id).to_string_lossy().to_string();
            let 结果 = CStr::from_ptr(result).to_string_lossy().to_string();
            let 工具名 = if tool_name.is_null() {
                String::new()
            } else {
                CStr::from_ptr(tool_name).to_string_lossy().to_string()
            };
            let 安全工具名 = 工具安全名称(&工具名);

            let mut 工具消息 = json!({
                "role": "tool",
                "tool_call_id": 调用ID,
                "content": 结果
            });

            if !安全工具名.is_empty() {
                工具消息["name"] = json!(安全工具名);
            }

            会话.历史.push(工具消息);
            return 1;
        }
    }

    -1
}

/// 设置会话配置参数
///
/// 参数:
/// - session_handle: 会话句柄
/// - key: 配置键 (如 "temperature", "max_tokens")
/// - value: 配置值
///
/// 返回: 1 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_llm_set_config(
    session_handle: i64,
    key: *const c_char,
    value: *const c_char,
) -> i64 {
    if key.is_null() || value.is_null() {
        return -1;
    }

    let mut 会话池 = 获取会话池().lock().unwrap();

    if let Some(会话) = 会话池.get_mut(&session_handle) {
        unsafe {
            let 键 = CStr::from_ptr(key).to_string_lossy().to_string();
            let 值 = CStr::from_ptr(value).to_string_lossy().to_string();
            会话.配置.insert(键, 值);
            return 1;
        }
    }

    -1
}

/// 清空对话历史
///
/// 参数:
/// - session_handle: 会话句柄
///
/// 返回: 1 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_llm_clear_history(session_handle: i64) -> i64 {
    let mut 会话池 = 获取会话池().lock().unwrap();

    if let Some(会话) = 会话池.get_mut(&session_handle) {
        会话.历史.clear();
        return 1;
    }

    -1
}

/// 获取对话历史记录数
///
/// 参数:
/// - session_handle: 会话句柄
///
/// 返回: 历史记录数 (>=0 成功, <0 失败)
#[no_mangle]
pub extern "C" fn qi_llm_get_history_count(session_handle: i64) -> i64 {
    let 会话池 = 获取会话池().lock().unwrap();

    if let Some(会话) = 会话池.get(&session_handle) {
        return 会话.历史.len() as i64;
    }

    -1
}

/// 获取整段对话历史（OpenAI 消息数组的 JSON 字符串）。
///
/// 历史里每条是 {"role":..,"content":..[, "tool_calls"/"tool_call_id"..]}。
/// 上下文窗口管理（qi-harness 上下文 模块）靠它把历史读到 Qi 侧，数 token、
/// 决定丢哪些/摘要哪些，再用 qi_llm_set_history_json 写回。
///
/// 会话不存在返回空数组 "[]"。返回串需 qi_llm_free_string 释放。
#[no_mangle]
pub extern "C" fn qi_llm_get_history_json(session_handle: i64) -> *mut c_char {
    let 会话池 = 获取会话池().lock().unwrap();
    let 文本 = if let Some(会话) = 会话池.get(&session_handle) {
        Value::Array(会话.历史.clone()).to_string()
    } else {
        "[]".to_string()
    };
    crate::stdlib::qi_str::rc_cstr_from_string(文本)
}

/// 用一个 JSON 数组字符串**整体替换**对话历史。
///
/// 入参必须是消息对象数组（[{"role":..,"content":..}, ...]）。解析失败或不是
/// 数组则不改动、返回 -1。成功返回替换后历史条数（>=0）。这是上下文压缩的写回口：
/// Qi 侧构造「1 条摘要 system 消息 + 最近 M 条」的新数组塞回来即可。
#[no_mangle]
pub extern "C" fn qi_llm_set_history_json(
    session_handle: i64,
    history_json: *const c_char,
) -> i64 {
    if history_json.is_null() {
        return -1;
    }
    let 文本 = unsafe { CStr::from_ptr(history_json) }.to_string_lossy().to_string();
    let 解析: Value = match serde_json::from_str(&文本) {
        Ok(v) => v,
        Err(_) => return -1,
    };
    let 新历史 = match 解析 {
        Value::Array(a) => a,
        _ => return -1,
    };
    let mut 会话池 = 获取会话池().lock().unwrap();
    if let Some(会话) = 会话池.get_mut(&session_handle) {
        let 条数 = 新历史.len() as i64;
        会话.历史 = 新历史;
        条数
    } else {
        -1
    }
}

/// 设置会话 token 预算上限（0 = 取消限制）。设置后每次非流式调用自动累计 usage.total，
/// 累计达到上限 → 后续调用**直接拒绝**（返回 "LLM调用失败: 预算超限..."，不打 API）。
/// 返回 1 成功 / -1 会话不存在。
#[no_mangle]
pub extern "C" fn qi_llm_set_budget(session_handle: i64, limit: i64) -> i64 {
    let mut 会话池 = 获取会话池().lock().unwrap();
    if let Some(会话) = 会话池.get_mut(&session_handle) {
        会话.预算上限 = limit.max(0);
        return 1;
    }
    -1
}

/// 会话累计已用 token（预算记账值）。会话不存在 → 0。
#[no_mangle]
pub extern "C" fn qi_llm_budget_used(session_handle: i64) -> i64 {
    let 会话池 = 获取会话池().lock().unwrap();
    会话池
        .get(&session_handle)
        .map(|s| s.累计用量)
        .unwrap_or(0)
}

/// 最近一次非流式请求的 token 用量，返回 JSON 串 {"prompt":..,"completion":..,"total":..}。
/// 会话不存在或尚无请求 → 全 0。用于成本/预算统计（span 里记真实 tokens）。
#[no_mangle]
pub extern "C" fn qi_llm_last_usage(session_handle: i64) -> *mut c_char {
    let 会话池 = 获取会话池().lock().unwrap();
    let (p, c, t) = 会话池
        .get(&session_handle)
        .map(|s| s.最近用量)
        .unwrap_or((0, 0, 0));
    let 文本 = json!({"prompt": p, "completion": c, "total": t}).to_string();
    crate::stdlib::qi_str::rc_cstr_from_string(文本)
}

/// 关闭LLM会话
///
/// 参数:
/// - session_handle: 会话句柄
///
/// 返回: 1 成功, -1 失败
#[no_mangle]
pub extern "C" fn qi_llm_close_session(session_handle: i64) -> i64 {
    let mut 会话池 = 获取会话池().lock().unwrap();

    if 会话池.remove(&session_handle).is_some() {
        return 1;
    }

    -1
}

/// 释放LLM返回的字符串
///
/// 参数:
/// - s: 字符串指针
#[no_mangle]
pub extern "C" fn qi_llm_free_string(s: *mut c_char) {
    // 委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃
    crate::stdlib::qi_str::rc_cstr_release(s);
}

// ============================================================================
// 异步 LLM API
// ============================================================================

use crate::async_runtime::future::Future;
use std::thread;

/// 异步发送消息到LLM (返回 未来<字符串>)
///
/// 参数:
/// - session_handle: 会话句柄
/// - prompt: 用户提示
///
/// 返回: Future 指针 (需要使用 等待 关键字获取结果)
#[no_mangle]
pub extern "C" fn qi_llm_chat_async(session_handle: i64, prompt: *const c_char) -> *mut Future {
    if prompt.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 提示 = CStr::from_ptr(prompt).to_string_lossy().to_string();

        // 获取会话的克隆
        let mut 会话克隆 = {
            let 会话池 = 获取会话池().lock().unwrap();
            match 会话池.get(&session_handle) {
                Some(会话) => 会话.clone(),
                None => return std::ptr::null_mut(),
            }
        };

        // 创建一个 pending Future
        let future_state = std::sync::Arc::new(std::sync::Mutex::new(
            crate::async_runtime::future::FutureState::Pending,
        ));
        let future_value = std::sync::Arc::new(std::sync::Mutex::new(None));
        let future_error = std::sync::Arc::new(std::sync::Mutex::new(None));

        let state_clone = future_state.clone();
        let value_clone = future_value.clone();
        let error_clone = future_error.clone();

        // 在后台线程中执行 HTTP 请求
        thread::spawn(move || {
            match 会话克隆.调用API(&提示) {
                Ok(响应) => {
                    // 更新会话历史
                    {
                        let mut 会话池 = 获取会话池().lock().unwrap();
                        if let Some(会话) = 会话池.get_mut(&session_handle) {
                            会话.历史.push(json!({
                                "role": "user",
                                "content": 提示.clone()
                            }));
                            会话.历史.push(json!({
                                "role": "assistant",
                                "content": 响应.clone()
                            }));
                        }
                    }

                    // 更新 Future 状态
                    *value_clone.lock().unwrap() = Some(
                        crate::async_runtime::future::FutureValue::String(响应),
                    );
                    *state_clone.lock().unwrap() =
                        crate::async_runtime::future::FutureState::Completed;
                }
                Err(错误) => {
                    *error_clone.lock().unwrap() = Some(format!("LLM异步调用失败: {}", 错误));
                    *state_clone.lock().unwrap() =
                        crate::async_runtime::future::FutureState::Failed;
                }
            }
        });

        // 返回 Future 指针
        let future = Box::new(Future {
            state: future_state,
            value: future_value,
            error: future_error,
            notify: std::sync::Arc::new(tokio::sync::Notify::new()),
            sm_wakers: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        });
        Box::into_raw(future)
    }
}
