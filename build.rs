//! qi-runtime build —— 编译 async runtime 的 C syscall 库。
//! 纯 C + Rust，无 LLVM、无系统 TLS（reqwest 用 rustls），可被 zig 交叉编译。

fn main() {
    println!("cargo:rerun-if-changed=src/async_runtime/c_runtime/syscalls.c");

    cc::Build::new()
        .file("src/async_runtime/c_runtime/syscalls.c")
        .warnings(true)
        .opt_level(2)
        .compile("qi_async_syscalls");
}
