//! GUI FFI bindings for qi-gui library
//!
//! 图形化窗口接口
//!
//! This module provides FFI bindings to the qi-gui library when available.
//! When GUI library is not linked, stub implementations are provided that return errors.

use std::collections::HashMap;
use std::ffi::c_void;
use std::os::raw::c_char;
use std::sync::{Mutex, OnceLock};

/// Event callback function type
/// Parameters: window_id, event_type, param1, param2
type EventCallback = extern "C" fn(u64, i32, i64, i64);

/// Qi 事件处理器：按 window_id 存注册的 Qi **闭包对象**地址。
/// Qi 把函数当值传时会用 qi_closure_create 包成闭包对象：
///   布局 [offset0 = trampoline 函数指针, env...]；
///   trampoline ABI: extern "C" fn(env, 形参...)，调用时 env 传闭包对象自身。
type QiClosureFn = unsafe extern "C-unwind" fn(*const c_void, i64, i64, i64, i64);
static QI_事件处理器: OnceLock<Mutex<HashMap<u64, usize>>> = OnceLock::new();
fn qi_事件处理器表() -> &'static Mutex<HashMap<u64, usize>> {
    QI_事件处理器.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 固定的 C 回调：事件循环调它，它再按 Qi 闭包约定转调注册的处理函数。
/// 事件类型：0=关闭 1=尺寸(w,h) 2=按键(键码,修饰) 3=鼠标键(键,按下1/抬起0)
///           4=光标移动(x,y) 5=滚轮(dx,dy)
extern "C" fn qi_事件蹦床(window_id: u64, event_type: i32, p1: i64, p2: i64) {
    // 先取出对象地址再释放锁，避免 Qi 回调里再注册造成重入死锁
    let obj = {
        let 表 = qi_事件处理器表().lock().unwrap();
        表.get(&window_id).copied()
    };
    if let Some(obj) = obj {
        if obj != 0 {
            unsafe {
                // 闭包对象 offset 0 取出 trampoline，env 传对象自身
                let tramp_raw = *(obj as *const *const c_void);
                let tramp: QiClosureFn = std::mem::transmute(tramp_raw);
                tramp(
                    obj as *const c_void,
                    window_id as i64,
                    event_type as i64,
                    p1,
                    p2,
                );
            }
        }
    }
}

// When GUI library is available, link to it
#[cfg(has_gui)]
extern "C" {
    fn qi_gui_create_window_impl(title: *const c_char, width: u32, height: u32) -> u64;
    fn qi_gui_destroy_window_impl(window_id: u64);
    fn qi_gui_set_title_impl(window_id: u64, title: *const c_char);
    fn qi_gui_get_title_impl(window_id: u64) -> *mut c_char;
    fn qi_gui_show_window_impl(window_id: u64);
    fn qi_gui_hide_window_impl(window_id: u64);
    fn qi_gui_is_visible_impl(window_id: u64) -> i32;
    fn qi_gui_set_event_callback_impl(window_id: u64, callback: EventCallback);
    fn qi_gui_enable_event_printing_impl(window_id: u64);
    fn qi_gui_get_position_x_impl(window_id: u64) -> i64;
    fn qi_gui_get_position_y_impl(window_id: u64) -> i64;
    fn qi_gui_set_position_impl(window_id: u64, x: i32, y: i32);
    fn qi_gui_get_width_impl(window_id: u64) -> i64;
    fn qi_gui_get_height_impl(window_id: u64) -> i64;
    fn qi_gui_set_size_impl(window_id: u64, width: u32, height: u32);
    fn qi_gui_run_impl();
    fn qi_gui_set_timer_impl(interval_ms: u64);
    fn qi_gui_set_fps_impl(fps: u64);
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

    // Renderer functions
    fn qi_gui_renderer_create_impl(window_id: u64) -> u64;
    fn qi_gui_renderer_begin_frame_impl(renderer_id: u64);
    fn qi_gui_renderer_end_frame_impl(renderer_id: u64);
    fn qi_gui_renderer_clear_impl(renderer_id: u64, r: u8, g: u8, b: u8);
    fn qi_gui_renderer_draw_pixel_impl(renderer_id: u64, x: u32, y: u32, r: u8, g: u8, b: u8);
    fn qi_gui_renderer_draw_rect_impl(
        renderer_id: u64,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
        r: u8,
        g: u8,
        b: u8,
    );
    fn qi_gui_renderer_draw_line_impl(
        renderer_id: u64,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        r: u8,
        g: u8,
        b: u8,
    );
    fn qi_gui_renderer_draw_circle_impl(
        renderer_id: u64,
        cx: i32,
        cy: i32,
        radius: u32,
        r: u8,
        g: u8,
        b: u8,
    );
    fn qi_gui_renderer_draw_image_impl(
        renderer_id: u64,
        file_path: *const c_char,
        x: u32,
        y: u32,
    ) -> i32;
    fn qi_gui_renderer_draw_text_impl(
        renderer_id: u64,
        text: *const c_char,
        x: i32,
        y: i32,
        r: u8,
        g: u8,
        b: u8,
    );
    fn qi_gui_renderer_draw_text_scaled_impl(
        renderer_id: u64,
        text: *const c_char,
        x: i32,
        y: i32,
        scale: u32,
        r: u8,
        g: u8,
        b: u8,
    );
    fn qi_gui_renderer_free_impl(renderer_id: u64);
}

#[no_mangle]
pub extern "C" fn qi_gui_create_window(title: *const c_char, width: i64, height: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if title.is_null() {
            return 0;
        }
        unsafe { qi_gui_create_window_impl(title, width as u32, height as u32) as i64 }
    }

    #[cfg(not(has_gui))]
    {
        eprintln!("错误: GUI 库未安装。请安装完整版本以使用图形化功能。");
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_destroy_window(window_id: i64) {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_destroy_window_impl(window_id as u64);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_set_title(window_id: i64, title: *const c_char) {
    #[cfg(has_gui)]
    {
        if window_id <= 0 || title.is_null() {
            return;
        }
        unsafe {
            qi_gui_set_title_impl(window_id as u64, title);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (window_id, title);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_get_title(window_id: i64) -> *mut c_char {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return std::ptr::null_mut();
        }
        unsafe { qi_gui_get_title_impl(window_id as u64) }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
        std::ptr::null_mut()
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_show_window(window_id: i64) {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_show_window_impl(window_id as u64);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_hide_window(window_id: i64) {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_hide_window_impl(window_id as u64);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_is_visible(window_id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return 0;
        }
        unsafe { qi_gui_is_visible_impl(window_id as u64) as i64 }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_set_event_callback(window_id: i64, callback: EventCallback) {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_set_event_callback_impl(window_id as u64, callback);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (window_id, callback);
    }
}

/// Qi 友好版：注册一个 Qi 函数作为窗口事件处理器。
/// handler 是 Qi 顶层函数 函数(整数,整数,整数,整数) 的函数指针。
#[no_mangle]
pub extern "C" fn qi_gui_on_event(window_id: i64, handler: *const c_void) {
    #[cfg(has_gui)]
    {
        if window_id <= 0 || handler.is_null() {
            return;
        }
        qi_事件处理器表()
            .lock()
            .unwrap()
            .insert(window_id as u64, handler as usize);
        unsafe {
            qi_gui_set_event_callback_impl(window_id as u64, qi_事件蹦床);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (window_id, handler);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_enable_event_printing(window_id: i64) {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_enable_event_printing_impl(window_id as u64);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_get_position_x(window_id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return 0;
        }
        unsafe { qi_gui_get_position_x_impl(window_id as u64) }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_get_position_y(window_id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return 0;
        }
        unsafe { qi_gui_get_position_y_impl(window_id as u64) }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_set_position(window_id: i64, x: i64, y: i64) {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_set_position_impl(window_id as u64, x as i32, y as i32);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (window_id, x, y);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_get_width(window_id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return 0;
        }
        unsafe { qi_gui_get_width_impl(window_id as u64) }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_get_height(window_id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return 0;
        }
        unsafe { qi_gui_get_height_impl(window_id as u64) }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
        0
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_set_size(window_id: i64, width: i64, height: i64) {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_set_size_impl(window_id as u64, width as u32, height as u32);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (window_id, width, height);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_run() {
    #[cfg(has_gui)]
    {
        unsafe {
            qi_gui_run_impl();
        }
    }

    #[cfg(not(has_gui))]
    {
        eprintln!("错误: GUI 库未安装。请安装完整版本以使用图形化功能。");
    }
}

/// 设置自动刷新定时器间隔（毫秒）；0=关闭。需在 运行 之前调用。
/// 开启后事件循环每隔该间隔向窗口事件回调投递 event_type=6 的定时器事件。
#[no_mangle]
pub extern "C" fn qi_gui_set_timer(interval_ms: i64) {
    #[cfg(has_gui)]
    {
        unsafe {
            qi_gui_set_timer_impl(interval_ms as u64);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = interval_ms;
        eprintln!("错误: GUI 库未安装。请安装完整版本以使用图形化功能。");
    }
}

/// 设置渲染帧率（FPS，如 60/120）；0=关闭。需在 运行 之前调用。
/// 开启后事件循环按该帧率向窗口事件回调投递 event_type=7 的渲染帧事件
/// （参数1=自启动毫秒，参数2=帧间隔毫秒），用于逐帧动画。
#[no_mangle]
pub extern "C" fn qi_gui_set_fps(fps: i64) {
    #[cfg(has_gui)]
    {
        unsafe {
            qi_gui_set_fps_impl(fps as u64);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = fps;
        eprintln!("错误: GUI 库未安装。请安装完整版本以使用图形化功能。");
    }
}

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

// Audio wrapper functions

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

// Renderer wrapper functions

#[no_mangle]
pub extern "C" fn qi_gui_renderer_create(window_id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if window_id <= 0 {
            return 0;
        }
        unsafe { qi_gui_renderer_create_impl(window_id as u64) as i64 }
    }

    #[cfg(not(has_gui))]
    {
        let _ = window_id;
        0
    }
}

/// 开始一帧（双缓冲）：之后的绘制只写离屏缓冲、不上屏，配合 结束绘制 消除闪烁。
#[no_mangle]
pub extern "C" fn qi_gui_renderer_begin_frame(renderer_id: i64) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_renderer_begin_frame_impl(renderer_id as u64);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = renderer_id;
    }
}

/// 结束一帧（双缓冲）：把整帧一次性上屏并退出批处理模式。
#[no_mangle]
pub extern "C" fn qi_gui_renderer_end_frame(renderer_id: i64) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_renderer_end_frame_impl(renderer_id as u64);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = renderer_id;
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_renderer_clear(renderer_id: i64, r: i64, g: i64, b: i64) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_renderer_clear_impl(renderer_id as u64, r as u8, g as u8, b as u8);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (renderer_id, r, g, b);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_pixel(
    renderer_id: i64,
    x: i64,
    y: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_renderer_draw_pixel_impl(
                renderer_id as u64,
                x as u32,
                y as u32,
                r as u8,
                g as u8,
                b as u8,
            );
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (renderer_id, x, y, r, g, b);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_rect(
    renderer_id: i64,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_renderer_draw_rect_impl(
                renderer_id as u64,
                x as u32,
                y as u32,
                width as u32,
                height as u32,
                r as u8,
                g as u8,
                b as u8,
            );
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (renderer_id, x, y, width, height, r, g, b);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_line(
    renderer_id: i64,
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_renderer_draw_line_impl(
                renderer_id as u64,
                x0 as i32,
                y0 as i32,
                x1 as i32,
                y1 as i32,
                r as u8,
                g as u8,
                b as u8,
            );
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (renderer_id, x0, y0, x1, y1, r, g, b);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_circle(
    renderer_id: i64,
    cx: i64,
    cy: i64,
    radius: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_renderer_draw_circle_impl(
                renderer_id as u64,
                cx as i32,
                cy as i32,
                radius as u32,
                r as u8,
                g as u8,
                b as u8,
            );
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (renderer_id, cx, cy, radius, r, g, b);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_image(
    renderer_id: i64,
    file_path: *const c_char,
    x: i64,
    y: i64,
) -> i64 {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 || file_path.is_null() {
            return -1;
        }
        unsafe {
            qi_gui_renderer_draw_image_impl(renderer_id as u64, file_path, x as u32, y as u32)
                as i64
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (renderer_id, file_path, x, y);
        -1
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_text(
    renderer_id: i64,
    text: *const c_char,
    x: i64,
    y: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 || text.is_null() {
            return;
        }
        unsafe {
            qi_gui_renderer_draw_text_impl(
                renderer_id as u64,
                text,
                x as i32,
                y as i32,
                r as u8,
                g as u8,
                b as u8,
            );
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (renderer_id, text, x, y, r, g, b);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_renderer_draw_text_scaled(
    renderer_id: i64,
    text: *const c_char,
    x: i64,
    y: i64,
    scale: i64,
    r: i64,
    g: i64,
    b: i64,
) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 || text.is_null() {
            return;
        }
        unsafe {
            qi_gui_renderer_draw_text_scaled_impl(
                renderer_id as u64,
                text,
                x as i32,
                y as i32,
                scale as u32,
                r as u8,
                g as u8,
                b as u8,
            );
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = (renderer_id, text, x, y, scale, r, g, b);
    }
}

#[no_mangle]
pub extern "C" fn qi_gui_renderer_free(renderer_id: i64) {
    #[cfg(has_gui)]
    {
        if renderer_id <= 0 {
            return;
        }
        unsafe {
            qi_gui_renderer_free_impl(renderer_id as u64);
        }
    }

    #[cfg(not(has_gui))]
    {
        let _ = renderer_id;
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
