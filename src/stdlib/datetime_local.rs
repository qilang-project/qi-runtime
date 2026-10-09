//! 时间模块的本地时区部分。
//!
//! ## 语义（与编译器内嵌的 qi 版 `标准库/时间.qi` 一致）
//!
//! **时间戳永远是 Unix 秒，是一个绝对时刻，没有时区。** `年/月/日/时/分/秒/
//! 星期几/当天开始…` 这些不带「本地」的函数按 UTC 拆它 —— 这一点从来没变过，
//! 全仓也都按这个假设在用（qi-harness 的 排期 自己先平移到本地再调 `时()`）。
//!
//! 本地视角另起一套带「本地」前缀的函数：`本地年(ts)`…`本地时(ts)` 拿的就是
//! 墙上的钟。中间那层叫「本地墙钟读数」：把本地年月日时分秒当成 UTC 编码出来的
//! 整数（= ts + 时区偏移(ts)），只能拿给 UTC 系函数读分量，不是一个时刻。
//!
//!   - `时区偏移(ts)`   该时刻本地比 UTC 快多少秒（东八区 28800，夏令时自动算）
//!   - `转本地(ts)`     时刻 → 本地墙钟读数
//!   - `从本地(墙钟)`   本地墙钟读数 → 时刻（夏令时跳过的那一小时顺延，重复的那一小时取早的）
//!   - `本地时间()`     = 转本地(现在())。以前它返回 `Local::now().timestamp()`，
//!     而 Unix 秒本来就没有时区，于是跟 `现在()` 一模一样，
//!     `时间.时(时间.本地时间())` 在北京早上 9 点给 1。
//!
//! 本文件同时被 wasm 运行时 `#[path]` 编入（wasi 没有时区库，chrono 的 Local
//! 在那里就是 UTC，偏移恒为 0）。

use chrono::{DateTime, Local, NaiveDateTime, Offset, TimeZone};
use std::ffi::CStr;
use std::os::raw::c_char;

use super::datetime as dt;

/// 时刻 `ts` 处本地时区相对 UTC 的偏移（秒，东正西负）。
#[no_mangle]
pub extern "C" fn qi_datetime_local_offset(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(utc) => Local
            .offset_from_utc_datetime(&utc.naive_utc())
            .fix()
            .local_minus_utc() as i64,
        None => 0,
    }
}

/// 时刻 → 本地墙钟读数（本地年月日时分秒按 UTC 编码的整数）。
#[no_mangle]
pub extern "C" fn qi_datetime_to_local(timestamp: i64) -> i64 {
    timestamp + qi_datetime_local_offset(timestamp)
}

/// 本地墙钟读数 → 时刻。
///
/// 不用 chrono 的 `from_local_datetime`：它在夏令时结束那段重复的墙钟上
/// 给的是较晚的那个（实测纽约 11 月 1 日 1:30 → EST），跟文档说的不一致。
/// 这里自己做：取前后 12 小时的两个偏移各换算一次，能换回同一墙钟的就是
/// 合法解，取较早的；都换不回（夏令时开始时跳过的那段，不存在的本地时刻）
/// 就按跳变前的偏移换算，结果落在跳变之后（纽约 3 月 8 日 2:30 → 3:30 EDT）。
#[no_mangle]
pub extern "C" fn qi_datetime_from_local(wall: i64) -> i64 {
    let before = wall - qi_datetime_local_offset(wall - 43_200);
    let after = wall - qi_datetime_local_offset(wall + 43_200);
    let ok = |t: i64| qi_datetime_to_local(t) == wall;
    match (ok(before), ok(after)) {
        (true, true) => before.min(after),
        (true, false) => before,
        (false, true) => after,
        (false, false) => before,
    }
}

/// 当前时刻的本地墙钟读数。
#[no_mangle]
pub extern "C" fn qi_datetime_now_local() -> i64 {
    qi_datetime_to_local(dt::qi_datetime_now())
}

/// 按本地时区解析：串里写的是本地墙钟，返回时刻。解析失败返回 0（同 `解析`）。
#[no_mangle]
pub extern "C" fn qi_datetime_parse_local(
    datetime_str: *const c_char,
    format: *const c_char,
) -> i64 {
    if datetime_str.is_null() || format.is_null() {
        return 0;
    }
    let parsed = unsafe {
        (
            CStr::from_ptr(datetime_str).to_str(),
            CStr::from_ptr(format).to_str(),
        )
    };
    let (Ok(s), Ok(f)) = parsed else {
        return 0;
    };
    match NaiveDateTime::parse_from_str(s, f) {
        Ok(ndt) => qi_datetime_from_local(ndt.and_utc().timestamp()),
        Err(_) => 0,
    }
}

// ── 本地分量：先转墙钟读数，再用 UTC 那套拆 ──────────────────────────

macro_rules! local_component {
    ($name:ident, $utc:path) => {
        #[no_mangle]
        pub extern "C" fn $name(timestamp: i64) -> i64 {
            $utc(qi_datetime_to_local(timestamp))
        }
    };
}

local_component!(qi_datetime_local_year, dt::qi_datetime_year);
local_component!(qi_datetime_local_month, dt::qi_datetime_month);
local_component!(qi_datetime_local_day, dt::qi_datetime_day);
local_component!(qi_datetime_local_hour, dt::qi_datetime_hour);
local_component!(qi_datetime_local_minute, dt::qi_datetime_minute);
local_component!(qi_datetime_local_second, dt::qi_datetime_second);
local_component!(qi_datetime_local_weekday, dt::qi_datetime_weekday);
local_component!(qi_datetime_local_quarter, dt::qi_datetime_quarter);
local_component!(qi_datetime_local_day_of_year, dt::qi_datetime_day_of_year);

/// 本地当天 00:00:00 对应的时刻。
#[no_mangle]
pub extern "C" fn qi_datetime_local_start_of_day(timestamp: i64) -> i64 {
    qi_datetime_from_local(dt::qi_datetime_start_of_day(qi_datetime_to_local(
        timestamp,
    )))
}

/// 本地当天 23:59:59 对应的时刻。
#[no_mangle]
pub extern "C" fn qi_datetime_local_end_of_day(timestamp: i64) -> i64 {
    qi_datetime_from_local(dt::qi_datetime_end_of_day(qi_datetime_to_local(timestamp)))
}

/// 本地 年-月-日 00:00:00 对应的时刻。
#[no_mangle]
pub extern "C" fn qi_datetime_from_local_ymd(year: i64, month: i64, day: i64) -> i64 {
    qi_datetime_from_local(dt::qi_datetime_from_ymd(year, month, day))
}

/// 本地 年-月-日 时:分:秒 对应的时刻。
#[no_mangle]
pub extern "C" fn qi_datetime_from_local_ymdhms(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> i64 {
    qi_datetime_from_local(dt::qi_datetime_from_ymdhms(
        year, month, day, hour, minute, second,
    ))
}

/// 是不是本地的今天。
#[no_mangle]
pub extern "C" fn qi_datetime_is_local_today(timestamp: i64) -> i64 {
    let now = dt::qi_datetime_now();
    let a = qi_datetime_local_start_of_day(now);
    let b = qi_datetime_local_end_of_day(now);
    (timestamp >= a && timestamp <= b) as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_any_zone() {
        for ts in [0i64, 1_700_000_000, 1_759_971_600, -86_400 * 400] {
            assert_eq!(qi_datetime_from_local(qi_datetime_to_local(ts)), ts);
        }
    }

    #[test]
    fn local_components_follow_offset() {
        let ts = 1_759_971_600; // 2025-10-09 01:00:00 UTC
        let off = qi_datetime_local_offset(ts);
        assert_eq!(
            qi_datetime_local_hour(ts),
            dt::qi_datetime_hour(ts + off),
            "本地时 = UTC 时 + 偏移"
        );
        assert_eq!(
            qi_datetime_from_local_ymdhms(2025, 10, 9, 9, 0, 0),
            dt::qi_datetime_from_ymdhms(2025, 10, 9, 9, 0, 0)
                - qi_datetime_local_offset(qi_datetime_from_local_ymdhms(2025, 10, 9, 9, 0, 0))
        );
    }

    #[test]
    fn now_local_is_wall_clock_reading() {
        let a = qi_datetime_now_local();
        let now = dt::qi_datetime_now();
        let diff = a - (now + qi_datetime_local_offset(now));
        assert!(diff.abs() <= 1, "本地时间() 应为 现在()+偏移，差 {diff}");
    }
}
