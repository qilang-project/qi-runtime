//! Chinese Language Support Module
//!
//! This module provides comprehensive Chinese language support including
//! error message localization, Chinese keywords, and cultural adaptations.

use std::collections::HashMap;

/// Chinese error messages
#[derive(Debug, Clone)]
pub struct ChineseErrorMessages {
    /// Message mappings
    messages: HashMap<String, String>,
    /// Current language setting
    language: String,
}

impl ChineseErrorMessages {
    /// Create new Chinese error messages
    pub fn new() -> Self {
        let mut messages = HashMap::new();

        // Initialize common error messages
        messages.insert(
            "memory_allocation_failed".to_string(),
            "内存分配失败".to_string(),
        );
        messages.insert(
            "io_operation_failed".to_string(),
            "输入输出操作失败".to_string(),
        );
        messages.insert("network_error".to_string(), "网络错误".to_string());
        messages.insert("system_error".to_string(), "系统错误".to_string());
        messages.insert("validation_error".to_string(), "验证错误".to_string());
        messages.insert("security_error".to_string(), "安全错误".to_string());
        messages.insert(
            "initialization_failed".to_string(),
            "初始化失败".to_string(),
        );
        messages.insert(
            "program_execution_error".to_string(),
            "程序执行错误".to_string(),
        );
        messages.insert("user_error".to_string(), "用户错误".to_string());
        messages.insert("debug_error".to_string(), "调试错误".to_string());
        messages.insert("internal_error".to_string(), "内部错误".to_string());
        messages.insert("conversion_error".to_string(), "转换错误".to_string());
        messages.insert("assertion_error".to_string(), "断言错误".to_string());

        // Runtime specific messages
        messages.insert(
            "runtime_not_initialized".to_string(),
            "运行时环境未初始化".to_string(),
        );
        messages.insert("program_not_loaded".to_string(), "程序未加载".to_string());
        messages.insert("stack_overflow".to_string(), "栈溢出".to_string());
        messages.insert("heap_overflow".to_string(), "堆溢出".to_string());
        messages.insert("out_of_memory".to_string(), "内存不足".to_string());
        messages.insert("division_by_zero".to_string(), "除零错误".to_string());
        messages.insert("index_out_of_bounds".to_string(), "索引越界".to_string());
        messages.insert("null_pointer".to_string(), "空指针错误".to_string());
        messages.insert("type_mismatch".to_string(), "类型不匹配".to_string());
        messages.insert("invalid_operation".to_string(), "无效操作".to_string());
        messages.insert("timeout_error".to_string(), "超时错误".to_string());
        messages.insert("connection_error".to_string(), "连接错误".to_string());
        messages.insert("permission_denied".to_string(), "权限不足".to_string());
        messages.insert("file_not_found".to_string(), "文件未找到".to_string());
        messages.insert("invalid_input".to_string(), "无效输入".to_string());
        messages.insert("buffer_overflow".to_string(), "缓冲区溢出".to_string());

        Self {
            messages,
            language: "zh-CN".to_string(),
        }
    }

    /// Get localized message for error key
    pub fn get_message(&self, key: &str) -> Option<&String> {
        self.messages.get(key)
    }

    /// Add or update a message
    pub fn add_message(&mut self, key: &str, message: &str) {
        self.messages.insert(key.to_string(), message.to_string());
    }

    /// Get all messages
    pub fn get_all_messages(&self) -> &HashMap<String, String> {
        &self.messages
    }

    /// Set language
    pub fn set_language(&mut self, language: &str) {
        self.language = language.to_string();
    }

    /// Get current language
    pub fn language(&self) -> &str {
        &self.language
    }

    /// Format error message with context
    pub fn format_message(&self, key: &str, context: &str) -> String {
        if let Some(message) = self.get_message(key) {
            format!("{}: {}", message, context)
        } else {
            format!("未知错误: {}", context)
        }
    }
}

impl Default for ChineseErrorMessages {
    fn default() -> Self {
        Self::new()
    }
}

/// Chinese keywords for the Qi programming language
#[derive(Debug, Clone)]
pub struct ChineseKeywords {
    /// Keyword mappings
    keywords: HashMap<String, String>,
}

impl ChineseKeywords {
    /// Create new Chinese keywords
    pub fn new() -> Self {
        let mut keywords = HashMap::new();

        // Control flow keywords
        keywords.insert("if".to_string(), "如果".to_string());
        keywords.insert("else".to_string(), "否则".to_string());
        keywords.insert("while".to_string(), "当".to_string());
        keywords.insert("for".to_string(), "循环".to_string());
        keywords.insert("break".to_string(), "中断".to_string());
        keywords.insert("continue".to_string(), "继续".to_string());
        keywords.insert("return".to_string(), "返回".to_string());
        keywords.insert("function".to_string(), "函数".to_string());
        keywords.insert("class".to_string(), "类".to_string());
        keywords.insert("object".to_string(), "对象".to_string());
        keywords.insert("method".to_string(), "方法".to_string());
        keywords.insert("property".to_string(), "属性".to_string());
        keywords.insert("variable".to_string(), "变量".to_string());
        keywords.insert("constant".to_string(), "常量".to_string());
        keywords.insert("true".to_string(), "真".to_string());
        keywords.insert("false".to_string(), "假".to_string());
        keywords.insert("null".to_string(), "空".to_string());
        keywords.insert("undefined".to_string(), "未定义".to_string());
        keywords.insert("import".to_string(), "导入".to_string());
        keywords.insert("export".to_string(), "导出".to_string());
        keywords.insert("module".to_string(), "模块".to_string());
        keywords.insert("package".to_string(), "包".to_string());
        keywords.insert("try".to_string(), "尝试".to_string());
        keywords.insert("catch".to_string(), "捕获".to_string());
        keywords.insert("finally".to_string(), "最终".to_string());
        keywords.insert("throw".to_string(), "抛出".to_string());
        keywords.insert("async".to_string(), "异步".to_string());
        keywords.insert("await".to_string(), "等待".to_string());
        keywords.insert("yield".to_string(), "让出".to_string());
        keywords.insert("let".to_string(), "让".to_string());
        keywords.insert("const".to_string(), "常量".to_string());
        keywords.insert("var".to_string(), "变量".to_string());
        keywords.insert("public".to_string(), "公共".to_string());
        keywords.insert("private".to_string(), "私有".to_string());
        keywords.insert("protected".to_string(), "保护".to_string());
        keywords.insert("static".to_string(), "静态".to_string());
        keywords.insert("final".to_string(), "最终".to_string());
        keywords.insert("abstract".to_string(), "抽象".to_string());
        keywords.insert("interface".to_string(), "接口".to_string());
        keywords.insert("implements".to_string(), "实现".to_string());
        keywords.insert("extends".to_string(), "扩展".to_string());
        keywords.insert("super".to_string(), "超类".to_string());
        keywords.insert("this".to_string(), "此".to_string());
        keywords.insert("self".to_string(), "自身".to_string());
        keywords.insert("new".to_string(), "新建".to_string());
        keywords.insert("delete".to_string(), "删除".to_string());
        keywords.insert("typeof".to_string(), "类型".to_string());
        keywords.insert("instanceof".to_string(), "实例".to_string());
        keywords.insert("in".to_string(), "在".to_string());
        keywords.insert("of".to_string(), "属于".to_string());
        keywords.insert("and".to_string(), "与".to_string());
        keywords.insert("or".to_string(), "或".to_string());
        keywords.insert("not".to_string(), "非".to_string());
        keywords.insert("xor".to_string(), "异或".to_string());

        Self { keywords }
    }

    /// Get Chinese keyword for English keyword
    pub fn get_chinese_keyword(&self, english: &str) -> Option<&String> {
        self.keywords.get(english)
    }

    /// Get English keyword for Chinese keyword
    pub fn get_english_keyword(&self, chinese: &str) -> Option<&String> {
        for (english, chinese_keyword) in &self.keywords {
            if chinese_keyword == chinese {
                return Some(english);
            }
        }
        None
    }

    /// Add or update keyword mapping
    pub fn add_keyword(&mut self, english: &str, chinese: &str) {
        self.keywords
            .insert(english.to_string(), chinese.to_string());
    }

    /// Get all keywords
    pub fn get_all_keywords(&self) -> &HashMap<String, String> {
        &self.keywords
    }

    /// Check if string is a Chinese keyword
    pub fn is_chinese_keyword(&self, text: &str) -> bool {
        self.keywords.values().any(|keyword| keyword == text)
    }

    /// Check if string is an English keyword
    pub fn is_english_keyword(&self, text: &str) -> bool {
        self.keywords.contains_key(text)
    }
}

impl Default for ChineseKeywords {
    fn default() -> Self {
        Self::new()
    }
}

/// Message localizer for different languages
#[derive(Debug, Clone)]
pub struct MessageLocalizer {
    /// Error messages
    error_messages: ChineseErrorMessages,
    /// Keywords
    keywords: ChineseKeywords,
    /// Current locale
    locale: String,
}

impl MessageLocalizer {
    /// Create new message localizer
    pub fn new() -> Self {
        Self {
            error_messages: ChineseErrorMessages::new(),
            keywords: ChineseKeywords::new(),
            locale: "zh-CN".to_string(),
        }
    }

    /// Create message localizer with specific locale
    pub fn with_locale(locale: &str) -> Self {
        Self {
            error_messages: ChineseErrorMessages::new(),
            keywords: ChineseKeywords::new(),
            locale: locale.to_string(),
        }
    }

    /// Localize error message
    pub fn localize_error(&self, error_key: &str, context: &str) -> String {
        if self.locale.starts_with("zh") {
            self.error_messages.format_message(error_key, context)
        } else {
            format!("Error: {}", context)
        }
    }

    /// Localize keyword
    pub fn localize_keyword(&self, keyword: &str) -> String {
        if self.locale.starts_with("zh") {
            self.keywords
                .get_chinese_keyword(keyword)
                .cloned()
                .unwrap_or_else(|| keyword.to_string())
        } else {
            self.keywords
                .get_english_keyword(keyword)
                .cloned()
                .unwrap_or_else(|| keyword.to_string())
        }
    }

    /// Translate code with keywords
    pub fn translate_code(&self, code: &str, to_chinese: bool) -> String {
        let mut result = code.to_string();

        if to_chinese {
            for (english, chinese) in self.keywords.get_all_keywords() {
                result = result.replace(english, chinese);
            }
        } else {
            for (english, chinese) in self.keywords.get_all_keywords() {
                result = result.replace(chinese, english);
            }
        }

        result
    }

    /// Get error messages
    pub fn error_messages(&self) -> &ChineseErrorMessages {
        &self.error_messages
    }

    /// Get keywords
    pub fn keywords(&self) -> &ChineseKeywords {
        &self.keywords
    }

    /// Get current locale
    pub fn locale(&self) -> &str {
        &self.locale
    }

    /// Set locale
    pub fn set_locale(&mut self, locale: &str) {
        self.locale = locale.to_string();
    }

    /// Add custom error message
    pub fn add_error_message(&mut self, key: &str, message: &str) {
        self.error_messages.add_message(key, message);
    }

    /// Add custom keyword
    pub fn add_keyword(&mut self, english: &str, chinese: &str) {
        self.keywords.add_keyword(english, chinese);
    }
}

impl Default for MessageLocalizer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chinese_error_messages() {
        let messages = ChineseErrorMessages::new();

        assert_eq!(
            messages.get_message("memory_allocation_failed"),
            Some(&"内存分配失败".to_string())
        );
        assert_eq!(messages.get_message("nonexistent"), None);

        let formatted = messages.format_message("io_operation_failed", "file.txt");
        assert_eq!(formatted, "输入输出操作失败: file.txt");
    }

    #[test]
    fn test_chinese_keywords() {
        let keywords = ChineseKeywords::new();

        assert_eq!(
            keywords.get_chinese_keyword("if"),
            Some(&"如果".to_string())
        );
        assert_eq!(
            keywords.get_chinese_keyword("while"),
            Some(&"当".to_string())
        );
        assert_eq!(
            keywords.get_english_keyword("如果"),
            Some(&"if".to_string())
        );
        assert_eq!(
            keywords.get_english_keyword("当"),
            Some(&"while".to_string())
        );

        assert!(keywords.is_chinese_keyword("如果"));
        assert!(keywords.is_english_keyword("if"));
        assert!(!keywords.is_chinese_keyword("if"));
        assert!(!keywords.is_english_keyword("如果"));
    }

    #[test]
    fn test_message_localizer() {
        let localizer = MessageLocalizer::with_locale("zh-CN");

        let error_msg = localizer.localize_error("memory_allocation_failed", "heap");
        assert_eq!(error_msg, "内存分配失败: heap");

        let keyword = localizer.localize_keyword("if");
        assert_eq!(keyword, "如果");

        let code = localizer.translate_code("if (condition) { while (true) { break; } }", true);
        assert!(code.contains("如果"));
        assert!(code.contains("当"));
        assert!(code.contains("中断"));
    }

    #[test]
    fn test_english_localization() {
        let localizer = MessageLocalizer::with_locale("en-US");

        let keyword = localizer.localize_keyword("如果");
        assert_eq!(keyword, "if");

        let code = localizer.translate_code("如果 (condition) { 当 (true) { 中断; } }", false);
        assert!(code.contains("if"));
        assert!(code.contains("while"));
        assert!(code.contains("break"));
    }

    #[test]
    fn test_custom_messages_and_keywords() {
        let mut localizer = MessageLocalizer::new();

        localizer.add_error_message("custom_error", "自定义错误");
        localizer.add_keyword("custom", "自定义");

        assert_eq!(
            localizer.error_messages().get_message("custom_error"),
            Some(&"自定义错误".to_string())
        );
        assert_eq!(
            localizer.keywords().get_chinese_keyword("custom"),
            Some(&"自定义".to_string())
        );
    }

    #[test]
    fn test_locale_switching() {
        let mut localizer = MessageLocalizer::new();

        assert_eq!(localizer.locale(), "zh-CN");

        localizer.set_locale("en-US");
        assert_eq!(localizer.locale(), "en-US");

        let zh_keyword = localizer.localize_keyword("如果");
        assert_eq!(zh_keyword, "if");
    }
}
