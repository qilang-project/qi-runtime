//! 窗口内视频 / 原生文件对话框 / 原生菜单栏 FFI 转发
//!
//! 与 `gui_ffi.rs` 同族：`has_gui` 时转发到 qi-gui 的 `*_impl`，不带 GUI 编译时用
//! 吞参数的桩保证链接通过（视频加载返回 0、对话框返回空串、菜单返回 0，qi 侧都有退路）。
//! 单开文件是因为 `gui_ffi.rs` 已经过了「单文件 < 1000 行」。
//!
//! 语义细节见 qi-gui 的 `egui_video.rs` / `native_dialog.rs` / `native_menu.rs`。
//! 返回字符串的一律用 `qi_string_from_cstr` 拷成 RC 串（qi-gui 返回的是单槽复用缓冲）。

use std::os::raw::c_char;

#[cfg(has_gui)]
extern "C" {
    fn qi_gui_egui_video_available_impl() -> i64;
    fn qi_gui_egui_video_load_impl(path: *const c_char) -> u64;
    fn qi_gui_egui_video_draw_impl(id: u64, max_width: i64) -> i64;
    fn qi_gui_egui_video_play_impl(id: u64);
    fn qi_gui_egui_video_pause_impl(id: u64);
    fn qi_gui_egui_video_seek_impl(id: u64, ms: i64);
    fn qi_gui_egui_video_position_impl(id: u64) -> i64;
    fn qi_gui_egui_video_duration_impl(id: u64) -> i64;
    fn qi_gui_egui_video_playing_impl(id: u64) -> i64;
    fn qi_gui_egui_video_free_impl(id: u64);

    fn qi_gui_dialog_open_file_impl(title: *const c_char, filters: *const c_char) -> *const c_char;
    fn qi_gui_dialog_save_file_impl(
        title: *const c_char,
        default_name: *const c_char,
    ) -> *const c_char;
    fn qi_gui_dialog_pick_folder_impl(title: *const c_char) -> *const c_char;

    fn qi_gui_menu_add_impl(menu: *const c_char, item: *const c_char, accel: *const c_char) -> i64;
    fn qi_gui_menu_separator_impl(menu: *const c_char);
    fn qi_gui_menu_add_edit_impl(menu: *const c_char) -> i64;
    fn qi_gui_menu_clicked_impl() -> i64;
}

// ============================================================================
// 视频
// ============================================================================

/// 视频可用() → 1 本机有 ffmpeg 和 ffprobe / 0
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_available() -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_video_available_impl() }
    }
    #[cfg(not(has_gui))]
    {
        0
    }
}

/// 视频加载(路径) → 句柄，失败（没装 ffmpeg / 打不开 / 没画面）0
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_load(path: *const c_char) -> i64 {
    #[cfg(has_gui)]
    {
        if path.is_null() {
            return 0;
        }
        unsafe { qi_gui_egui_video_load_impl(path) as i64 }
    }
    #[cfg(not(has_gui))]
    {
        let _ = path;
        0
    }
}

/// 视频画(句柄, 最大宽) → 1 画面本帧被点了 / 0
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_draw(id: i64, max_width: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if id <= 0 {
            return 0;
        }
        unsafe { qi_gui_egui_video_draw_impl(id as u64, max_width) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, max_width);
        0
    }
}

/// 视频播放(句柄)：放完了再播从头开始
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_play(id: i64) {
    #[cfg(has_gui)]
    {
        if id > 0 {
            unsafe { qi_gui_egui_video_play_impl(id as u64) }
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = id;
    }
}

/// 视频暂停(句柄)
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_pause(id: i64) {
    #[cfg(has_gui)]
    {
        if id > 0 {
            unsafe { qi_gui_egui_video_pause_impl(id as u64) }
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = id;
    }
}

/// 视频跳到(句柄, 毫秒)
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_seek(id: i64, ms: i64) {
    #[cfg(has_gui)]
    {
        if id > 0 {
            unsafe { qi_gui_egui_video_seek_impl(id as u64, ms) }
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (id, ms);
    }
}

/// 视频位置(句柄) → 毫秒
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_position(id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if id <= 0 {
            return 0;
        }
        unsafe { qi_gui_egui_video_position_impl(id as u64) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = id;
        0
    }
}

/// 视频时长(句柄) → 毫秒，量不出 0
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_duration(id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if id <= 0 {
            return 0;
        }
        unsafe { qi_gui_egui_video_duration_impl(id as u64) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = id;
        0
    }
}

/// 视频播放中(句柄) → 1 / 0（放到头自动停）
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_playing(id: i64) -> i64 {
    #[cfg(has_gui)]
    {
        if id <= 0 {
            return 0;
        }
        unsafe { qi_gui_egui_video_playing_impl(id as u64) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = id;
        0
    }
}

/// 视频释放(句柄)：停掉、杀掉解码进程
#[no_mangle]
pub extern "C" fn qi_gui_egui_video_free(id: i64) {
    #[cfg(has_gui)]
    {
        if id > 0 {
            unsafe { qi_gui_egui_video_free_impl(id as u64) }
        }
    }
    #[cfg(not(has_gui))]
    {
        let _ = id;
    }
}

// ============================================================================
// 文件对话框
// ============================================================================

/// 选择打开文件(标题, 扩展名过滤) → 路径，取消空串。
/// 过滤：`md,txt` 一组；`图片:png,jpg;音频:mp3` 多组；空串不过滤
#[no_mangle]
pub extern "C" fn qi_gui_dialog_open_file(
    title: *const c_char,
    filters: *const c_char,
) -> *mut c_char {
    #[cfg(has_gui)]
    {
        super::qi_str::qi_string_from_cstr(unsafe { qi_gui_dialog_open_file_impl(title, filters) })
    }
    #[cfg(not(has_gui))]
    {
        let _ = (title, filters);
        super::qi_str::qi_string_from_cstr(std::ptr::null())
    }
}

/// 选择保存文件(标题, 默认名) → 路径，取消空串
#[no_mangle]
pub extern "C" fn qi_gui_dialog_save_file(
    title: *const c_char,
    default_name: *const c_char,
) -> *mut c_char {
    #[cfg(has_gui)]
    {
        super::qi_str::qi_string_from_cstr(unsafe {
            qi_gui_dialog_save_file_impl(title, default_name)
        })
    }
    #[cfg(not(has_gui))]
    {
        let _ = (title, default_name);
        super::qi_str::qi_string_from_cstr(std::ptr::null())
    }
}

/// 选择文件夹(标题) → 路径，取消空串
#[no_mangle]
pub extern "C" fn qi_gui_dialog_pick_folder(title: *const c_char) -> *mut c_char {
    #[cfg(has_gui)]
    {
        super::qi_str::qi_string_from_cstr(unsafe { qi_gui_dialog_pick_folder_impl(title) })
    }
    #[cfg(not(has_gui))]
    {
        let _ = title;
        super::qi_str::qi_string_from_cstr(std::ptr::null())
    }
}

// ============================================================================
// 菜单栏
// ============================================================================

/// 菜单添加(菜单名, 项名, 快捷键) → 编号；没有原生菜单的平台 0
#[no_mangle]
pub extern "C" fn qi_gui_menu_add(
    menu: *const c_char,
    item: *const c_char,
    accel: *const c_char,
) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_menu_add_impl(menu, item, accel) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (menu, item, accel);
        0
    }
}

/// 菜单分隔线(菜单名)
#[no_mangle]
pub extern "C" fn qi_gui_menu_separator(menu: *const c_char) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_menu_separator_impl(menu) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = menu;
    }
}

/// 菜单添加编辑项(菜单名) → 1 / 0
#[no_mangle]
pub extern "C" fn qi_gui_menu_add_edit(menu: *const c_char) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_menu_add_edit_impl(menu) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = menu;
        0
    }
}

/// 菜单被点() → 编号，没有 0
#[no_mangle]
pub extern "C" fn qi_gui_menu_clicked() -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_menu_clicked_impl() }
    }
    #[cfg(not(has_gui))]
    {
        0
    }
}
