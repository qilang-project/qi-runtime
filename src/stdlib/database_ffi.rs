//! 数据库模块 FFI
//!
//! 实现与编译器内嵌运行时保持一致，避免生成程序链接到过期数据库符号。

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../qi/src/runtime/stdlib/database_ffi.rs"
));
