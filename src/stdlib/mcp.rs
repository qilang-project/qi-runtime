//! MCP服务器模块 (Model Context Protocol Server Module)
//!
//! 提供 MCP 服务器功能的中文接口
//! Provides MCP server capabilities with Chinese interface

#![allow(non_snake_case)]

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;

/// MCP错误
#[derive(Debug, thiserror::Error)]
pub enum MCP错误 {
    #[error("服务器错误: {0}")]
    服务器错误(String),

    #[error("工具错误: {0}")]
    工具错误(String),

    #[error("资源错误: {0}")]
    资源错误(String),

    #[error("提示错误: {0}")]
    提示错误(String),

    #[error("协议错误: {0}")]
    协议错误(String),

    #[error("JSON错误: {0}")]
    JSON错误(String),

    #[error("传输错误: {0}")]
    传输错误(String),

    #[error("无效参数: {0}")]
    无效参数(String),
}

pub type MCP结果<T> = Result<T, MCP错误>;

/// 工具参数定义
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct 工具参数 {
    /// 参数名称
    pub 名称: String,
    /// 参数类型 (string, number, boolean, object, array)
    pub 类型: String,
    /// 参数描述
    pub 描述: String,
    /// 是否必需
    pub 必需: bool,
    /// 默认值
    pub 默认值: Option<JsonValue>,
}

impl 工具参数 {
    /// 创建新的工具参数
    pub fn 创建(名称: String, 类型: String, 描述: String, 必需: bool) -> Self {
        Self {
            名称,
            类型,
            描述,
            必需,
            默认值: None,
        }
    }

    /// 设置默认值
    pub fn 设置默认值(mut self, 默认值: JsonValue) -> Self {
        self.默认值 = Some(默认值);
        self
    }
}

/// 工具回调函数类型
pub type 工具回调函数 =
    Box<dyn Fn(&HashMap<String, JsonValue>) -> MCP结果<JsonValue> + Send + Sync>;

/// MCP工具定义
#[derive(Clone)]
pub struct MCP工具 {
    /// 工具名称
    pub 名称: String,
    /// 工具描述
    pub 描述: String,
    /// 参数列表
    pub 参数列表: Vec<工具参数>,
    /// 工具实现函数 (参数 -> 结果)
    pub 执行函数: Option<fn(&HashMap<String, JsonValue>) -> MCP结果<JsonValue>>,
    /// 工具回调ID (用于从Qi语言层设置回调, 旧字符串方式)
    pub 回调ID: Option<String>,
    /// 工具回调闭包对象指针 (Qi 闭包对象地址, 用于stdio服务器直接调用)
    /// 对象布局: [fn_ptr_at_offset_0, env_slots...]
    /// 调用: trampoline(env=obj_ptr, args_json) → result_str
    pub 回调指针: Option<usize>,
    /// 原始 inputSchema 覆盖 (Qi 层直接给出完整 JSON Schema 字符串时使用)。
    /// 存在时 转为Schema 直接采用它, 不再从 参数列表 重建。
    pub 原始输入schema: Option<JsonValue>,
}

impl std::fmt::Debug for MCP工具 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MCP工具")
            .field("名称", &self.名称)
            .field("描述", &self.描述)
            .field("参数列表", &self.参数列表)
            .field("执行函数", &self.执行函数.is_some())
            .field("回调ID", &self.回调ID)
            .field("回调指针", &self.回调指针)
            .field("原始输入schema", &self.原始输入schema.is_some())
            .finish()
    }
}

impl MCP工具 {
    /// 创建新工具
    pub fn 创建(名称: String, 描述: String) -> Self {
        Self {
            名称,
            描述,
            参数列表: Vec::new(),
            执行函数: None,
            回调ID: None,
            回调指针: None,
            原始输入schema: None,
        }
    }

    /// 设置回调ID
    pub fn 设置回调ID(mut self, 回调ID: String) -> Self {
        self.回调ID = Some(回调ID);
        self
    }

    /// 设置回调闭包对象指针 (Qi 闭包对象地址)
    pub fn 设置回调指针(mut self, 指针: usize) -> Self {
        self.回调指针 = Some(指针);
        self
    }

    /// 添加参数
    pub fn 添加参数(mut self, 参数: 工具参数) -> Self {
        self.参数列表.push(参数);
        self
    }

    /// 设置执行函数
    pub fn 设置执行函数(
        mut self,
        函数: fn(&HashMap<String, JsonValue>) -> MCP结果<JsonValue>,
    ) -> Self {
        self.执行函数 = Some(函数);
        self
    }

    /// 执行工具
    pub fn 执行(&self, 参数: &HashMap<String, JsonValue>) -> MCP结果<JsonValue> {
        // 验证必需参数
        for 工具参数 in &self.参数列表 {
            if 工具参数.必需 && !参数.contains_key(&工具参数.名称) {
                return Err(MCP错误::工具错误(format!(
                    "缺少必需参数: {}",
                    工具参数.名称
                )));
            }
        }

        // 执行函数
        match self.执行函数 {
            Some(函数) => 函数(参数),
            None => Err(MCP错误::工具错误("工具未实现执行函数".to_string())),
        }
    }

    /// 转换为JSON Schema格式
    pub fn 转为Schema(&self) -> JsonValue {
        // 若 Qi 层已给出完整 inputSchema 字符串, 直接采用它。
        if let Some(schema) = &self.原始输入schema {
            return serde_json::json!({
                "name": self.名称,
                "description": self.描述,
                "inputSchema": schema
            });
        }

        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();

        for 参数 in &self.参数列表 {
            let mut param_schema = serde_json::Map::new();
            param_schema.insert("type".to_string(), JsonValue::String(参数.类型.clone()));
            param_schema.insert(
                "description".to_string(),
                JsonValue::String(参数.描述.clone()),
            );

            if let Some(ref 默认值) = 参数.默认值 {
                param_schema.insert("default".to_string(), 默认值.clone());
            }

            properties.insert(参数.名称.clone(), JsonValue::Object(param_schema));

            if 参数.必需 {
                required.push(JsonValue::String(参数.名称.clone()));
            }
        }

        serde_json::json!({
            "name": self.名称,
            "description": self.描述,
            "inputSchema": {
                "type": "object",
                "properties": properties,
                "required": required
            }
        })
    }
}

/// MCP资源类型
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum 资源类型 {
    /// 文本资源
    文本,
    /// 二进制资源
    二进制,
    /// JSON资源
    JSON,
}

impl 资源类型 {
    pub fn 转为字符串(&self) -> &str {
        match self {
            资源类型::文本 => "text",
            资源类型::二进制 => "blob",
            资源类型::JSON => "json",
        }
    }
}

/// MCP资源
#[derive(Debug, Clone)]
pub struct MCP资源 {
    /// 资源URI
    pub uri: String,
    /// 资源名称
    pub 名称: String,
    /// 资源描述
    pub 描述: String,
    /// 资源类型
    pub 类型: 资源类型,
    /// MIME类型
    pub mime类型: Option<String>,
    /// 资源内容 (根据类型存储不同格式)
    pub 内容: Option<资源内容>,
}

/// 资源内容枚举
#[derive(Debug, Clone)]
pub enum 资源内容 {
    /// 文本内容
    文本(String),
    /// 二进制内容 (base64编码)
    二进制(Vec<u8>),
    /// JSON内容
    JSON(JsonValue),
}

impl MCP资源 {
    /// 创建新资源
    pub fn 创建(uri: String, 名称: String, 描述: String, 类型: 资源类型) -> Self {
        Self {
            uri,
            名称,
            描述,
            类型,
            mime类型: None,
            内容: None,
        }
    }

    /// 设置MIME类型
    pub fn 设置MIME类型(mut self, mime类型: String) -> Self {
        self.mime类型 = Some(mime类型);
        self
    }

    /// 设置文本内容
    pub fn 设置文本内容(mut self, 内容: String) -> Self {
        self.内容 = Some(资源内容::文本(内容));
        self
    }

    /// 设置二进制内容
    pub fn 设置二进制内容(mut self, 内容: Vec<u8>) -> Self {
        self.内容 = Some(资源内容::二进制(内容));
        self
    }

    /// 设置JSON内容
    pub fn 设置JSON内容(mut self, 内容: JsonValue) -> Self {
        self.内容 = Some(资源内容::JSON(内容));
        self
    }

    /// 读取资源内容
    pub fn 读取内容(&self) -> MCP结果<&资源内容> {
        self.内容
            .as_ref()
            .ok_or_else(|| MCP错误::资源错误("资源没有设置内容".to_string()))
    }

    /// 读取文本内容
    pub fn 读取文本(&self) -> MCP结果<&str> {
        match self.读取内容()? {
            资源内容::文本(text) => Ok(text.as_str()),
            _ => Err(MCP错误::资源错误("资源类型不是文本".to_string())),
        }
    }

    /// 读取二进制内容
    pub fn 读取二进制(&self) -> MCP结果<&[u8]> {
        match self.读取内容()? {
            资源内容::二进制(bytes) => Ok(bytes.as_slice()),
            _ => Err(MCP错误::资源错误("资源类型不是二进制".to_string())),
        }
    }

    /// 读取JSON内容
    pub fn 读取JSON(&self) -> MCP结果<&JsonValue> {
        match self.读取内容()? {
            资源内容::JSON(json) => Ok(json),
            _ => Err(MCP错误::资源错误("资源类型不是JSON".to_string())),
        }
    }

    /// 转换为JSON格式
    pub fn 转为JSON(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert("uri".to_string(), JsonValue::String(self.uri.clone()));
        obj.insert("name".to_string(), JsonValue::String(self.名称.clone()));
        obj.insert(
            "description".to_string(),
            JsonValue::String(self.描述.clone()),
        );
        obj.insert(
            "type".to_string(),
            JsonValue::String(self.类型.转为字符串().to_string()),
        );

        if let Some(ref mime) = self.mime类型 {
            obj.insert("mimeType".to_string(), JsonValue::String(mime.clone()));
        }

        JsonValue::Object(obj)
    }
}

/// MCP提示参数
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct 提示参数 {
    /// 参数名称
    pub 名称: String,
    /// 参数描述
    pub 描述: String,
    /// 是否必需
    pub 必需: bool,
}

impl 提示参数 {
    /// 创建新的提示参数
    pub fn 创建(名称: String, 描述: String, 必需: bool) -> Self {
        Self {
            名称, 描述, 必需
        }
    }
}

/// MCP提示
#[derive(Debug, Clone)]
pub struct MCP提示 {
    /// 提示名称
    pub 名称: String,
    /// 提示描述
    pub 描述: String,
    /// 参数列表
    pub 参数列表: Vec<提示参数>,
    /// 提示模板
    pub 模板: String,
}

impl MCP提示 {
    /// 创建新提示
    pub fn 创建(名称: String, 描述: String, 模板: String) -> Self {
        Self {
            名称,
            描述,
            参数列表: Vec::new(),
            模板,
        }
    }

    /// 添加参数
    pub fn 添加参数(mut self, 参数: 提示参数) -> Self {
        self.参数列表.push(参数);
        self
    }

    /// 填充提示模板
    pub fn 填充(&self, 参数: &HashMap<String, String>) -> MCP结果<String> {
        let mut 结果 = self.模板.clone();

        // 验证必需参数
        for 提示参数 in &self.参数列表 {
            if 提示参数.必需 && !参数.contains_key(&提示参数.名称) {
                return Err(MCP错误::提示错误(format!(
                    "缺少必需参数: {}",
                    提示参数.名称
                )));
            }
        }

        // 替换变量
        for (键, 值) in 参数 {
            let 占位符 = format!("{{{}}}", 键);
            结果 = 结果.replace(&占位符, 值);
        }

        Ok(结果)
    }

    /// 转换为JSON格式
    pub fn 转为JSON(&self) -> JsonValue {
        let arguments: Vec<JsonValue> = self
            .参数列表
            .iter()
            .map(|p| {
                serde_json::json!({
                    "name": p.名称,
                    "description": p.描述,
                    "required": p.必需
                })
            })
            .collect();

        serde_json::json!({
            "name": self.名称,
            "description": self.描述,
            "arguments": arguments
        })
    }
}

/// MCP服务器配置
#[derive(Debug, Clone)]
pub struct MCP服务器配置 {
    /// 服务器名称
    pub 名称: String,
    /// 服务器版本
    pub 版本: String,
    /// 服务器描述
    pub 描述: Option<String>,
    /// 协议版本
    pub 协议版本: String,
}

impl Default for MCP服务器配置 {
    fn default() -> Self {
        Self {
            名称: "Qi MCP Server".to_string(),
            版本: "0.1.0".to_string(),
            描述: None,
            协议版本: "2025-06-18".to_string(),
        }
    }
}

/// MCP服务器
#[derive(Debug)]
pub struct MCP服务器 {
    /// 服务器配置
    配置: MCP服务器配置,
    /// 注册的工具（IndexMap：迭代序 = 注册序，tools/list 输出跨运行确定）
    工具表: IndexMap<String, MCP工具>,
    /// 注册的资源（同上，resources/list 按注册序）
    资源表: IndexMap<String, MCP资源>,
    /// 注册的提示（同上，prompts/list 按注册序）
    提示表: IndexMap<String, MCP提示>,
    /// 是否正在运行
    运行中: bool,
    /// 当前日志级别（logging/setLevel 设置）
    pub 日志级别: String,
}

impl MCP服务器 {
    /// 创建新的MCP服务器
    pub fn 创建(配置: MCP服务器配置) -> Self {
        Self {
            配置,
            工具表: IndexMap::new(),
            资源表: IndexMap::new(),
            提示表: IndexMap::new(),
            运行中: false,
            日志级别: "info".to_string(),
        }
    }

    /// 注册工具
    pub fn 注册工具(&mut self, 工具: MCP工具) -> MCP结果<()> {
        if self.工具表.contains_key(&工具.名称) {
            return Err(MCP错误::工具错误(
                format!("工具已存在: {}", 工具.名称),
            ));
        }
        self.工具表.insert(工具.名称.clone(), 工具);
        Ok(())
    }

    /// 注册资源
    pub fn 注册资源(&mut self, 资源: MCP资源) -> MCP结果<()> {
        if self.资源表.contains_key(&资源.uri) {
            return Err(MCP错误::资源错误(format!("资源已存在: {}", 资源.uri)));
        }
        self.资源表.insert(资源.uri.clone(), 资源);
        Ok(())
    }

    /// 注册提示
    pub fn 注册提示(&mut self, 提示: MCP提示) -> MCP结果<()> {
        if self.提示表.contains_key(&提示.名称) {
            return Err(MCP错误::提示错误(
                format!("提示已存在: {}", 提示.名称),
            ));
        }
        self.提示表.insert(提示.名称.clone(), 提示);
        Ok(())
    }

    /// 获取工具列表
    pub fn 获取工具列表(&self) -> Vec<JsonValue> {
        self.工具表.values().map(|工具| 工具.转为Schema()).collect()
    }

    /// 获取资源列表
    pub fn 获取资源列表(&self) -> Vec<JsonValue> {
        self.资源表.values().map(|资源| 资源.转为JSON()).collect()
    }

    /// 获取提示列表
    pub fn 获取提示列表(&self) -> Vec<JsonValue> {
        self.提示表.values().map(|提示| 提示.转为JSON()).collect()
    }

    /// 执行工具
    pub fn 执行工具(
        &self,
        工具名: &str,
        参数: &HashMap<String, JsonValue>,
    ) -> MCP结果<JsonValue> {
        let 工具 = self
            .工具表
            .get(工具名)
            .ok_or_else(|| MCP错误::工具错误(format!("工具不存在: {}", 工具名)))?;

        工具.执行(参数)
    }

    /// 获取资源
    pub fn 获取资源(&self, uri: &str) -> MCP结果<&MCP资源> {
        self.资源表
            .get(uri)
            .ok_or_else(|| MCP错误::资源错误(format!("资源不存在: {}", uri)))
    }

    /// 获取提示
    pub fn 获取提示(&self, 名称: &str) -> MCP结果<&MCP提示> {
        self.提示表
            .get(名称)
            .ok_or_else(|| MCP错误::提示错误(format!("提示不存在: {}", 名称)))
    }

    /// 为工具添加参数
    pub fn 为工具添加参数(
        &mut self, 工具名: &str, 参数: 工具参数
    ) -> MCP结果<()> {
        let 工具 = self
            .工具表
            .get_mut(工具名)
            .ok_or_else(|| MCP错误::工具错误(format!("工具不存在: {}", 工具名)))?;

        工具.参数列表.push(参数);
        Ok(())
    }

    /// 设置工具回调ID
    pub fn 设置工具回调ID(&mut self, 工具名: &str, 回调ID: String) -> MCP结果<()> {
        let 工具 = self
            .工具表
            .get_mut(工具名)
            .ok_or_else(|| MCP错误::工具错误(format!("工具不存在: {}", 工具名)))?;

        工具.回调ID = Some(回调ID);
        Ok(())
    }

    /// 设置工具回调闭包对象指针 (Qi 闭包对象地址)
    pub fn 设置工具回调指针(&mut self, 工具名: &str, 指针: usize) -> MCP结果<()> {
        let 工具 = self
            .工具表
            .get_mut(工具名)
            .ok_or_else(|| MCP错误::工具错误(format!("工具不存在: {}", 工具名)))?;

        工具.回调指针 = Some(指针);
        Ok(())
    }

    /// 设置工具原始 inputSchema (完整 JSON Schema 字符串)
    pub fn 设置工具原始schema(&mut self, 工具名: &str, schema: JsonValue) -> MCP结果<()> {
        let 工具 = self
            .工具表
            .get_mut(工具名)
            .ok_or_else(|| MCP错误::工具错误(format!("工具不存在: {}", 工具名)))?;

        工具.原始输入schema = Some(schema);
        Ok(())
    }

    /// 获取工具
    pub fn 获取工具(&self, 工具名: &str) -> MCP结果<&MCP工具> {
        self.工具表
            .get(工具名)
            .ok_or_else(|| MCP错误::工具错误(format!("工具不存在: {}", 工具名)))
    }

    /// 获取工具（可变）
    pub fn 获取工具_可变(&mut self, 工具名: &str) -> MCP结果<&mut MCP工具> {
        self.工具表
            .get_mut(工具名)
            .ok_or_else(|| MCP错误::工具错误(format!("工具不存在: {}", 工具名)))
    }

    /// 设置资源内容（文本）
    pub fn 设置资源文本内容(&mut self, uri: &str, 内容: String) -> MCP结果<()> {
        let 资源 = self
            .资源表
            .get_mut(uri)
            .ok_or_else(|| MCP错误::资源错误(format!("资源不存在: {}", uri)))?;

        资源.内容 = Some(资源内容::文本(内容));
        Ok(())
    }

    /// 设置资源内容（二进制）
    pub fn 设置资源二进制内容(&mut self, uri: &str, 内容: Vec<u8>) -> MCP结果<()> {
        let 资源 = self
            .资源表
            .get_mut(uri)
            .ok_or_else(|| MCP错误::资源错误(format!("资源不存在: {}", uri)))?;

        资源.内容 = Some(资源内容::二进制(内容));
        Ok(())
    }

    /// 设置资源内容（JSON）
    pub fn 设置资源JSON内容(&mut self, uri: &str, 内容: JsonValue) -> MCP结果<()> {
        let 资源 = self
            .资源表
            .get_mut(uri)
            .ok_or_else(|| MCP错误::资源错误(format!("资源不存在: {}", uri)))?;

        资源.内容 = Some(资源内容::JSON(内容));
        Ok(())
    }

    /// 读取资源内容（文本）
    pub fn 读取资源文本(&self, uri: &str) -> MCP结果<&str> {
        let 资源 = self.获取资源(uri)?;
        资源.读取文本()
    }

    /// 读取资源内容（二进制）
    pub fn 读取资源二进制(&self, uri: &str) -> MCP结果<&[u8]> {
        let 资源 = self.获取资源(uri)?;
        资源.读取二进制()
    }

    /// 读取资源内容（JSON）
    pub fn 读取资源JSON(&self, uri: &str) -> MCP结果<&JsonValue> {
        let 资源 = self.获取资源(uri)?;
        资源.读取JSON()
    }

    /// 启动服务器
    pub fn 启动(&mut self) -> MCP结果<()> {
        if self.运行中 {
            return Err(MCP错误::服务器错误("服务器已在运行".to_string()));
        }
        self.运行中 = true;
        Ok(())
    }

    /// 停止服务器
    pub fn 停止(&mut self) -> MCP结果<()> {
        if !self.运行中 {
            return Err(MCP错误::服务器错误("服务器未运行".to_string()));
        }
        self.运行中 = false;
        Ok(())
    }

    /// 获取服务器信息
    pub fn 获取服务器信息(&self) -> JsonValue {
        serde_json::json!({
            "name": self.配置.名称,
            "version": self.配置.版本,
            "protocolVersion": self.配置.协议版本,
            "description": self.配置.描述,
            "capabilities": {
                "tools": !self.工具表.is_empty(),
                "resources": !self.资源表.is_empty(),
                "prompts": !self.提示表.is_empty()
            }
        })
    }

    /// 检查是否正在运行
    pub fn 是否运行中(&self) -> bool {
        self.运行中
    }
}

/// MCP服务器模块
#[derive(Debug)]
pub struct MCP服务器模块 {
    /// 默认配置
    默认配置: MCP服务器配置,
}

impl MCP服务器模块 {
    /// 创建新的MCP服务器模块
    pub fn 创建() -> Self {
        Self {
            默认配置: MCP服务器配置::default(),
        }
    }

    /// 创建MCP服务器
    pub fn 创建服务器(&self, 配置: Option<MCP服务器配置>) -> MCP服务器 {
        let 配置 = 配置.unwrap_or_else(|| self.默认配置.clone());
        MCP服务器::创建(配置)
    }

    /// 创建工具
    pub fn 创建工具(&self, 名称: String, 描述: String) -> MCP工具 {
        MCP工具::创建(名称, 描述)
    }

    /// 创建资源
    pub fn 创建资源(
        &self,
        uri: String,
        名称: String,
        描述: String,
        类型: 资源类型,
    ) -> MCP资源 {
        MCP资源::创建(uri, 名称, 描述, 类型)
    }

    /// 创建提示
    pub fn 创建提示(&self, 名称: String, 描述: String, 模板: String) -> MCP提示 {
        MCP提示::创建(名称, 描述, 模板)
    }
}

impl Default for MCP服务器模块 {
    fn default() -> Self {
        Self::创建()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn 测试创建工具() {
        let 参数 = 工具参数::创建(
            "输入".to_string(),
            "string".to_string(),
            "输入文本".to_string(),
            true,
        );

        let 工具 = MCP工具::创建("测试工具".to_string(), "测试描述".to_string()).添加参数(参数);

        assert_eq!(工具.名称, "测试工具");
        assert_eq!(工具.参数列表.len(), 1);
    }

    #[test]
    fn 测试工具Schema() {
        let 参数 = 工具参数::创建(
            "文本".to_string(),
            "string".to_string(),
            "输入文本".to_string(),
            true,
        );

        let 工具 = MCP工具::创建("echo".to_string(), "回显工具".to_string()).添加参数(参数);

        let schema = 工具.转为Schema();
        assert!(schema["name"].as_str().unwrap() == "echo");
        assert!(schema["inputSchema"]["required"].as_array().unwrap().len() == 1);
    }

    #[test]
    fn 测试创建资源() {
        let 资源 = MCP资源::创建(
            "file:///test.txt".to_string(),
            "测试文件".to_string(),
            "测试资源".to_string(),
            资源类型::文本,
        );

        assert_eq!(资源.uri, "file:///test.txt");
        assert_eq!(资源.类型, 资源类型::文本);
    }

    #[test]
    fn 测试创建提示() {
        let 参数 = 提示参数::创建("主题".to_string(), "文章主题".to_string(), true);

        let 提示 = MCP提示::创建(
            "写作助手".to_string(),
            "帮助写作".to_string(),
            "请写一篇关于{主题}的文章".to_string(),
        )
        .添加参数(参数);

        let mut 参数map = HashMap::new();
        参数map.insert("主题".to_string(), "AI".to_string());

        let 结果 = 提示.填充(&参数map).unwrap();
        assert!(结果.contains("AI"));
    }

    #[test]
    fn 测试MCP服务器() {
        let 模块 = MCP服务器模块::创建();
        let mut 服务器 = 模块.创建服务器(None);

        let 工具 = 模块.创建工具("测试".to_string(), "测试工具".to_string());
        服务器.注册工具(工具).unwrap();

        let 工具列表 = 服务器.获取工具列表();
        assert_eq!(工具列表.len(), 1);

        assert!(!服务器.是否运行中());
        服务器.启动().unwrap();
        assert!(服务器.是否运行中());
        服务器.停止().unwrap();
        assert!(!服务器.是否运行中());
    }

    #[test]
    fn 测试服务器信息() {
        let 模块 = MCP服务器模块::创建();
        let 服务器 = 模块.创建服务器(None);

        let 信息 = 服务器.获取服务器信息();
        assert_eq!(信息["name"].as_str().unwrap(), "Qi MCP Server");
        assert_eq!(信息["protocolVersion"].as_str().unwrap(), "2025-06-18");
    }
}
