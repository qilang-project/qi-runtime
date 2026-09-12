//! Qi 语言 WebAssembly 运行时（wasm32-wasip1）
//!
//! # 一份源码，两个目标
//!
//! 这个 crate 自己几乎不写业务逻辑：`stdlib::*` 下面的模块全部用 `#[path]` 从主运行时
//! `../src/stdlib/*.rs` **原样编进来**。字符串 / JSON / 列表 / 哈希表 / 字节切片 / 数学 /
//! 时间 / 闭包 / 反射 / 正则 / 加密 / 压缩 / 向量 / 词法索引，在 wasm 里跑的就是
//! 原生那份代码，行为逐字节一致，改一处两边都改。
//!
//! 能这么做的前提是那些模块只依赖 `crate::stdlib::{qi_str, list, rc_obj}` 和几个纯 Rust
//! crate（serde_json / dashmap / chrono / regex / md5 / sha2 …），它们都能编到
//! wasm32-wasip1。模块树在这里按主运行时的路径**同形**摆放（`crate::stdlib::qi_str`
//! 在两边都解析到同一份源文件），源文件里的 `crate::` 引用才不用改。
//!
//! # 为什么不直接给 qi-runtime 加 feature 编到 wasm
//!
//! 主运行时的**非可选**依赖里有两棵原生 C 库树：rusqlite（bundled sqlite）和
//! reqwest → rustls → aws-lc-sys。它们的 build script 在 wasm32 上直接失败，跟
//! feature 开关无关。要走那条路得把十几个依赖改成 optional、给五十多个 FFI 文件加
//! cfg，还会把主运行时的每次构建都拖慢。`#[path]` 复用是零侵入的同一效果。
//!
//! # 手写的只有跟宿主强相关的几件
//!
//! - `host`：打印（wasi fd_write）、分配、GC 空转、等待组 / 互斥（单线程里都是平凡的）、
//!   goroutine（**就地同步执行**：wasm 没有第二根线程）、定时器、文件（wasi 有文件系统）
//! - `async_runtime::future`：同步版 Future。所有生产者在 wasm 里都是同步的，
//!   所以 Future 一出生就已完成，`等待` 立刻返回
//! - `stdlib::exception_ffi`：wasi libc **没有 setjmp/longjmp**，`尝试/捕获` 的控制转移
//!   做不出来。这里只保留「抛出 = 打印消息并退出」，用到 `尝试` 的程序在链接期报
//!   `setjmp` 未定义 —— 这是已知边界，见 README
//!
//! # ABI 铁律
//!
//! RC 字符串 / 对象的 24 字节隐藏 header 由 `stdlib::qi_str` / `stdlib::rc_obj` 定义，
//! 与主运行时是**同一份源码**，不存在漂移的可能。codegen 发射的字面量常量在 wasm32 上
//! 同形（三个字段都是定宽 64 位）。

#![allow(clippy::missing_safety_doc)]
#![allow(non_snake_case)]
// extern "C" FFI 面按约定收裸指针并解引用（调用方是 qi 生成的代码，指针来源可控），
// 这里几百个函数都是 #[path] 从主运行时原样编进来的，逐个标 unsafe 就是改主运行时的
// 公开签名。与主运行时同一口径，crate 级放开。
#![allow(clippy::not_unsafe_ptr_arg_deref)]

pub mod stdlib {
    //! 与主运行时同形的模块树：`crate::stdlib::X` 两边都指向 ../src/stdlib/X.rs

    // ── 基石：RC 字符串 / RC 对象 / 列表 ─────────────────────────────
    #[path = "../../../src/stdlib/list.rs"]
    pub mod list;
    #[path = "../../../src/stdlib/qi_str.rs"]
    pub mod qi_str;
    #[path = "../../../src/stdlib/qi_str_ffi.rs"]
    pub mod qi_str_ffi;
    #[path = "../../../src/stdlib/rc_obj.rs"]
    pub mod rc_obj;

    // ── 纯计算 ──────────────────────────────────────────────────────
    #[path = "../../../src/stdlib/bytes_ffi.rs"]
    pub mod bytes_ffi;
    #[path = "../../../src/stdlib/closure_ffi.rs"]
    pub mod closure_ffi;
    #[path = "../../../src/stdlib/compress_ffi.rs"]
    pub mod compress_ffi;
    #[path = "../../../src/stdlib/conversion.rs"]
    pub mod conversion;
    #[path = "../../../src/stdlib/crypto.rs"]
    pub mod crypto;
    #[path = "../../../src/stdlib/crypto_ffi.rs"]
    pub mod crypto_ffi;
    #[path = "../../../src/stdlib/datetime.rs"]
    pub mod datetime;
    #[path = "../../../src/stdlib/hashmap.rs"]
    pub mod hashmap;
    #[path = "../../../src/stdlib/json_ffi.rs"]
    pub mod json_ffi;
    #[path = "../../../src/stdlib/lexical_ffi.rs"]
    pub mod lexical_ffi;
    #[path = "../../../src/stdlib/mailbox_ffi.rs"]
    pub mod mailbox_ffi;
    #[path = "../../../src/stdlib/math.rs"]
    pub mod math;
    #[path = "../../../src/stdlib/math_ffi.rs"]
    pub mod math_ffi;
    #[path = "../../../src/stdlib/os_ffi.rs"]
    pub mod os_ffi;
    #[path = "../../../src/stdlib/path_ffi.rs"]
    pub mod path_ffi;
    #[path = "../../../src/stdlib/random_ffi.rs"]
    pub mod random_ffi;
    #[path = "../../../src/stdlib/reflect_ffi.rs"]
    pub mod reflect_ffi;
    #[path = "../../../src/stdlib/regex_ffi.rs"]
    pub mod regex_ffi;
    #[path = "../../../src/stdlib/string.rs"]
    pub mod string;
    #[path = "../../../src/stdlib/string_ffi.rs"]
    pub mod string_ffi;
    #[path = "../../../src/stdlib/sync_ffi.rs"]
    pub mod sync_ffi;
    #[path = "../../../src/stdlib/system.rs"]
    pub mod system;
    #[path = "../../../src/stdlib/vector.rs"]
    pub mod vector;
    #[path = "../../../src/stdlib/vector_ffi.rs"]
    pub mod vector_ffi;

    // ── wasm 特有实现（主运行时那份依赖 setjmp / tokio）────────────────
    pub mod exception_ffi;

    /// 主运行时 `stdlib/mod.rs` 里的错误与结果类型，被 string.rs / crypto.rs 用到；
    /// 变体与主运行时逐个一致（那边是 thiserror 派生，这边照抄）。
    pub type StdlibResult<T> = Result<T, StdlibError>;

    #[derive(Debug, thiserror::Error)]
    pub enum StdlibError {
        #[error("字符串操作错误: {operation} - {message}")]
        StringOperationError { operation: String, message: String },
        #[error("数学运算错误: {operation} - {message}")]
        MathError { operation: String, message: String },
        #[error("系统调用错误: {system_call} - {message}")]
        SystemError {
            system_call: String,
            message: String,
        },
        #[error("类型转换错误: {from_type} -> {to_type} - {message}")]
        ConversionError {
            from_type: String,
            to_type: String,
            message: String,
        },
        #[error("加密操作错误: {operation} - {message}")]
        CryptoError { operation: String, message: String },
        #[error("无效参数: {parameter} - {message}")]
        InvalidParameter { parameter: String, message: String },
        #[error("索引越界: 索引 {index}，长度 {length}")]
        IndexOutOfBounds { index: usize, length: usize },
        #[error("除零错误")]
        DivisionByZero,
        #[error("数值溢出: {operation}")]
        NumericOverflow { operation: String },
    }

    /// 主运行时 `stdlib/mod.rs` 里的通用值类型；个别被 #[path] 进来的模块在签名里用到它。
    /// 与主运行时定义逐字段一致。
    #[derive(Debug, Clone, PartialEq)]
    pub enum StdlibValue {
        Null,
        Boolean(bool),
        Integer(i64),
        Float(f64),
        String(String),
        Array(Vec<StdlibValue>),
        Object(std::collections::HashMap<String, StdlibValue>),
    }
}

pub mod async_runtime {
    //! 主运行时这里是 tokio + 工作线程；wasm 单线程，只保留 ABI 形状
    #[path = "../../../src/async_runtime/coro.rs"]
    pub mod coro;
    pub mod ffi;
    pub mod future;
}

#[path = "../../src/tool_control.rs"]
pub mod tool_control;

// 主运行时的错误类型整个搬过来（只依赖 std）：conversion / math / system 用
// `crate::RuntimeError::{conversion, internal, system}` 这几个构造器
#[path = "../../src/error/mod.rs"]
pub mod error;
pub type RuntimeError = error::Error;

// error/mod.rs 里有 `From<crate::memory::MemoryError>` / `From<crate::io::IoError>`，
// 只用了 `{:?}`。wasm 里没有那两个子系统，给两个同名的最小类型让 impl 成立。
pub mod memory {
    #[derive(Debug)]
    pub enum MemoryError {
        OutOfMemory { size: usize },
    }
}
pub mod io {
    #[derive(Debug)]
    pub enum IoError {
        FileOperationFailed { path: String, message: String },
    }
}
pub type RuntimeResult<T> = Result<T, RuntimeError>;

pub mod host;

// wasm 上的 HTTP：不自己实现，声明宿主导入让 JS 那边用 fetch/XHR 去发。
// 只有真用到 HTTP 的程序才会带上这个导入（wasm-ld 只链引用到的）。
pub mod http_host;
