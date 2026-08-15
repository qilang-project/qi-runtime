//! 画布精灵层 FFI 转发 —— 在画布上贴图片（缩放 / 旋转 / 水平镜像）
//!
//! 与 `gui_ffi.rs` 同族：`has_gui` 时转发到 qi-gui 的 `*_impl`，不带 GUI 编译时
//! 用吞参数的桩保证链接通过。之所以单开一个文件而不是塞进 `gui_ffi.rs`，
//! 是因为那边已经 950 多行，再加就破了本仓「单文件 < 1000 行」的规矩。
//!
//! 面向儿童编程：这几个函数是 Scratch「角色」的平替，让孩子能把 png 摆到画布上
//! 转向、镜像。语义细节（坐标系、角度方向、缓存、失败占位）见 qi-gui/src/egui_sprite.rs。

use std::os::raw::c_char;

#[cfg(has_gui)]
extern "C" {
    fn qi_gui_egui_canvas_image_impl(path: *const c_char, x: i64, y: i64, width: i64, height: i64);
    fn qi_gui_egui_canvas_image_rotated_impl(
        path: *const c_char,
        cx: i64,
        cy: i64,
        width: i64,
        height: i64,
        degrees: i64,
    );
    fn qi_gui_egui_canvas_image_flipped_impl(
        path: *const c_char,
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        flip_h: i64,
    );
    fn qi_gui_egui_image_width_impl(path: *const c_char) -> i64;
    fn qi_gui_egui_image_height_impl(path: *const c_char) -> i64;
    fn qi_gui_egui_sprite_version_impl() -> i64;
}

/// 画布图片(路径, x, y, 宽, 高)：左上角对齐，拉伸到宽高（<=0 按原图比例）
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_image(
    path: *const c_char,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_image_impl(path, x, y, width, height) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (path, x, y, width, height);
    }
}

/// 画布图片旋转(路径, 中心x, 中心y, 宽, 高, 角度)：绕中心转，角度制，正数顺时针
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_image_rotated(
    path: *const c_char,
    cx: i64,
    cy: i64,
    width: i64,
    height: i64,
    degrees: i64,
) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_image_rotated_impl(path, cx, cy, width, height, degrees) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (path, cx, cy, width, height, degrees);
    }
}

/// 画布图片翻转(路径, x, y, 宽, 高, 水平翻)：水平翻 != 0 时左右镜像
#[no_mangle]
pub extern "C" fn qi_gui_egui_canvas_image_flipped(
    path: *const c_char,
    x: i64,
    y: i64,
    width: i64,
    height: i64,
    flip_h: i64,
) {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_canvas_image_flipped_impl(path, x, y, width, height, flip_h) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = (path, x, y, width, height, flip_h);
    }
}

/// 图片宽(路径) → 原始像素宽（读不到返回 0）
#[no_mangle]
pub extern "C" fn qi_gui_egui_image_width(path: *const c_char) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_image_width_impl(path) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = path;
        0
    }
}

/// 图片高(路径) → 原始像素高（读不到返回 0）
#[no_mangle]
pub extern "C" fn qi_gui_egui_image_height(path: *const c_char) -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_image_height_impl(path) }
    }
    #[cfg(not(has_gui))]
    {
        let _ = path;
        0
    }
}

/// 精灵层版本() → 批次号；不带 GUI 编译时返回 0，Qi 侧可据此判断能力是否在
#[no_mangle]
pub extern "C" fn qi_gui_egui_sprite_version() -> i64 {
    #[cfg(has_gui)]
    {
        unsafe { qi_gui_egui_sprite_version_impl() }
    }
    #[cfg(not(has_gui))]
    {
        0
    }
}

#[cfg(all(test, not(has_gui)))]
mod tests {
    use super::*;

    /// 不带 GUI 编译时，桩必须安静地返回默认值（否则 no-gui 构建一跑就崩）
    #[test]
    fn no_gui_stubs_are_safe() {
        assert_eq!(qi_gui_egui_sprite_version(), 0);
        assert_eq!(qi_gui_egui_image_width(std::ptr::null()), 0);
        assert_eq!(qi_gui_egui_image_height(std::ptr::null()), 0);
        qi_gui_egui_canvas_image(std::ptr::null(), 0, 0, 16, 16);
        qi_gui_egui_canvas_image_rotated(std::ptr::null(), 0, 0, 16, 16, 90);
        qi_gui_egui_canvas_image_flipped(std::ptr::null(), 0, 0, 16, 16, 1);
    }
}
