//! qi-runtime build —— 编译 async runtime 的 C syscall 库。
//! 纯 C + Rust，无 LLVM、无系统 TLS（reqwest 用 rustls），可被 zig 交叉编译。

fn main() {
    // has_gui：随 `gui` feature 开启（qi-gui 被编入本 crate → GUI FFI 走真实现）。
    // 默认关闭：运行时保持轻量、可 zig 交叉编译。
    println!("cargo:rustc-check-cfg=cfg(has_gui)");
    if std::env::var("CARGO_FEATURE_GUI").is_ok() {
        println!("cargo:rustc-cfg=has_gui");
    }

    println!("cargo:rerun-if-changed=src/async_runtime/c_runtime/syscalls.c");

    cc::Build::new()
        .file("src/async_runtime/c_runtime/syscalls.c")
        .warnings(true)
        .opt_level(2)
        .compile("qi_async_syscalls");
}
