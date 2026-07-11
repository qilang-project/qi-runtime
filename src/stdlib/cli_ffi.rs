/// CLI参数解析FFI层
///
/// 基于Rust的clap库实现，为Qi语言提供强大的命令行参数解析功能
use clap::{Arg, ArgAction, ArgMatches, Command};
use once_cell::sync::Lazy;
use std::collections::HashMap;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::ptr;
use std::sync::Mutex;

/// 全局存储，用于管理应用、参数和匹配结果的生命周期
static APPS: Lazy<Mutex<HashMap<usize, QiCliApp>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static ARGS: Lazy<Mutex<HashMap<usize, QiCliArg>>> = Lazy::new(|| Mutex::new(HashMap::new()));
static MATCHES: Lazy<Mutex<HashMap<usize, QiCliMatches>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

static NEXT_ID: Lazy<Mutex<usize>> = Lazy::new(|| Mutex::new(1));

/// 生成唯一ID
fn next_id() -> usize {
    let mut id = NEXT_ID.lock().unwrap();
    let result = *id;
    *id += 1;
    result
}

/// CLI应用结构
pub struct QiCliApp {
    command: Command,
}

/// CLI参数结构
pub struct QiCliArg {
    arg: Arg,
}

/// CLI匹配结果
pub struct QiCliMatches {
    matches: ArgMatches,
}

// ==================== 应用创建与配置 ====================

/// 创建CLI应用
#[no_mangle]
pub extern "C" fn qi_cli_create_app(name: *const c_char) -> i64 {
    if name.is_null() {
        return 0;
    }

    unsafe {
        let name_string = CStr::from_ptr(name).to_string_lossy().into_owned();
        let name_static: &'static str = Box::leak(name_string.into_boxed_str());
        let app = QiCliApp {
            command: Command::new(name_static),
        };

        let id = next_id();
        APPS.lock().unwrap().insert(id, app);
        id as i64
    }
}

/// 设置版本
#[no_mangle]
pub extern "C" fn qi_cli_set_version(app_id: i64, version: *const c_char) -> i64 {
    if version.is_null() || app_id <= 0 {
        return -1;
    }

    unsafe {
        let version_string = CStr::from_ptr(version).to_string_lossy().into_owned();
        let version_static: &'static str = Box::leak(version_string.into_boxed_str());
        let mut apps = APPS.lock().unwrap();

        if let Some(app) = apps.get_mut(&(app_id as usize)) {
            app.command =
                std::mem::replace(&mut app.command, Command::new("")).version(version_static);
            1
        } else {
            -1
        }
    }
}

/// 设置作者
#[no_mangle]
pub extern "C" fn qi_cli_set_author(app_id: i64, author: *const c_char) -> i64 {
    if author.is_null() || app_id <= 0 {
        return -1;
    }

    unsafe {
        let author_string = CStr::from_ptr(author).to_string_lossy().into_owned();
        let author_static: &'static str = Box::leak(author_string.into_boxed_str());
        let mut apps = APPS.lock().unwrap();

        if let Some(app) = apps.get_mut(&(app_id as usize)) {
            app.command =
                std::mem::replace(&mut app.command, Command::new("")).author(author_static);
            1
        } else {
            -1
        }
    }
}

/// 设置关于信息
#[no_mangle]
pub extern "C" fn qi_cli_set_about(app_id: i64, about: *const c_char) -> i64 {
    if about.is_null() || app_id <= 0 {
        return -1;
    }

    unsafe {
        let about_string = CStr::from_ptr(about).to_string_lossy().into_owned();
        let about_static: &'static str = Box::leak(about_string.into_boxed_str());
        let mut apps = APPS.lock().unwrap();

        if let Some(app) = apps.get_mut(&(app_id as usize)) {
            app.command = std::mem::replace(&mut app.command, Command::new("")).about(about_static);
            1
        } else {
            -1
        }
    }
}

/// 设置详细说明
#[no_mangle]
pub extern "C" fn qi_cli_set_long_about(app_id: i64, detail: *const c_char) -> i64 {
    if detail.is_null() || app_id <= 0 {
        return -1;
    }

    unsafe {
        let detail_string = CStr::from_ptr(detail).to_string_lossy().into_owned();
        let detail_static: &'static str = Box::leak(detail_string.into_boxed_str());
        let mut apps = APPS.lock().unwrap();

        if let Some(app) = apps.get_mut(&(app_id as usize)) {
            app.command =
                std::mem::replace(&mut app.command, Command::new("")).long_about(detail_static);
            1
        } else {
            -1
        }
    }
}

/// 设置用法
#[no_mangle]
pub extern "C" fn qi_cli_set_override_usage(app_id: i64, usage: *const c_char) -> i64 {
    if usage.is_null() || app_id <= 0 {
        return -1;
    }

    unsafe {
        let usage_string = CStr::from_ptr(usage).to_string_lossy().into_owned();
        let usage_static: &'static str = Box::leak(usage_string.into_boxed_str());
        let mut apps = APPS.lock().unwrap();

        if let Some(app) = apps.get_mut(&(app_id as usize)) {
            app.command =
                std::mem::replace(&mut app.command, Command::new("")).override_usage(usage_static);
            1
        } else {
            -1
        }
    }
}

/// 设置尾部帮助
#[no_mangle]
pub extern "C" fn qi_cli_set_after_help(app_id: i64, help: *const c_char) -> i64 {
    if help.is_null() || app_id <= 0 {
        return -1;
    }

    unsafe {
        let help_string = CStr::from_ptr(help).to_string_lossy().into_owned();
        let help_static: &'static str = Box::leak(help_string.into_boxed_str());
        let mut apps = APPS.lock().unwrap();

        if let Some(app) = apps.get_mut(&(app_id as usize)) {
            app.command =
                std::mem::replace(&mut app.command, Command::new("")).after_help(help_static);
            1
        } else {
            -1
        }
    }
}

// ==================== 参数创建与配置 ====================

/// 创建参数
#[no_mangle]
pub extern "C" fn qi_cli_create_arg(name: *const c_char) -> i64 {
    if name.is_null() {
        return 0;
    }

    unsafe {
        let name_string = CStr::from_ptr(name).to_string_lossy().into_owned();
        let name_static: &'static str = Box::leak(name_string.into_boxed_str());
        let arg = QiCliArg {
            arg: Arg::new(name_static),
        };

        let id = next_id();
        ARGS.lock().unwrap().insert(id, arg);
        id as i64
    }
}

/// 设置短名称（单字符）
#[no_mangle]
pub extern "C" fn qi_cli_arg_set_short(arg_id: i64, short: *const c_char) -> i64 {
    if short.is_null() || arg_id <= 0 {
        return -1;
    }

    unsafe {
        let short_str = CStr::from_ptr(short).to_string_lossy().to_string();
        if let Some(ch) = short_str.chars().next() {
            let mut args = ARGS.lock().unwrap();
            if let Some(arg_wrapper) = args.get_mut(&(arg_id as usize)) {
                arg_wrapper.arg = std::mem::replace(&mut arg_wrapper.arg, Arg::new("")).short(ch);
                return 1;
            }
        }
        -1
    }
}

/// 设置长名称
#[no_mangle]
pub extern "C" fn qi_cli_arg_set_long(arg_id: i64, long: *const c_char) -> i64 {
    if long.is_null() || arg_id <= 0 {
        return -1;
    }

    unsafe {
        let long_string = CStr::from_ptr(long).to_string_lossy().into_owned();
        let long_static: &'static str = Box::leak(long_string.into_boxed_str());
        let mut args = ARGS.lock().unwrap();

        if let Some(arg_wrapper) = args.get_mut(&(arg_id as usize)) {
            arg_wrapper.arg =
                std::mem::replace(&mut arg_wrapper.arg, Arg::new("")).long(long_static);
            1
        } else {
            -1
        }
    }
}

/// 设置帮助信息
#[no_mangle]
pub extern "C" fn qi_cli_arg_set_help(arg_id: i64, help: *const c_char) -> i64 {
    if help.is_null() || arg_id <= 0 {
        return -1;
    }

    unsafe {
        let help_string = CStr::from_ptr(help).to_string_lossy().into_owned();
        let help_static: &'static str = Box::leak(help_string.into_boxed_str());
        let mut args = ARGS.lock().unwrap();

        if let Some(arg_wrapper) = args.get_mut(&(arg_id as usize)) {
            arg_wrapper.arg =
                std::mem::replace(&mut arg_wrapper.arg, Arg::new("")).help(help_static);
            1
        } else {
            -1
        }
    }
}

/// 设置是否必需
#[no_mangle]
pub extern "C" fn qi_cli_arg_set_required(arg_id: i64, required: i64) -> i64 {
    if arg_id <= 0 {
        return -1;
    }

    let mut args = ARGS.lock().unwrap();

    if let Some(arg_wrapper) = args.get_mut(&(arg_id as usize)) {
        arg_wrapper.arg =
            std::mem::replace(&mut arg_wrapper.arg, Arg::new("")).required(required != 0);
        1
    } else {
        -1
    }
}

/// 设置默认值
#[no_mangle]
pub extern "C" fn qi_cli_arg_set_default(arg_id: i64, default: *const c_char) -> i64 {
    if default.is_null() || arg_id <= 0 {
        return -1;
    }

    unsafe {
        let default_string = CStr::from_ptr(default).to_string_lossy().into_owned();
        let default_static: &'static str = Box::leak(default_string.into_boxed_str());
        let mut args = ARGS.lock().unwrap();

        if let Some(arg_wrapper) = args.get_mut(&(arg_id as usize)) {
            arg_wrapper.arg =
                std::mem::replace(&mut arg_wrapper.arg, Arg::new("")).default_value(default_static);
            1
        } else {
            -1
        }
    }
}

/// 设置为标志（布尔类型，不需要值）
#[no_mangle]
pub extern "C" fn qi_cli_arg_set_flag(arg_id: i64) -> i64 {
    if arg_id <= 0 {
        return -1;
    }

    let mut args = ARGS.lock().unwrap();

    if let Some(arg_wrapper) = args.get_mut(&(arg_id as usize)) {
        arg_wrapper.arg =
            std::mem::replace(&mut arg_wrapper.arg, Arg::new("")).action(ArgAction::SetTrue);
        1
    } else {
        -1
    }
}

/// 设置多值（可接受多个值）
#[no_mangle]
pub extern "C" fn qi_cli_arg_set_multiple(arg_id: i64) -> i64 {
    if arg_id <= 0 {
        return -1;
    }

    let mut args = ARGS.lock().unwrap();

    if let Some(arg_wrapper) = args.get_mut(&(arg_id as usize)) {
        arg_wrapper.arg = std::mem::replace(&mut arg_wrapper.arg, Arg::new("")).num_args(1..);
        1
    } else {
        -1
    }
}

/// 设置从环境变量读取（暂不支持，clap 4.x需要启用env feature）
#[no_mangle]
pub extern "C" fn qi_cli_arg_set_env(_arg_id: i64, _env_var: *const c_char) -> i64 {
    // 当前clap版本不支持.env()方法，需要手动从环境变量读取
    // 返回1表示"接受"，实际不做处理
    1
}

/// 设置为全局参数（被所有子命令继承，可在任意层级位置出现）
#[no_mangle]
pub extern "C" fn qi_cli_arg_set_global(arg_id: i64) -> i64 {
    if arg_id <= 0 {
        return -1;
    }

    let mut args = ARGS.lock().unwrap();

    if let Some(arg_wrapper) = args.get_mut(&(arg_id as usize)) {
        arg_wrapper.arg = std::mem::replace(&mut arg_wrapper.arg, Arg::new("")).global(true);
        1
    } else {
        -1
    }
}

// ==================== 应用参数添加 ====================

/// 添加参数到应用
#[no_mangle]
pub extern "C" fn qi_cli_app_add_arg(app_id: i64, arg_id: i64) -> i64 {
    if app_id <= 0 || arg_id <= 0 {
        return -1;
    }

    let args = ARGS.lock().unwrap();
    let mut apps = APPS.lock().unwrap();

    if let (Some(app), Some(arg_wrapper)) = (
        apps.get_mut(&(app_id as usize)),
        args.get(&(arg_id as usize)),
    ) {
        app.command =
            std::mem::replace(&mut app.command, Command::new("")).arg(arg_wrapper.arg.clone());
        1
    } else {
        -1
    }
}

// ==================== 子命令支持 ====================

/// 创建子命令
#[no_mangle]
pub extern "C" fn qi_cli_create_subcommand(name: *const c_char) -> i64 {
    if name.is_null() {
        return 0;
    }

    unsafe {
        let name_string = CStr::from_ptr(name).to_string_lossy().into_owned();
        let name_static: &'static str = Box::leak(name_string.into_boxed_str());
        let app = QiCliApp {
            command: Command::new(name_static),
        };

        let id = next_id();
        APPS.lock().unwrap().insert(id, app);
        id as i64
    }
}

/// 添加子命令到应用
#[no_mangle]
pub extern "C" fn qi_cli_app_add_subcommand(app_id: i64, subcommand_id: i64) -> i64 {
    if app_id <= 0 || subcommand_id <= 0 {
        return -1;
    }

    let mut apps = APPS.lock().unwrap();

    // 获取子命令并克隆
    let subcommand = if let Some(sub_app) = apps.get(&(subcommand_id as usize)) {
        sub_app.command.clone()
    } else {
        return -1;
    };

    // 添加到主应用
    if let Some(app) = apps.get_mut(&(app_id as usize)) {
        app.command = std::mem::replace(&mut app.command, Command::new("")).subcommand(subcommand);
        1
    } else {
        -1
    }
}

/// 添加命令别名
#[no_mangle]
pub extern "C" fn qi_cli_app_add_alias(app_id: i64, alias: *const c_char) -> i64 {
    if alias.is_null() || app_id <= 0 {
        return -1;
    }

    unsafe {
        let alias_string = CStr::from_ptr(alias).to_string_lossy().into_owned();
        let alias_static: &'static str = Box::leak(alias_string.into_boxed_str());
        let mut apps = APPS.lock().unwrap();

        if let Some(app) = apps.get_mut(&(app_id as usize)) {
            app.command = std::mem::replace(&mut app.command, Command::new("")).alias(alias_static);
            1
        } else {
            -1
        }
    }
}

/// 显示帮助
#[no_mangle]
pub extern "C" fn qi_cli_print_help(app_id: i64) -> i64 {
    if app_id <= 0 {
        return -1;
    }

    let mut apps = APPS.lock().unwrap();
    if let Some(app) = apps.get_mut(&(app_id as usize)) {
        match qi_cli_localize(app.command.clone()).print_help() {
            Ok(_) => {
                println!();
                1
            }
            Err(_) => -1,
        }
    } else {
        -1
    }
}

// ==================== 帮助信息本地化 ====================
//
// clap 默认输出英文的段落标题（Usage/Commands/Options/Arguments）、自动
// 生成的 -h/--help、-V/--version 说明，以及内置的 help 子命令说明。这里
// 在解析/打印帮助前对整棵命令树做一次本地化，让所有 Qi CLI 程序的帮助
// 都显示中文。
//
// 说明：clap 的用法行占位符 `[OPTIONS]` 和默认值注解 `[default: ...]`
// 是 clap 内部硬编码的英文，无法通过公开 API 翻译，保持原样。

/// 根据命令实际拥有的段落动态生成中文帮助模板，避免出现空标题。
fn qi_cli_help_template(cmd: &Command) -> String {
    let has_sub = cmd.get_subcommands().next().is_some();
    let has_pos = cmd.get_positionals().next().is_some();
    let mut t = String::from("{about-with-newline}\n用法：{usage}\n");
    if has_sub {
        t.push_str("\n命令：\n{subcommands}\n");
    }
    if has_pos {
        t.push_str("\n参数：\n{positionals}\n");
    }
    t.push_str("\n选项：\n{options}{after-help}");
    t
}

/// 递归地把一棵命令树的帮助信息本地化为中文。
fn qi_cli_localize(mut cmd: Command) -> Command {
    // 用中文说明替换自动生成的 -h/--help
    let has_help_arg = cmd.get_arguments().any(|a| a.get_id() == "help");
    if !has_help_arg {
        cmd = cmd.disable_help_flag(true).arg(
            Arg::new("help")
                .long("help")
                .short('h')
                .help("打印帮助信息")
                .action(ArgAction::Help),
        );
    }
    // 仅在设置了版本时才有 -V/--version
    if cmd.get_version().is_some() {
        let has_version_arg = cmd.get_arguments().any(|a| a.get_id() == "version");
        if !has_version_arg {
            cmd = cmd.disable_version_flag(true).arg(
                Arg::new("version")
                    .long("version")
                    .short('V')
                    .help("打印版本信息")
                    .action(ArgAction::Version),
            );
        }
    }
    // 先本地化已有子命令
    let names: Vec<String> = cmd
        .get_subcommands()
        .map(|s| s.get_name().to_string())
        .collect();
    for name in names {
        cmd = cmd.mut_subcommand(&name, qi_cli_localize);
    }
    // 用中文 help 子命令替换 clap 的英文自动 help 子命令
    // （派发逻辑在 qi_cli_parse 中重新实现）
    if cmd.get_subcommands().next().is_some() && cmd.find_subcommand("help").is_none() {
        let help_sub = qi_cli_localize(
            Command::new("help")
                .about("打印本消息或指定子命令的帮助信息")
                .arg(Arg::new("子命令").help("要查看帮助的子命令").num_args(0..)),
        );
        cmd = cmd
            .disable_help_subcommand(true)
            .subcommand_value_name("命令")
            .subcommand(help_sub);
    }
    let tmpl = qi_cli_help_template(&cmd);
    cmd.help_template(tmpl)
}

/// 重新实现 `<程序> help [子命令...]` 的派发：沿命令链走到目标命令并打印其帮助。
fn qi_cli_dispatch_help(root: &mut Command, sub_matches: &ArgMatches) {
    let path: Vec<String> = sub_matches
        .get_many::<String>("子命令")
        .map(|values| values.map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let mut current: &mut Command = root;
    for name in &path {
        match current.find_subcommand_mut(name) {
            Some(next) => current = next,
            None => {
                eprintln!("未找到子命令：{}", name);
                return;
            }
        }
    }
    let _ = current.print_help();
    println!();
}

// ==================== 参数解析 ====================

/// 解析命令行参数
#[no_mangle]
pub extern "C" fn qi_cli_parse(app_id: i64) -> i64 {
    if app_id <= 0 {
        return 0;
    }

    let mut apps = APPS.lock().unwrap();

    if let Some(app) = apps.get_mut(&(app_id as usize)) {
        // 获取命令行参数
        let args: Vec<String> = std::env::args().collect();

        // 本地化整棵命令树，让帮助输出为中文
        let mut command = qi_cli_localize(app.command.clone());

        // 解析
        match command.clone().try_get_matches_from(args) {
            Ok(matches) => {
                // 处理中文 help 子命令：打印目标命令的帮助后退出
                if let Some(("help", sub_matches)) = matches.subcommand() {
                    qi_cli_dispatch_help(&mut command, sub_matches);
                    std::process::exit(0);
                }
                let matches_wrapper = QiCliMatches { matches };
                let id = next_id();
                MATCHES.lock().unwrap().insert(id, matches_wrapper);
                id as i64
            }
            Err(e) => {
                // 打印错误并退出（clap会自动显示帮助）
                eprintln!("{}", e);
                0
            }
        }
    } else {
        0
    }
}

// ==================== 结果获取 ====================

/// 获取字符串值
#[no_mangle]
pub extern "C" fn qi_cli_get_value(matches_id: i64, name: *const c_char) -> *mut c_char {
    if matches_id <= 0 || name.is_null() {
        return ptr::null_mut();
    }

    unsafe {
        let name_str = CStr::from_ptr(name).to_string_lossy().to_string();
        let matches = MATCHES.lock().unwrap();

        if let Some(m) = matches.get(&(matches_id as usize)) {
            if let Some(value) = m.matches.get_one::<String>(&name_str) {
                // 转换为 RC C 字符串
                return crate::stdlib::qi_str::rc_cstr_from_str(value.as_str());
            }
        }

        ptr::null_mut()
    }
}

/// 获取布尔标志
#[no_mangle]
pub extern "C" fn qi_cli_get_flag(matches_id: i64, name: *const c_char) -> i64 {
    if matches_id <= 0 || name.is_null() {
        return 0;
    }

    unsafe {
        let name_str = CStr::from_ptr(name).to_string_lossy().to_string();
        let matches = MATCHES.lock().unwrap();

        if let Some(m) = matches.get(&(matches_id as usize)) {
            if m.matches.get_flag(&name_str) {
                return 1;
            }
        }

        0
    }
}

/// 检查是否有某个值
#[no_mangle]
pub extern "C" fn qi_cli_has_value(matches_id: i64, name: *const c_char) -> i64 {
    if matches_id <= 0 || name.is_null() {
        return 0;
    }

    unsafe {
        let name_str = CStr::from_ptr(name).to_string_lossy().to_string();
        let matches = MATCHES.lock().unwrap();

        if let Some(m) = matches.get(&(matches_id as usize)) {
            if m.matches.contains_id(&name_str) {
                return 1;
            }
        }

        0
    }
}

/// 检查是否包含子命令
#[no_mangle]
pub extern "C" fn qi_cli_has_subcommand(matches_id: i64, name: *const c_char) -> i64 {
    if matches_id <= 0 || name.is_null() {
        return 0;
    }

    unsafe {
        let name_str = CStr::from_ptr(name).to_string_lossy().to_string();
        let matches = MATCHES.lock().unwrap();

        if let Some(m) = matches.get(&(matches_id as usize)) {
            if m.matches.subcommand_matches(&name_str).is_some() {
                return 1;
            }
        }

        0
    }
}

/// 获取子命令的匹配结果
#[no_mangle]
pub extern "C" fn qi_cli_get_subcommand(matches_id: i64, name: *const c_char) -> i64 {
    if matches_id <= 0 || name.is_null() {
        return 0;
    }

    unsafe {
        let name_str = CStr::from_ptr(name).to_string_lossy().to_string();
        let matches = MATCHES.lock().unwrap();

        if let Some(m) = matches.get(&(matches_id as usize)) {
            if let Some(sub_matches) = m.matches.subcommand_matches(&name_str) {
                let sub_wrapper = QiCliMatches {
                    matches: sub_matches.clone(),
                };
                let id = next_id();
                drop(matches); // 释放锁
                MATCHES.lock().unwrap().insert(id, sub_wrapper);
                return id as i64;
            }
        }

        0
    }
}

// ==================== 内存管理 ====================

/// 释放字符串内存（委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃）
#[no_mangle]
pub extern "C" fn qi_cli_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

/// 释放应用
#[no_mangle]
pub extern "C" fn qi_cli_free_app(app_id: i64) -> i64 {
    if app_id <= 0 {
        return -1;
    }

    let mut apps = APPS.lock().unwrap();
    if apps.remove(&(app_id as usize)).is_some() {
        1
    } else {
        -1
    }
}

/// 释放参数
#[no_mangle]
pub extern "C" fn qi_cli_free_arg(arg_id: i64) -> i64 {
    if arg_id <= 0 {
        return -1;
    }

    let mut args = ARGS.lock().unwrap();
    if args.remove(&(arg_id as usize)).is_some() {
        1
    } else {
        -1
    }
}

/// 释放匹配结果
#[no_mangle]
pub extern "C" fn qi_cli_free_matches(matches_id: i64) -> i64 {
    if matches_id <= 0 {
        return -1;
    }

    let mut matches = MATCHES.lock().unwrap();
    if matches.remove(&(matches_id as usize)).is_some() {
        1
    } else {
        -1
    }
}
