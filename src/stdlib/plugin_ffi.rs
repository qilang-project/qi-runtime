//! dlopen 插件热加载 FFI —— 运行中的 Qi 进程动态加载一个 Qi 编译的插件
//! （`导出 函数` 的 .so/.dylib），调它导出的函数，可卸载再换新版，**不重启进程**。
//!
//! 面向 AI Agent「prompt/工具变了不重启」的诉求：把每个工具编成插件 .so，
//! 运行时 `插件.加载` → `插件.取函数` → `插件.调用*`；工具升级后重新编出 .so，
//! `插件.卸载` 旧的 + `插件.加载` 新的，同进程内行为即刻切换。
//!
//! ## 句柄表示
//! `加载` 返回 dlopen 句柄的位模式（i64，0 = 失败）；`取函数` 返回 dlsym 得到的
//! 函数指针位模式（i64，0 = 失败）。64 位平台上指针 ↔ i64 无损，句柄是**不透明整数**，
//! 不入 ARC、不解引用 —— 与既有 registry 里「ptr/句柄按整数处理」的约定一致。
//!
//! ## 双运行时与 ARC 跨 .so 安全（关键设计）
//! 主进程与插件各自静态链一份 qi-runtime（`--库 动态` 用 force_load 打包），
//! 于是进程内有两份运行时副本。这**不会**造成「两个 ARC 堆打架」，因为：
//!   1. 两份副本都用系统分配器（qi-runtime 无自定义 global_allocator）→ 共享同一
//!      libc malloc/free；
//!   2. ARC 引用计数是**内嵌在分配头**里的（magic + refcount，见 rc_obj/qi_str），
//!      不是某个中央堆登记表，两份副本 header 布局与 magic 完全一致；
//!   3. `qi_runtime_initialize` 用 `Once` 幂等，且**不**在初始化时启动调度线程
//!      （异步调度器是首次用到才惰性起 tokio），故插件的 global_ctors 自初始化
//!      只建内存/IO 管理器、不留活线程 → `dlclose` 卸载 .so 时没有指向旧 .so
//!      代码的返回地址（这正是「活协程跨 dlopen 迁移」被砍掉的危险点，此处规避）。
//!
//! 更进一步，本模块的**字符串调用边界只走 C 字符串**：入参把主进程 Qi 串当只读
//! `char*` 借给插件（插件内部 qi_string_from_cstr 拷成自己副本的 Qi 串）；返回值是
//! 插件 strdup 出来的 **C 堆串**（libc malloc），主进程 qi_string_from_cstr 拷成
//! 自己副本的 Qi 串后 libc free 掉它。于是**没有任何 Qi RC 对象跨越 .so 边界**——
//! 每个 Qi RC 串都在「分配它的那份副本」里创建与回收，跨边界的只有 libc 管的 C 串。
//! 无 UAF、无双重释放、无跨副本 refcount 竞争。

use crate::stdlib::qi_str::rc_cstr_from_str;
use std::ffi::CStr;
use std::os::raw::{c_char, c_void};

/// dlopen 一个 Qi 插件动态库，返回句柄位模式（i64，0 = 失败）。
///
/// RTLD_NOW：立即解析全部符号（尽早暴露缺符号，别等首次调用才崩）。
/// RTLD_LOCAL：插件符号不进全局作用域 —— 保持两份运行时副本相互隔离，
/// 插件内部调用绑定到它自己那份 runtime，主进程不受污染。
#[no_mangle]
pub extern "C" fn qi_plugin_load(path: *const c_char) -> i64 {
    if path.is_null() {
        return 0;
    }
    let handle = unsafe { libc::dlopen(path, libc::RTLD_NOW | libc::RTLD_LOCAL) };
    handle as i64
}

/// dlsym 取一个导出符号（函数）的地址，返回函数指针位模式（i64，0 = 失败）。
#[no_mangle]
pub extern "C" fn qi_plugin_sym(handle: i64, name: *const c_char) -> i64 {
    if handle == 0 || name.is_null() {
        return 0;
    }
    let sym = unsafe { libc::dlsym(handle as *mut c_void, name) };
    sym as i64
}

/// 调用签名 `(整数) -> 整数` 的插件函数。fnptr=0 时返回 0。
#[no_mangle]
pub extern "C" fn qi_plugin_call_i64(fnptr: i64, arg: i64) -> i64 {
    if fnptr == 0 {
        return 0;
    }
    let f: extern "C" fn(i64) -> i64 = unsafe { std::mem::transmute(fnptr as *const c_void) };
    f(arg)
}

/// 调用签名 `() -> 整数` 的插件函数。fnptr=0 时返回 0。
#[no_mangle]
pub extern "C" fn qi_plugin_call_i64_noarg(fnptr: i64) -> i64 {
    if fnptr == 0 {
        return 0;
    }
    let f: extern "C" fn() -> i64 = unsafe { std::mem::transmute(fnptr as *const c_void) };
    f()
}

/// 调用签名 `(字符串) -> 字符串` 的插件函数。
///
/// 边界只走 C 串（见模块头「ARC 跨 .so 安全」）：
///   - `arg` 是主进程 Qi 串（NUL 结尾的 data 指针），当只读 `char*` 借给插件；
///   - 插件返回它 strdup 出来的 C 堆串（libc malloc，所有权移交本函数）；
///   - 本函数 qi_string_from_cstr 拷成主进程副本的 Qi RC 串（rc=1），随后 free 掉 C 串。
#[no_mangle]
pub extern "C" fn qi_plugin_call_str(fnptr: i64, arg: *const c_char) -> *mut c_char {
    if fnptr == 0 {
        return rc_cstr_from_str("");
    }
    let f: extern "C" fn(*const c_char) -> *mut c_char =
        unsafe { std::mem::transmute(fnptr as *const c_void) };
    let ret = f(arg);
    if ret.is_null() {
        return rc_cstr_from_str("");
    }
    // 拷进主进程 Qi 串（qi_string_from_cstr 只读 ret、不接管其所有权）
    let qi = crate::stdlib::qi_str::qi_string_from_cstr(ret);
    // 回收插件 strdup 出来的 C 堆串（与插件 strdup 同一 libc free）
    unsafe {
        libc::free(ret as *mut c_void);
    }
    qi
}

/// dlclose 卸载插件。返回 0 = 成功，非 0 = 失败。
///
/// 安全前提：卸载前插件不得有活线程 / 未返回的协程栈（本运行时初始化不起线程，
/// 同步插件天然满足）。卸载后该句柄取得的所有函数指针立即失效，勿再调用。
#[no_mangle]
pub extern "C" fn qi_plugin_unload(handle: i64) -> i64 {
    if handle == 0 {
        return 0;
    }
    unsafe { libc::dlclose(handle as *mut c_void) as i64 }
}

/// 最近一次 dl* 操作的错误文本（无错误返回空串）。诊断加载失败用。
#[no_mangle]
pub extern "C" fn qi_plugin_error() -> *mut c_char {
    let e = unsafe { libc::dlerror() };
    if e.is_null() {
        return rc_cstr_from_str("");
    }
    let msg = unsafe { CStr::from_ptr(e) }.to_string_lossy().into_owned();
    rc_cstr_from_str(&msg)
}
