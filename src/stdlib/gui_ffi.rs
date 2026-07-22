//! GUI FFI bindings for qi-gui library —— 单轨 egui 架构
//!
//! 图形化窗口接口。老 tao 自绘轨（窗口/渲染器/事件回调/定时器/keycode）已移除，
//! 保留：音频（rodio）+ egui 控件层（winit + softbuffer 软件光栅）+ 画布层（图元自绘）。
//!
//! When GUI library is linked (`has_gui`), these forward to `*_impl` in qi-gui.
//! When not linked, stubs swallow args and return defaults so no-gui builds link.

use std::os::raw::c_char;

// When GUI library is available, link to it
#[cfg(has_gui)]
extern "C" {
    fn qi_gui_version_impl() -> *mut c_char;
    fn qi_gui_free_string_impl(s: *mut c_char);

    // Audio functions
    fn qi_gui_audio_load_impl(file_path: *const c_char) -> u64;
    fn qi_gui_audio_play_impl(audio_id: u64);
    fn qi_gui_audio_pause_impl(audio_id: u64);
    fn qi_gui_audio_stop_impl(audio_id: u64);
    fn qi_gui_audio_set_volume_impl(audio_id: u64, volume: f32);
    fn qi_gui_audio_is_playing_impl(audio_id: u64) -> i32;
    fn qi_gui_audio_is_finished_impl(audio_id: u64) -> i32;
    fn qi_gui_audio_free_impl(audio_id: u64);

    // ── egui 控件层 ──────────────────────────────────────────────
    fn qi_gui_egui_app_create_impl(title: *const c_char, width: u32, height: u32) -> u64;
    fn qi_gui_egui_frame_begin_impl(app_id: u64) -> i32;
    fn qi_gui_egui_frame_end_impl(app_id: u64);
    fn qi_gui_egui_app_close_impl(app_id: u64);
    fn qi_gui_egui_label_impl(text: *const c_char);
    fn qi_gui_egui_heading_impl(text: *const c_char);
    fn qi_gui_egui_colored_label_impl(text: *const c_char, r: i64, g: i64, b: i64);
    fn qi_gui_egui_button_impl(text: *const c_char) -> i32;
    fn qi_gui_egui_text_edit_impl(id: *const c_char, value: *const c_char) -> *const c_char;
    fn qi_gui_egui_text_edit_multiline_impl(
        id: *const c_char,
        value: *const c_char,
    ) -> *const c_char;
    fn qi_gui_egui_slider_impl(id: *const c_char, cur: i64, min: i64, max: i64) -> i64;
    fn qi_gui_egui_checkbox_impl(id: *const c_char, text: *const c_char, cur: i32) -> i32;
    fn qi_gui_egui_combo_impl(id: *const c_char, options_csv: *const c_char, cur: i64) -> i64;
    fn qi_gui_egui_separator_impl();
    fn qi_gui_egui_space_impl();
    fn qi_gui_egui_horizontal_begin_impl();
    fn qi_gui_egui_horizontal_end_impl();
    fn qi_gui_egui_group_begin_impl(title: *const c_char);
    fn qi_gui_egui_group_end_impl();
    fn qi_gui_egui_progress_impl(percent: i64);
    fn qi_gui_egui_plot_impl(id: *const c_char, values_csv: *const c_char, width: i64, height: i64);
    fn qi_gui_egui_message_impl(text: *const c_char);
    // ── egui 第二批（滚动/折叠/单选/表格/图片/主题等）──
    fn qi_gui_egui_scroll_begin_impl(id: *const c_char, height: i64);
    fn qi_gui_egui_scroll_end_impl();
    fn qi_gui_egui_collapse_begin_impl(title: *const c_char) -> i32;
    fn qi_gui_egui_collapse_end_impl();
    fn qi_gui_egui_radio_impl(text: *const c_char, selected: i32) -> i32;
    fn qi_gui_egui_selectable_impl(text: *const c_char, selected: i32) -> i32;
    fn qi_gui_egui_drag_value_impl(id: *const c_char, cur: i64) -> i64;
    fn qi_gui_egui_slider_f64_impl(id: *const c_char, cur: f64, min: f64, max: f64) -> f64;
    fn qi_gui_egui_hyperlink_impl(text: *const c_char, url: *const c_char);
    fn qi_gui_egui_label_tip_impl(text: *const c_char, tip: *const c_char);
    fn qi_gui_egui_table_impl(
        id: *const c_char,
        headers_csv: *const c_char,
        rows_data: *const c_char,
    );
    fn qi_gui_egui_bar_chart_impl(id: *const c_char, values_csv: *const c_char, width: i64, height: i64);
    fn qi_gui_egui_image_impl(path: *const c_char, width: i64, height: i64);
    fn qi_gui_egui_set_theme_impl(dark: i32);
    fn qi_gui_egui_set_zoom_impl(percent: i64);
    fn qi_gui_egui_window_title_impl(app_id: u64, title: *const c_char);
    // ── egui 画布层（承接老图元能力）──
    fn qi_gui_egui_canvas_begin_impl(id: *const c_char, width: i64, height: i64);
    fn qi_gui_egui_canvas_end_impl();
    fn qi_gui_egui_canvas_rect_impl(x: i64, y: i64, w: i64, h: i64, r: i64, g: i64, b: i64);
    fn qi_gui_egui_canvas_circle_impl(x: i64, y: i64, radius: i64, r: i64, g: i64, b: i64);
    fn qi_gui_egui_canvas_line_impl(
        x1: i64,
        y1: i64,
        x2: i64,
        y2: i64,
        width: i64,
        r: i64,
        g: i64,
        b: i64,
    );
    fn qi_gui_egui_canvas_text_impl(
        x: i64,
        y: i64,
        text: *const c_char,
        size: i64,
        r: i64,
        g: i64,
        b: i64,
    );
    fn qi_gui_egui_canvas_clicked_impl() -> i64;
    fn qi_gui_egui_canvas_mouse_x_impl() -> i64;
    fn qi_gui_egui_canvas_mouse_y_impl() -> i64;
}

// ============================================================================
// 库工具 —— 版本 / 释放字符串
// ============================================================================

#[no_mangle]
pub extern "C" fn qi_gui_version() -> *mut c_char {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_version_impl() }
    }
    #[cfg(not(has_gui))]
    {
        std::ptr::null_mut()
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_free_string(s: *mut c_char) {
    #[cfg(has_gui)]
    {
        if s.is_null() {
            return;
        }
        unsafe {
            qi_gui_free_string_impl(s);
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = s;
    }
}

// ============================================================================
// 音频 —— Qi ABI 包装
// ============================================================================

#[no_mangle]
pub extern "C" fn qi_gui_audio_load(file_path: *const c_char) -> i64 {
    #[cfg(has_gui)]
    {
        if file_path.is_null() {
            return 0;
        }
        unsafe { qi_gui_audio_load_impl(file_path) as i64 }
    }
    #[cfg(not(has_gui))]
    {
        let _ = file_path;
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_audio_play(audio_id: i64) {
    #[cfg(has_gui)]
    {
        if audio_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_audio_play_impl(audio_id as u64);
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = audio_id;
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_audio_pause(audio_id: i64) {
    #[cfg(has_gui)]
    {
        if audio_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_audio_pause_impl(audio_id as u64);
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = audio_id;
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_audio_stop(audio_id: i64) {
    #[cfg(has_gui)]
    {
        if audio_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_audio_stop_impl(audio_id as u64);
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = audio_id;
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_audio_set_volume(audio_id: i64, volume: f64) {
    #[cfg(has_gui)]
    {
        if audio_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_audio_set_volume_impl(audio_id as u64, volume as f32);
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (audio_id, volume);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_audio_is_playing(audio_id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if audio_id <= 0 {
            return 0;
        }
        unsafe { qi_gui_audio_is_playing_impl(audio_id as u64) as i64 }
    }
    #[cfg(not(has_gui))]
    {
        let _ = audio_id;
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_audio_is_finished(audio_id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if audio_id <= 0 {
            return 0;
        }
        unsafe { qi_gui_audio_is_finished_impl(audio_id as u64) as i64 }
    }
    #[cfg(not(has_gui))]
    {
        let _ = audio_id;
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_audio_free(audio_id: i64) {
    #[cfg(has_gui)]
    {
        if audio_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_audio_free_impl(audio_id as u64);
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = audio_id;
    }
}

// ============================================================================
// egui 控件层 —— Qi ABI 包装（整数一律 i64，字符串 *const c_char）
// ============================================================================

/// 创建 egui 应用窗口，返回句柄（>0 成功，0 失败）
#[no_mangle]
pub extern "C" fn qi_gui_egui_app_create(title: *const c_char, width: i64, height: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if title.is_null() {
            return 0;
        }
        unsafe { qi_gui_egui_app_create_impl(title, width as u32, height as u32) as i64 }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (title, width, height);
        eprintln!("错误: GUI 库未安装。请安装完整版本以使用图形化功能。");
        0
    }
}

/// 帧开始：返回 1=窗口存活，0=已关闭
#[no_mangle]
pub extern "C" fn qi_gui_egui_frame_begin(app_id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if app_id <= 0 {
            return 0;
        }
        unsafe { qi_gui_egui_frame_begin_impl(app_id as u64) as i64 }
    }
    #[cfg(not(has_gui))]
    {
        let _ = app_id;
        0
    }
}

/// 帧结束
#[no_mangle]
pub extern "C" fn qi_gui_egui_frame_end(app_id: i64) {
    #[cfg(has_gui)]
    {
        if app_id <= 0 {
            return;
        }
        unsafe { qi_gui_egui_frame_end_impl(app_id as u64) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = app_id;
    }
}

/// 关闭应用
#[no_mangle]
pub extern "C" fn qi_gui_egui_app_close(app_id: i64) {
    #[cfg(has_gui)]
    {
        if app_id <= 0 {
            return;
        }
        unsafe { qi_gui_egui_app_close_impl(app_id as u64) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = app_id;
    }
}

/// 标签
#[no_mangle]
pub extern "C" fn qi_gui_egui_label(text: *const c_char) {
    #[cfg(has_gui)]
    {
        if !text.is_null() {
            unsafe { qi_gui_egui_label_impl(text) }
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = text;
    }
}

/// 标题文本（大号）
#[no_mangle]
pub extern "C" fn qi_gui_egui_heading(text: *const c_char) {
    #[cfg(has_gui)]
    {
        if !text.is_null() {
            unsafe { qi_gui_egui_heading_impl(text) }
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = text;
    }
}

/// 彩色标签
#[no_mangle]
pub extern "C" fn qi_gui_egui_colored_label(text: *const c_char, r: i64, g: i64, b: i64) {
    #[cfg(has_gui)]
    {
        if !text.is_null() {
            unsafe { qi_gui_egui_colored_label_impl(text, r, g, b) }
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (text, r, g, b);
    }
}

/// 按钮：返回本帧是否被点击（1/0）
#[no_mangle]
pub extern "C" fn qi_gui_egui_button(text: *const c_char) -> i64 {
    #[cfg(has_gui)]
    {
        if text.is_null() {
            return 0;
        }
        unsafe { qi_gui_egui_button_impl(text) as i64 }
    }
    #[cfg(not(has_gui))]
    {
        let _ = text;
        0
    }
}

/// 单行输入框：传入当前值，返回新值
#[no_mangle]
pub extern "C" fn qi_gui_egui_text_edit(id: *const c_char, value: *const c_char) -> *mut c_char {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_text_edit_impl(id, value) as *mut c_char }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, value);
        std::ptr::null_mut()
    }
}

/// 多行输入框
#[no_mangle]
pub extern "C" fn qi_gui_egui_text_edit_multiline(
    id: *const c_char,
    value: *const c_char,
) -> *mut c_char {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_text_edit_multiline_impl(id, value) as *mut c_char }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, value);
        std::ptr::null_mut()
    }
}

/// 整数滑条：返回新值
#[no_mangle]
pub extern "C" fn qi_gui_egui_slider(id: *const c_char, cur: i64, min: i64, max: i64) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_slider_impl(id, cur, min, max) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, min, max);
        cur
    }
}

/// 复选框：返回新的勾选状态（1/0）
#[no_mangle]
pub extern "C" fn qi_gui_egui_checkbox(id: *const c_char, text: *const c_char, cur: i64) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_checkbox_impl(id, text, cur as i32) as i64 }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, text);
        cur
    }
}

/// 下拉选择：options 为 CSV，cur 为当前序号，返回新序号
#[no_mangle]
pub extern "C" fn qi_gui_egui_combo(
    id: *const c_char,
    options_csv: *const c_char,
    cur: i64,
) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_combo_impl(id, options_csv, cur) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, options_csv);
        cur
    }
}

/// 分隔线
#[no_mangle]
pub extern "C" fn qi_gui_egui_separator() {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_separator_impl()
    }
}

/// 空行（纵向间距）
#[no_mangle]
pub extern "C" fn qi_gui_egui_space() {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_space_impl()
    }
}

/// 水平布局开始
#[no_mangle]
pub extern "C" fn qi_gui_egui_horizontal_begin() {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_horizontal_begin_impl()
    }
}

/// 水平布局结束
#[no_mangle]
pub extern "C" fn qi_gui_egui_horizontal_end() {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_horizontal_end_impl()
    }
}

/// 分组开始（带标题边框）
#[no_mangle]
pub extern "C" fn qi_gui_egui_group_begin(title: *const c_char) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_group_begin_impl(title) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = title;
    }
}

/// 分组结束
#[no_mangle]
pub extern "C" fn qi_gui_egui_group_end() {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_group_end_impl()
    }
}

/// 进度条：percent 0..100
#[no_mangle]
pub extern "C" fn qi_gui_egui_progress(percent: i64) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_progress_impl(percent) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = percent;
    }
}

/// 折线图：id 标识，values 为 CSV 数值，宽高（点）
#[no_mangle]
pub extern "C" fn qi_gui_egui_plot(
    id: *const c_char,
    values_csv: *const c_char,
    width: i64,
    height: i64,
) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_plot_impl(id, values_csv, width, height) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, values_csv, width, height);
    }
}

/// 消息弹窗：浮动窗口显示文本（需每帧调用保持显示）
#[no_mangle]
pub extern "C" fn qi_gui_egui_message(text: *const c_char) {
    #[cfg(has_gui)]
    {
        if !text.is_null() {
            unsafe { qi_gui_egui_message_impl(text) }
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = text;
    }
}

// ══════════════ egui 第二批控件 shim ══════════════

/// 滚动区开始(id, 高度pt)
#[no_mangle]
pub extern "C" fn qi_gui_egui_scroll_begin(id: *const c_char, height: i64) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_scroll_begin_impl(id, height) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, height);
    }
}

/// 滚动区结束
#[no_mangle]
pub extern "C" fn qi_gui_egui_scroll_end() {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_scroll_end_impl()
    }
}

/// 折叠区开始(标题) → 1 展开 / 0 收起
#[no_mangle]
pub extern "C" fn qi_gui_egui_collapse_begin(title: *const c_char) -> i32 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_collapse_begin_impl(title) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = title;
        0
    }
}

/// 折叠区结束
#[no_mangle]
pub extern "C" fn qi_gui_egui_collapse_end() {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_collapse_end_impl()
    }
}

/// 单选按钮 → 1 被点击
#[no_mangle]
pub extern "C" fn qi_gui_egui_radio(text: *const c_char, selected: i32) -> i32 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_radio_impl(text, selected) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (text, selected);
        0
    }
}

/// 可选中列表项 → 1 被点击
#[no_mangle]
pub extern "C" fn qi_gui_egui_selectable(text: *const c_char, selected: i32) -> i32 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_selectable_impl(text, selected) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (text, selected);
        0
    }
}

/// 数字输入（拖拽整数框）→ 新值
#[no_mangle]
pub extern "C" fn qi_gui_egui_drag_value(id: *const c_char, cur: i64) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_drag_value_impl(id, cur) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = id;
        cur
    }
}

/// 浮点滑条 → 新值
#[no_mangle]
pub extern "C" fn qi_gui_egui_slider_f64(id: *const c_char, cur: f64, min: f64, max: f64) -> f64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_slider_f64_impl(id, cur, min, max) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, min, max);
        cur
    }
}

/// 超链接（系统浏览器打开）
#[no_mangle]
pub extern "C" fn qi_gui_egui_hyperlink(text: *const c_char, url: *const c_char) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_hyperlink_impl(text, url) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (text, url);
    }
}

/// 带悬浮提示的标签
#[no_mangle]
pub extern "C" fn qi_gui_egui_label_tip(text: *const c_char, tip: *const c_char) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_label_tip_impl(text, tip) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (text, tip);
    }
}

/// 只读表格（表头 CSV + 行数据 \n 分行逗号分列）
#[no_mangle]
pub extern "C" fn qi_gui_egui_table(
    id: *const c_char,
    headers_csv: *const c_char,
    rows_data: *const c_char,
) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_table_impl(id, headers_csv, rows_data) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, headers_csv, rows_data);
    }
}

/// 柱状图
#[no_mangle]
pub extern "C" fn qi_gui_egui_bar_chart(
    id: *const c_char,
    values_csv: *const c_char,
    width: i64,
    height: i64,
) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_bar_chart_impl(id, values_csv, width, height) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, values_csv, width, height);
    }
}

/// 图片显示(路径, 宽, 高)；0=原尺寸/按比例
#[no_mangle]
pub extern "C" fn qi_gui_egui_image(path: *const c_char, width: i64, height: i64) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_image_impl(path, width, height) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (path, width, height);
    }
}

/// 设置主题：1 深色 / 0 浅色
#[no_mangle]
pub extern "C" fn qi_gui_egui_set_theme(dark: i32) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_set_theme_impl(dark) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = dark;
    }
}

/// 界面缩放（百分比 50..300）
#[no_mangle]
pub extern "C" fn qi_gui_egui_set_zoom(percent: i64) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_set_zoom_impl(percent) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = percent;
    }
}

/// 运行中改窗口标题
#[no_mangle]
pub extern "C" fn qi_gui_egui_set_window_title(app_id: u64, title: *const c_char) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_window_title_impl(app_id, title) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (app_id, title);
    }
}

// ══════════════ egui 画布层 shim（承接老 tao 图元能力）══════════════

/// 画布开始(id, 宽, 高)：在当前 Ui 占一块定尺寸自绘区
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_begin(id: *const c_char, width: i64, height: i64) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_begin_impl(id, width, height) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, width, height);
    }
}

/// 画布结束()
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_end() {
    #[cfg(has_gui)]
    unsafe {
        qi_gui_egui_canvas_end_impl()
    }
}

/// 画布矩形(x, y, 宽, 高, r, g, b)
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_rect(x: i64, y: i64, w: i64, h: i64, r: i64, g: i64, b: i64) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_rect_impl(x, y, w, h, r, g, b) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (x, y, w, h, r, g, b);
    }
}

/// 画布圆(x, y, 半径, r, g, b)
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_circle(x: i64, y: i64, radius: i64, r: i64, g: i64, b: i64) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_circle_impl(x, y, radius, r, g, b) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (x, y, radius, r, g, b);
    }
}

/// 画布线(x1, y1, x2, y2, 粗, r, g, b)
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_line(
    x1: i64,
    y1: i64,
    x2: i64,
    y2: i64,
    width: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_line_impl(x1, y1, x2, y2, width, r, g, b) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (x1, y1, x2, y2, width, r, g, b);
    }
}

/// 画布文本(x, y, 文本, 字号, r, g, b)
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_text(
    x: i64,
    y: i64,
    text: *const c_char,
    size: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_text_impl(x, y, text, size, r, g, b) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (x, y, text, size, r, g, b);
    }
}

/// 画布点击() → 1/0
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_clicked() -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_clicked_impl() }
    }
    #[cfg(not(has_gui))]
    {
        0
    }
}

/// 画布鼠标X() → 局部 X（无悬停 -1）
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_mouse_x() -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_mouse_x_impl() }
    }
    #[cfg(not(has_gui))]
    {
        -1
    }
}

/// 画布鼠标Y() → 局部 Y（无悬停 -1）
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_mouse_y() -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_mouse_y_impl() }
    }
    #[cfg(not(has_gui))]
    {
        -1
    }
}

#[cfg(all(test, has_gui))]
mod tests {
    use super::*;
    use std::ffi::CStr;

    #[test]
    fn test_gui_available() {
        unsafe {
            let version = qi_gui_version();
            assert!(!version.is_null());
            let version_str = CStr::from_ptr(version).to_str().unwrap();
            assert!(version_str.contains("qi-gui"));
            qi_gui_free_string(version);
        }
    }
}
