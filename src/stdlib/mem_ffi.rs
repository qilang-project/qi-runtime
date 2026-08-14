//! 裸内存读写原语 —— 给 C FFI 用的最后一块拼图。
//!
//! `外部 "c"` 能拿到 malloc 的 指针，也能把 Qi 顶层函数当回调传给 C，但 C 回调
//! 收到的 `void*` 在 Qi 侧一直是个不能拆的句柄：读不出里面的 i64，也写不进去。
//! qsort 的比较器就卡在这一步。这两个函数补上这条缝。
//!
//! 刻意不做边界检查：调用方是手写 FFI 的人，基址和偏移都由他自己保证，
//! 加一层「长度」参数只会制造安全的假象（Qi 拿不到 malloc 块的真实大小）。
//! 唯一的防线是 null 判断 —— 空指针是最常见的手滑，静默返回 0 好过段错误。

#![allow(non_snake_case)]

/// 从 base + byte_offset 处读一个 i64（本机字节序，按 8 字节对齐访问）。
/// base 为 null 时返回 0。
///
/// # Safety
/// 调用方必须保证 base + byte_offset 起的 8 个字节在同一块有效分配内。
#[no_mangle]
pub extern "C" fn qi_mem_read_i64(base: *const u8, byte_offset: i64) -> i64 {
    if base.is_null() {
        return 0;
    }
    unsafe { (base.offset(byte_offset as isize) as *const i64).read_unaligned() }
}

/// 往 base + byte_offset 处写一个 i64（本机字节序）。base 为 null 时什么也不做。
///
/// # Safety
/// 同 qi_mem_read_i64：8 字节的可写范围由调用方保证。
#[no_mangle]
pub extern "C" fn qi_mem_write_i64(base: *mut u8, byte_offset: i64, value: i64) {
    if base.is_null() {
        return;
    }
    unsafe { (base.offset(byte_offset as isize) as *mut i64).write_unaligned(value) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_then_read_roundtrip() {
        let mut buf = [0u8; 24];
        let p = buf.as_mut_ptr();
        qi_mem_write_i64(p, 0, -1);
        qi_mem_write_i64(p, 8, i64::MAX);
        qi_mem_write_i64(p, 16, 42);
        assert_eq!(qi_mem_read_i64(p, 0), -1);
        assert_eq!(qi_mem_read_i64(p, 8), i64::MAX);
        assert_eq!(qi_mem_read_i64(p, 16), 42);
    }

    #[test]
    fn null_base_is_inert() {
        assert_eq!(qi_mem_read_i64(std::ptr::null(), 0), 0);
        qi_mem_write_i64(std::ptr::null_mut(), 0, 7); // 不该崩
    }
}
