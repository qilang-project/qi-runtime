//! 字符串插值 / 拼接 / 数值转串的「一次分配」实现。
//!
//! 以前 `"key${i}"` 每次要三对 malloc+free：`i.to_string()` 的 `String` →
//! 拷进 RC 缓冲 → 再和 "key" 拼接出结果（中间的 int 串随即释放）。N 段插值
//! 还要额外 N-1 个中间串。这里的做法：
//!
//!   - 数值先格式化进**栈上**暂存区（整数手写十进制，浮点走 `Display`，与
//!     `to_string()` 逐字节相同），字符串段只记指针和长度；
//!   - 总长求出来后**只分配一次** RC 缓冲，各段按序拷进去。
//!
//! 编译器把 `"…${表达式}…"` 降成一次 [`qi_string_format_parts`] 调用：
//! 段数组放在调用方栈上，整数/浮点段原样传值（不再先转串），字符串段传指针。
//!
//! 本文件同时被 wasm 运行时 `#[path]` 编入，两个目标一份实现。

use std::ffi::CStr;
use std::fmt::Write as _;
use std::mem::MaybeUninit;
use std::os::raw::c_char;

use super::qi_str::{rc_cstr_build, rc_cstr_from_bytes};

/// 段类型：字符串（value = 指针）
pub const FMT_PART_STR: i64 = 0;
/// 段类型：整数（value = i64 本身；布尔已由编译器零扩展成 0/1）
pub const FMT_PART_INT: i64 = 1;
/// 段类型：浮点（value = f64 的位模式）
pub const FMT_PART_FLOAT: i64 = 2;

/// 插值段 —— 编译器在栈上排成 `[N x {i64, i64}]` 传进来。
/// 两个字段都用 i64，布局在 64 位与 wasm32 上一致（16 字节）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FmtPart {
    pub kind: i64,
    pub value: i64,
}

/// i64 的十进制表示，写进 `buf` 尾部并返回那一段（与 `i64::to_string` 同）。
#[inline]
pub fn format_i64(v: i64, buf: &mut [u8; 20]) -> &[u8] {
    let mut n = v.unsigned_abs();
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
        if n == 0 {
            break;
        }
    }
    if v < 0 {
        i -= 1;
        buf[i] = b'-';
    }
    &buf[i..]
}

/// 栈上字节暂存区：前 `INLINE` 字节不碰堆，写满后整体挪到 `Vec`。
/// 只追加；外面记的是**偏移**，挪堆后偏移照样有效。
struct Scratch<const INLINE: usize> {
    inline: [MaybeUninit<u8>; INLINE],
    len: usize,
    heap: Option<Vec<u8>>,
}

impl<const INLINE: usize> Scratch<INLINE> {
    #[inline]
    fn new() -> Self {
        Scratch {
            inline: [const { MaybeUninit::uninit() }; INLINE],
            len: 0,
            heap: None,
        }
    }

    #[inline]
    fn push(&mut self, bytes: &[u8]) {
        if let Some(h) = self.heap.as_mut() {
            h.extend_from_slice(bytes);
        } else if self.len + bytes.len() <= INLINE {
            unsafe {
                std::ptr::copy_nonoverlapping(
                    bytes.as_ptr(),
                    self.inline.as_mut_ptr().add(self.len) as *mut u8,
                    bytes.len(),
                );
            }
        } else {
            let mut h = Vec::with_capacity((self.len + bytes.len()) * 2);
            h.extend_from_slice(self.bytes());
            h.extend_from_slice(bytes);
            self.heap = Some(h);
        }
        self.len += bytes.len();
    }

    #[inline]
    fn bytes(&self) -> &[u8] {
        match &self.heap {
            Some(h) => h,
            // 前 len 字节都由 push 写过
            None => unsafe {
                std::slice::from_raw_parts(self.inline.as_ptr() as *const u8, self.len)
            },
        }
    }
}

impl<const INLINE: usize> std::fmt::Write for Scratch<INLINE> {
    #[inline]
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        self.push(s.as_bytes());
        Ok(())
    }
}

/// f64 → 与 `f64::to_string()` 逐字节相同的文本，写进暂存区。
#[inline]
fn push_f64<const N: usize>(scratch: &mut Scratch<N>, v: f64) {
    let _ = write!(scratch, "{}", v);
}

/// 整数 → RC 串：数字先落栈，只分配一次。
pub fn rc_cstr_from_i64(v: i64) -> *mut c_char {
    let mut buf = [0u8; 20];
    rc_cstr_from_bytes(format_i64(v, &mut buf))
}

/// 浮点 → RC 串（文本同 `to_string()`）：先落栈，只分配一次。
pub fn rc_cstr_from_f64(v: f64) -> *mut c_char {
    let mut scratch = Scratch::<64>::new();
    push_f64(&mut scratch, v);
    rc_cstr_from_bytes(scratch.bytes())
}

/// 读一个 C 串参数：null 或非法 UTF-8 → None（与 `qi_runtime_string_concat`
/// 的老口径一致：任一侧不合法，整个结果为 null）。
#[inline]
unsafe fn cstr_utf8<'a>(p: *const c_char) -> Option<&'a [u8]> {
    if p.is_null() {
        return None;
    }
    let bytes = CStr::from_ptr(p).to_bytes();
    std::str::from_utf8(bytes).ok().map(|s| s.as_bytes())
}

/// 两串拼接，一次分配。任一侧 null / 非法 UTF-8 → null（老语义不变）。
///
/// # Safety
/// 非 null 的参数必须是以 NUL 结尾的合法 C 串。
pub unsafe fn concat2(s1: *const c_char, s2: *const c_char) -> *mut c_char {
    let (a, b) = match (cstr_utf8(s1), cstr_utf8(s2)) {
        (Some(a), Some(b)) => (a, b),
        _ => return std::ptr::null_mut(),
    };
    rc_cstr_build(a.len() + b.len(), |dst| {
        std::ptr::copy_nonoverlapping(a.as_ptr(), dst, a.len());
        std::ptr::copy_nonoverlapping(b.as_ptr(), dst.add(a.len()), b.len());
    })
}

/// 一段的来源：字符串段直接指向原串，数值段指向暂存区里的偏移。
#[derive(Clone, Copy)]
enum Piece {
    Borrowed(*const u8, usize),
    Scratch(usize, usize),
}

/// 段数不超过这个值时，段表整个放栈上。
const INLINE_PIECES: usize = 16;

/// 插值段拼成一条新 RC 串（rc=1，调用方持有），全程只分配一次。
///
/// - 字符串段 null 或非法 UTF-8 → 返回 null（与逐段 `+` 拼接的老结果一致）
/// - 未知 kind 按整数处理（防御：编译器只发三种）
/// - 总长 0 → 静态 immortal 空串（free 为 no-op，与老拼接的空结果同口径）
///
/// # Safety
/// `parts` 指向 `n` 个连续的 [`FmtPart`]；字符串段的指针是以 NUL 结尾的合法 C 串。
#[no_mangle]
pub unsafe extern "C" fn qi_string_format_parts(parts: *const FmtPart, n: i64) -> *mut c_char {
    if parts.is_null() || n <= 0 {
        return rc_cstr_from_bytes(b"");
    }
    let parts = std::slice::from_raw_parts(parts, n as usize);

    let mut inline_pieces = [const { MaybeUninit::<Piece>::uninit() }; INLINE_PIECES];
    let mut heap_pieces: Vec<MaybeUninit<Piece>> = Vec::new();
    let pieces: &mut [MaybeUninit<Piece>] = if parts.len() <= INLINE_PIECES {
        &mut inline_pieces[..parts.len()]
    } else {
        heap_pieces.resize_with(parts.len(), MaybeUninit::uninit);
        &mut heap_pieces[..]
    };

    let mut scratch = Scratch::<256>::new();
    let mut total = 0usize;
    for (part, slot) in parts.iter().zip(pieces.iter_mut()) {
        let piece = match part.kind {
            FMT_PART_STR => match cstr_utf8(part.value as usize as *const c_char) {
                Some(b) => Piece::Borrowed(b.as_ptr(), b.len()),
                None => return std::ptr::null_mut(),
            },
            FMT_PART_FLOAT => {
                let off = scratch.len;
                push_f64(&mut scratch, f64::from_bits(part.value as u64));
                Piece::Scratch(off, scratch.len - off)
            }
            _ => {
                let off = scratch.len;
                let mut buf = [0u8; 20];
                scratch.push(format_i64(part.value, &mut buf));
                Piece::Scratch(off, scratch.len - off)
            }
        };
        total += match piece {
            Piece::Borrowed(_, l) | Piece::Scratch(_, l) => l,
        };
        slot.write(piece);
    }

    let scratch_bytes = scratch.bytes();
    rc_cstr_build(total, |dst| {
        let mut at = 0usize;
        for slot in pieces.iter() {
            let (src, len) = match slot.assume_init_read() {
                Piece::Borrowed(p, l) => (p, l),
                Piece::Scratch(off, l) => (scratch_bytes.as_ptr().add(off), l),
            };
            std::ptr::copy_nonoverlapping(src, dst.add(at), len);
            at += len;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stdlib::qi_str::rc_cstr_release;

    unsafe fn text(p: *mut c_char) -> String {
        CStr::from_ptr(p).to_str().unwrap().to_string()
    }

    fn s(p: &CStr) -> FmtPart {
        FmtPart {
            kind: FMT_PART_STR,
            value: p.as_ptr() as usize as i64,
        }
    }
    fn i(v: i64) -> FmtPart {
        FmtPart {
            kind: FMT_PART_INT,
            value: v,
        }
    }
    fn f(v: f64) -> FmtPart {
        FmtPart {
            kind: FMT_PART_FLOAT,
            value: v.to_bits() as i64,
        }
    }

    #[test]
    fn format_i64_matches_to_string() {
        for v in [
            0,
            1,
            -1,
            9,
            10,
            -10,
            123456789,
            i64::MAX,
            i64::MIN,
            i64::MIN + 1,
        ] {
            let mut buf = [0u8; 20];
            assert_eq!(format_i64(v, &mut buf), v.to_string().as_bytes());
        }
    }

    #[test]
    fn float_matches_to_string() {
        for v in [
            0.0,
            -0.0,
            1.5,
            0.1 + 0.2,
            1e300,
            -5e-324,
            f64::MAX,
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            3.0,
        ] {
            let p = rc_cstr_from_f64(v);
            assert_eq!(unsafe { text(p) }, v.to_string());
            rc_cstr_release(p);
        }
    }

    #[test]
    fn parts_mixed() {
        let key = c"key";
        let uni = c"你好🌏";
        let parts = [s(key), i(-42), s(c""), f(2.5), s(uni), i(0), f(-0.0)];
        unsafe {
            let p = qi_string_format_parts(parts.as_ptr(), parts.len() as i64);
            assert_eq!(text(p), "key-422.5你好🌏0-0");
            rc_cstr_release(p);
        }
    }

    #[test]
    fn parts_many_and_long_spill() {
        // 超过 INLINE_PIECES 段 + 暂存区溢出到堆（很长的浮点文本）
        let mut parts = Vec::new();
        let mut want = String::new();
        for k in 0..40 {
            parts.push(i(k));
            want += &k.to_string();
            parts.push(f(1e300));
            want += &1e300f64.to_string();
            parts.push(s(c"|"));
            want += "|";
        }
        unsafe {
            let p = qi_string_format_parts(parts.as_ptr(), parts.len() as i64);
            assert_eq!(text(p), want);
            rc_cstr_release(p);
        }
    }

    #[test]
    fn parts_empty_and_null() {
        unsafe {
            let p = qi_string_format_parts(std::ptr::null(), 0);
            assert_eq!(text(p), "");
            let parts = [s(c""), s(c"")];
            let p = qi_string_format_parts(parts.as_ptr(), 2);
            assert_eq!(text(p), "");
            rc_cstr_release(p); // immortal，no-op
            let parts = [
                s(c"a"),
                FmtPart {
                    kind: FMT_PART_STR,
                    value: 0,
                },
            ];
            assert!(qi_string_format_parts(parts.as_ptr(), 2).is_null());
            let bad = [0xFFu8, 0];
            let parts = [FmtPart {
                kind: FMT_PART_STR,
                value: bad.as_ptr() as usize as i64,
            }];
            assert!(qi_string_format_parts(parts.as_ptr(), 1).is_null());
        }
    }

    #[test]
    fn concat2_semantics() {
        unsafe {
            let p = concat2(c"ab".as_ptr(), c"中".as_ptr());
            assert_eq!(text(p), "ab中");
            rc_cstr_release(p);
            assert!(concat2(std::ptr::null(), c"x".as_ptr()).is_null());
            let p = concat2(c"".as_ptr(), c"".as_ptr());
            assert_eq!(text(p), "");
        }
    }

    #[test]
    fn int_to_rc() {
        let p = rc_cstr_from_i64(i64::MIN);
        assert_eq!(unsafe { text(p) }, i64::MIN.to_string());
        rc_cstr_release(p);
    }
}
