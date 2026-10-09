//! 托管编辑区 FFI 转发（实现在 qi-gui egui_editor_buf.rs；光标行列在 egui_editor.rs）
//!
//! 正文留在 qi-gui 那边，qi 只在版本变了时取一次文本。返回的字符串照例拷成
//! qi 拥有的 RC 串。没编进 GUI（`has_gui` 未置）时全部吞参数、返回空值。

use std::os::raw::c_char;

#[cfg(has_gui)]
extern "C" {
    fn qi_gui_egui_editor_managed_impl(id: *const c_char, mono: i64, size: i64) -> i64;
    fn qi_gui_egui_editor_set_text_impl(id: *const c_char, text: *const c_char);
    fn qi_gui_egui_editor_text_impl(id: *const c_char) -> *const c_char;
    fn qi_gui_egui_editor_version_impl(id: *const c_char) -> i64;
    fn qi_gui_egui_editor_set_highlight_impl(id: *const c_char, spec: *const c_char);
    fn qi_gui_egui_set_highlight_color_impl(bits: i64, color: i64);
    fn qi_gui_egui_editor_cursor_row_impl() -> i64;
    fn qi_gui_egui_editor_cursor_col_impl() -> i64;
}

/// 托管编辑区(id, 等宽, 字号) → 内容版本号
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_managed(id: *const c_char, mono: i64, size: i64) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_editor_managed_impl(id, mono, size) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, mono, size);
        0
    }
}

/// 设置编辑区文本(id, 文本)
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_set_text(id: *const c_char, text: *const c_char) {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_editor_set_text_impl(id, text)
    }
    #[cfg(not(has_gui))]
    let _ = (id, text);
}

/// 编辑区文本(id) → 正文（Qi 拥有的 RC 串）
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_text(id: *const c_char) -> *mut c_char {
    #[cfg(has_gui)]
    {
        super::qi_str::qi_string_from_cstr(unsafe { qi_gui_egui_editor_text_impl(id) })
    }
    #[cfg(not(has_gui))]
    {
        let _ = id;
        super::qi_str::qi_string_from_cstr(std::ptr::null())
    }
}

/// 编辑区版本(id) → 版本号，没建过的编辑区 0
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_version(id: *const c_char) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_editor_version_impl(id) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = id;
        0
    }
}

/// 设置编辑区高亮(id, "起,止,样式;…")
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_set_highlight(id: *const c_char, spec: *const c_char) {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_editor_set_highlight_impl(id, spec)
    }
    #[cfg(not(has_gui))]
    let _ = (id, spec);
}

/// 设置高亮颜色(样式位, 颜色)
#[no_mangle]
pub extern "C" fn qi_gui_egui_set_highlight_color(bits: i64, color: i64) {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_set_highlight_color_impl(bits, color)
    }
    #[cfg(not(has_gui))]
    let _ = (bits, color);
}

/// 编辑区光标行() → 第几行（按换行分，从 1 起），没有光标 -1
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_cursor_row() -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_editor_cursor_row_impl() }
    }
    #[cfg(not(has_gui))]
    {
        -1
    }
}

/// 编辑区光标列() → 本行第几个字符（从 1 起），没有光标 -1
#[no_mangle]
pub extern "C" fn qi_gui_egui_editor_cursor_col() -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_editor_cursor_col_impl() }
    }
    #[cfg(not(has_gui))]
    {
        -1
    }
}
