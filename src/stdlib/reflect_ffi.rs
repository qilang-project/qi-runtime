//! 运行时反射注册表 FFI —— 让运行中的 Qi 程序（尤其内置 AI Agent）能自省
//! 「当前系统有哪些中文函数、结构体、枚举」。
//!
//! ## 数据来源
//! 编译期 codegen 在 `main` 序言里为每个用户函数 / 结构体 / 枚举各生成一条
//! `qi_reflect_register_*(名, 描述)` 调用（字符串是 immortal 全局常量）。这些调用
//! 在用户 `入口()` 体运行之前把元数据灌进本模块的进程级注册表。之后 Qi 代码
//! 通过 `反射.*` 查询。
//!
//! ## 为什么用「注册」而不是「静态表符号」
//! 静态表要求运行时与生成代码约定精确的内存布局（易随版本漂移）。注册式只依赖
//! 稳定的 C ABI 函数签名，codegen 只管把中文原名 + 可读签名文本喂进来，
//! 运行时全权保管，解耦干净。
//!
//! ## ARC / 内存
//! 注册的名字/描述来自 immortal 全局常量（rc=∞，永不释放），本模块 `to_string`
//! 各存一份自有 String，不持有生成代码的指针。查询返回的字符串是**新分配**的
//! Qi RC 堆串（rc=1，qi_string_from_cstr / rc_cstr_from_str），调用方按正常 ARC 释放。
//! 列表查询返回 Qi `数组<字符串>`（qi_obj_alloc 本体 rc=1 + 每元素 rc=1 串）。

use crate::stdlib::qi_str::rc_cstr_from_str;
use crate::stdlib::rc_obj::qi_obj_alloc;
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::Mutex;

/// 一条元数据：中文原名 + 可读描述（函数签名文本 / 字段列表 / 变体列表）。
#[derive(Clone)]
struct 反射项 {
    名: String,
    描述: String,
}

#[derive(Default)]
struct 反射注册表 {
    函数: Vec<反射项>,
    结构体: Vec<反射项>,
    枚举: Vec<反射项>,
}

// 进程级唯一注册表。global_ctors / main 序言在单线程启动阶段写入；
// 之后多协程只读查询。Mutex 足够（写在前、读在后，无热路径争用）。
static 注册表: Mutex<反射注册表> = Mutex::new(反射注册表 {
    函数: Vec::new(),
    结构体: Vec::new(),
    枚举: Vec::new(),
});

fn 读c字符串(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// 幂等去重登记：同名（+同类）只记一次。热重载 / 重复编译单元不会灌重。
fn 登记(表: &mut Vec<反射项>, 名: String, 描述: String) {
    if 表.iter().any(|it| it.名 == 名) {
        return;
    }
    表.push(反射项 { 名, 描述 });
}

// ============================================================================
// 注册 API（codegen 在 main 序言里调用）
// ============================================================================

/// 登记一个用户函数：名 + 签名文本（如 "整数, 字符串 -> 布尔"）。
#[no_mangle]
pub extern "C" fn qi_reflect_register_function(name: *const c_char, signature: *const c_char) {
    if let (Some(n), Some(s)) = (读c字符串(name), 读c字符串(signature)) {
        if let Ok(mut r) = 注册表.lock() {
            登记(&mut r.函数, n, s);
        }
    }
}

/// 登记一个结构体：名 + 字段描述（如 "名字:字符串, 年龄:整数"）。
#[no_mangle]
pub extern "C" fn qi_reflect_register_struct(name: *const c_char, fields: *const c_char) {
    if let (Some(n), Some(f)) = (读c字符串(name), 读c字符串(fields)) {
        if let Ok(mut r) = 注册表.lock() {
            登记(&mut r.结构体, n, f);
        }
    }
}

/// 登记一个枚举：名 + 变体描述（如 "红, 绿, 蓝"）。
#[no_mangle]
pub extern "C" fn qi_reflect_register_enum(name: *const c_char, variants: *const c_char) {
    if let (Some(n), Some(v)) = (读c字符串(name), 读c字符串(variants)) {
        if let Ok(mut r) = 注册表.lock() {
            登记(&mut r.枚举, n, v);
        }
    }
}

// ============================================================================
// 查询 API（Qi 代码经 反射.* 调用）
// ============================================================================

/// 用一组名字构建一个 Qi `数组<字符串>`：
/// 堆布局 `[len@0, str0@1, str1@2, ...]`，本体经 qi_obj_alloc（rc=1），
/// 每个元素是新分配的 Qi RC 串（rc=1）。与 codegen 数组表示一致。
fn 构建字符串数组(项: &[String]) -> *mut u8 {
    let n = 项.len();
    let base = qi_obj_alloc(((n + 1) * 8) as i64);
    if base.is_null() {
        return base;
    }
    unsafe {
        let 槽 = base as *mut i64;
        *槽 = n as i64; // 长度头
        for (i, s) in 项.iter().enumerate() {
            let sp = rc_cstr_from_str(s);
            *槽.add(i + 1) = sp as i64;
        }
    }
    base
}

fn 名字们(表: &[反射项]) -> Vec<String> {
    表.iter().map(|it| it.名.clone()).collect()
}

/// 所有用户函数名 —— 返回 `数组<字符串>`。AI Agent 自省「有哪些工具」的入口。
#[no_mangle]
pub extern "C" fn qi_reflect_function_list() -> *mut u8 {
    let names = 注册表.lock().map(|r| 名字们(&r.函数)).unwrap_or_default();
    构建字符串数组(&names)
}

/// 所有结构体名 —— 返回 `数组<字符串>`。
#[no_mangle]
pub extern "C" fn qi_reflect_struct_list() -> *mut u8 {
    let names = 注册表.lock().map(|r| 名字们(&r.结构体)).unwrap_or_default();
    构建字符串数组(&names)
}

/// 所有枚举名 —— 返回 `数组<字符串>`。
#[no_mangle]
pub extern "C" fn qi_reflect_enum_list() -> *mut u8 {
    let names = 注册表.lock().map(|r| 名字们(&r.枚举)).unwrap_or_default();
    构建字符串数组(&names)
}

/// 某函数的签名文本（"参数类型... -> 返回类型"）；无此函数返回空串。
#[no_mangle]
pub extern "C" fn qi_reflect_function_signature(name: *const c_char) -> *mut c_char {
    let 名 = match 读c字符串(name) {
        Some(n) => n,
        None => return rc_cstr_from_str(""),
    };
    let 描述 = 注册表
        .lock()
        .ok()
        .and_then(|r| {
            r.函数
                .iter()
                .find(|it| it.名 == 名)
                .map(|it| it.描述.clone())
        })
        .unwrap_or_default();
    rc_cstr_from_str(&描述)
}

/// 某结构体的字段描述（"字段:类型, ..."）；无此结构体返回空串。
#[no_mangle]
pub extern "C" fn qi_reflect_struct_fields(name: *const c_char) -> *mut c_char {
    let 名 = match 读c字符串(name) {
        Some(n) => n,
        None => return rc_cstr_from_str(""),
    };
    let 描述 = 注册表
        .lock()
        .ok()
        .and_then(|r| {
            r.结构体
                .iter()
                .find(|it| it.名 == 名)
                .map(|it| it.描述.clone())
        })
        .unwrap_or_default();
    rc_cstr_from_str(&描述)
}

/// 是否存在某用户函数（1 = 是，0 = 否）。
#[no_mangle]
pub extern "C" fn qi_reflect_has_function(name: *const c_char) -> i64 {
    let 名 = match 读c字符串(name) {
        Some(n) => n,
        None => return 0,
    };
    let 有 = 注册表
        .lock()
        .map(|r| r.函数.iter().any(|it| it.名 == 名))
        .unwrap_or(false);
    if 有 {
        1
    } else {
        0
    }
}

/// 用户函数个数（配合 qi_reflect_function_name 做索引遍历）。
#[no_mangle]
pub extern "C" fn qi_reflect_function_count() -> i64 {
    注册表.lock().map(|r| r.函数.len() as i64).unwrap_or(0)
}

/// 第 i 个用户函数名（越界返回空串）。
#[no_mangle]
pub extern "C" fn qi_reflect_function_name(i: i64) -> *mut c_char {
    let s = 注册表
        .lock()
        .ok()
        .and_then(|r| r.函数.get(i as usize).map(|it| it.名.clone()))
        .unwrap_or_default();
    rc_cstr_from_str(&s)
}

/// 结构体个数。
#[no_mangle]
pub extern "C" fn qi_reflect_struct_count() -> i64 {
    注册表.lock().map(|r| r.结构体.len() as i64).unwrap_or(0)
}

/// 第 i 个结构体名（越界返回空串）。
#[no_mangle]
pub extern "C" fn qi_reflect_struct_name(i: i64) -> *mut c_char {
    let s = 注册表
        .lock()
        .ok()
        .and_then(|r| r.结构体.get(i as usize).map(|it| it.名.clone()))
        .unwrap_or_default();
    rc_cstr_from_str(&s)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn 注册与查询往返() {
        let n = CString::new("处理").unwrap();
        let s = CString::new("整数 -> 整数").unwrap();
        qi_reflect_register_function(n.as_ptr(), s.as_ptr());
        assert_eq!(qi_reflect_has_function(n.as_ptr()), 1);
        let miss = CString::new("不存在").unwrap();
        assert_eq!(qi_reflect_has_function(miss.as_ptr()), 0);

        // 幂等：重复登记不增长
        let c0 = qi_reflect_function_count();
        qi_reflect_register_function(n.as_ptr(), s.as_ptr());
        assert_eq!(qi_reflect_function_count(), c0);

        // 签名往返
        let sig = qi_reflect_function_signature(n.as_ptr());
        let got = unsafe { CStr::from_ptr(sig) }
            .to_string_lossy()
            .into_owned();
        assert_eq!(got, "整数 -> 整数");
    }

    #[test]
    fn 字符串数组布局() {
        let items = vec!["甲".to_string(), "乙".to_string()];
        let arr = 构建字符串数组(&items);
        assert!(!arr.is_null());
        unsafe {
            let 槽 = arr as *const i64;
            assert_eq!(*槽, 2); // 长度头
            let s0 = *槽.add(1) as *const c_char;
            let s1 = *槽.add(2) as *const c_char;
            assert_eq!(CStr::from_ptr(s0).to_string_lossy(), "甲");
            assert_eq!(CStr::from_ptr(s1).to_string_lossy(), "乙");
        }
    }
}
