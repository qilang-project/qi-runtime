//! 时间模块 (Time Module)
//!
//! 提供完整的时间和日期处理功能
//! - 时间获取：当前时间、时间戳转换
//! - 时间格式化：多种格式输出
//! - 日期组件：年月日时分秒、星期、季度
//! - 时间计算：加减、时间差、时间范围
//! - 日期工具：闰年、月天数、周数
//! - 时间边界：当天/本周/本月/本年的开始和结束
//!
//! Provides comprehensive time and date handling

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveDateTime, Timelike, Utc};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::thread;

// ============================================================================
// 当前时间 (Current Time)
// ============================================================================

/// 获取当前 Unix 时间戳（秒）
#[no_mangle]
pub extern "C" fn qi_datetime_now() -> i64 {
    Utc::now().timestamp()
}

/// 获取当前 Unix 时间戳（毫秒）
#[no_mangle]
pub extern "C" fn qi_datetime_now_millis() -> i64 {
    Utc::now().timestamp_millis()
}

/// 获取当前本地时间戳
#[no_mangle]
pub extern "C" fn qi_datetime_now_local() -> i64 {
    Local::now().timestamp()
}

/// 获取当前 Unix 时间戳（微秒）
#[no_mangle]
pub extern "C" fn qi_datetime_now_micros() -> i64 {
    Utc::now().timestamp_micros()
}

/// 获取当前 Unix 时间戳（纳秒）
#[no_mangle]
pub extern "C" fn qi_datetime_now_nanos() -> i64 {
    Utc::now().timestamp_nanos_opt().unwrap_or(0)
}

// ============================================================================
// 格式化 (Formatting)
// ============================================================================

/// 格式化 Unix 时间戳为字符串（UTC）
/// format: "%Y-%m-%d %H:%M:%S", "%Y年%m月%d日", etc.
#[no_mangle]
pub extern "C" fn qi_datetime_format(timestamp: i64, format: *const c_char) -> *mut c_char {
    if format.is_null() {
        return std::ptr::null_mut();
    }

    let format_str = unsafe {
        match CStr::from_ptr(format).to_str() {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        }
    };

    let dt = match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt,
        None => return std::ptr::null_mut(),
    };

    let formatted = dt.format(format_str).to_string();
    CString::new(formatted).unwrap().into_raw()
}

/// 格式化 Unix 时间戳为字符串（本地时间）
#[no_mangle]
pub extern "C" fn qi_datetime_format_local(timestamp: i64, format: *const c_char) -> *mut c_char {
    if format.is_null() {
        return std::ptr::null_mut();
    }

    let format_str = unsafe {
        match CStr::from_ptr(format).to_str() {
            Ok(s) => s,
            Err(_) => return std::ptr::null_mut(),
        }
    };

    let dt = match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.with_timezone(&Local),
        None => return std::ptr::null_mut(),
    };

    let formatted = dt.format(format_str).to_string();
    CString::new(formatted).unwrap().into_raw()
}

// ============================================================================
// 解析 (Parsing)
// ============================================================================

/// 解析日期时间字符串为 Unix 时间戳
/// format: "%Y-%m-%d %H:%M:%S"
/// datetime_str: "2024-01-15 14:30:00"
#[no_mangle]
pub extern "C" fn qi_datetime_parse(datetime_str: *const c_char, format: *const c_char) -> i64 {
    if datetime_str.is_null() || format.is_null() {
        return 0;
    }

    let dt_str = unsafe {
        match CStr::from_ptr(datetime_str).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    let fmt_str = unsafe {
        match CStr::from_ptr(format).to_str() {
            Ok(s) => s,
            Err(_) => return 0,
        }
    };

    match NaiveDateTime::parse_from_str(dt_str, fmt_str) {
        Ok(ndt) => ndt.and_utc().timestamp(),
        Err(_) => 0,
    }
}

// ============================================================================
// 日期组件 (Date Components)
// ============================================================================

/// 获取年份
#[no_mangle]
pub extern "C" fn qi_datetime_year(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.year() as i64,
        None => 0,
    }
}

/// 获取月份（1-12）
#[no_mangle]
pub extern "C" fn qi_datetime_month(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.month() as i64,
        None => 0,
    }
}

/// 获取日期（1-31）
#[no_mangle]
pub extern "C" fn qi_datetime_day(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.day() as i64,
        None => 0,
    }
}

/// 获取小时（0-23）
#[no_mangle]
pub extern "C" fn qi_datetime_hour(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.hour() as i64,
        None => 0,
    }
}

/// 获取分钟（0-59）
#[no_mangle]
pub extern "C" fn qi_datetime_minute(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.minute() as i64,
        None => 0,
    }
}

/// 获取秒（0-59）
#[no_mangle]
pub extern "C" fn qi_datetime_second(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.second() as i64,
        None => 0,
    }
}

/// 获取星期几（1=周一, 7=周日）
#[no_mangle]
pub extern "C" fn qi_datetime_weekday(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.weekday().num_days_from_monday() as i64 + 1,
        None => 0,
    }
}

/// 获取季度（1-4）
#[no_mangle]
pub extern "C" fn qi_datetime_quarter(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => ((dt.month() - 1) / 3 + 1) as i64,
        None => 0,
    }
}

/// 获取年份的第几天（1-366）
#[no_mangle]
pub extern "C" fn qi_datetime_day_of_year(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.ordinal() as i64,
        None => 0,
    }
}

/// 获取年份的第几周（1-53）
#[no_mangle]
pub extern "C" fn qi_datetime_week_of_year(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => dt.iso_week().week() as i64,
        None => 0,
    }
}

/// 获取毫秒部分（0-999）
#[no_mangle]
pub extern "C" fn qi_datetime_millisecond(timestamp_millis: i64) -> i64 {
    (timestamp_millis % 1000).abs()
}

// ============================================================================
// 日期计算 (Date Calculations)
// ============================================================================

/// 添加秒数
#[no_mangle]
pub extern "C" fn qi_datetime_add_seconds(timestamp: i64, seconds: i64) -> i64 {
    timestamp + seconds
}

/// 添加分钟数
#[no_mangle]
pub extern "C" fn qi_datetime_add_minutes(timestamp: i64, minutes: i64) -> i64 {
    timestamp + (minutes * 60)
}

/// 添加小时数
#[no_mangle]
pub extern "C" fn qi_datetime_add_hours(timestamp: i64, hours: i64) -> i64 {
    timestamp + (hours * 3600)
}

/// 添加天数
#[no_mangle]
pub extern "C" fn qi_datetime_add_days(timestamp: i64, days: i64) -> i64 {
    timestamp + (days * 86400)
}

/// 计算两个时间戳之间的天数差
#[no_mangle]
pub extern "C" fn qi_datetime_diff_days(timestamp1: i64, timestamp2: i64) -> i64 {
    (timestamp1 - timestamp2) / 86400
}

/// 计算两个时间戳之间的小时数差
#[no_mangle]
pub extern "C" fn qi_datetime_diff_hours(timestamp1: i64, timestamp2: i64) -> i64 {
    (timestamp1 - timestamp2) / 3600
}

/// 计算两个时间戳之间的分钟数差
#[no_mangle]
pub extern "C" fn qi_datetime_diff_minutes(timestamp1: i64, timestamp2: i64) -> i64 {
    (timestamp1 - timestamp2) / 60
}

/// 计算两个时间戳之间的秒数差
#[no_mangle]
pub extern "C" fn qi_datetime_diff_seconds(timestamp1: i64, timestamp2: i64) -> i64 {
    timestamp1 - timestamp2
}

/// 添加周数
#[no_mangle]
pub extern "C" fn qi_datetime_add_weeks(timestamp: i64, weeks: i64) -> i64 {
    timestamp + (weeks * 7 * 86400)
}

/// 添加月数
#[no_mangle]
pub extern "C" fn qi_datetime_add_months(timestamp: i64, months: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let naive = dt.naive_utc();
            let new_date = if months >= 0 {
                naive.date() + Duration::days(months * 30)
            } else {
                naive.date() - Duration::days((-months) * 30)
            };
            new_date.and_time(naive.time()).and_utc().timestamp()
        }
        None => 0,
    }
}

/// 添加年数
#[no_mangle]
pub extern "C" fn qi_datetime_add_years(timestamp: i64, years: i64) -> i64 {
    timestamp + (years * 365 * 86400)
}

// ============================================================================
// 日期创建 (Date Creation)
// ============================================================================

/// 从年月日创建日期时间戳（UTC，时分秒为 0）
#[no_mangle]
pub extern "C" fn qi_datetime_from_ymd(year: i64, month: i64, day: i64) -> i64 {
    match NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32) {
        Some(date) => date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp(),
        None => 0,
    }
}

/// 从年月日时分秒创建时间戳（UTC）
#[no_mangle]
pub extern "C" fn qi_datetime_from_ymdhms(
    year: i64,
    month: i64,
    day: i64,
    hour: i64,
    minute: i64,
    second: i64,
) -> i64 {
    match NaiveDate::from_ymd_opt(year as i32, month as u32, day as u32) {
        Some(date) => match date.and_hms_opt(hour as u32, minute as u32, second as u32) {
            Some(dt) => dt.and_utc().timestamp(),
            None => 0,
        },
        None => 0,
    }
}

// ============================================================================
// 工具函数 (Utility Functions)
// ============================================================================

/// 检查是否为闰年
#[no_mangle]
pub extern "C" fn qi_datetime_is_leap_year(year: i64) -> i64 {
    let year = year as i32;
    if (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0) {
        1
    } else {
        0
    }
}

/// 获取某月的天数
#[no_mangle]
pub extern "C" fn qi_datetime_days_in_month(year: i64, month: i64) -> i64 {
    if month < 1 || month > 12 {
        return 0;
    }

    match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 => {
            if qi_datetime_is_leap_year(year) == 1 {
                29
            } else {
                28
            }
        }
        _ => 0,
    }
}

// ============================================================================
// 时间边界 (Time Boundaries)
// ============================================================================

/// 获取当天开始时间戳（0时0分0秒）
#[no_mangle]
pub extern "C" fn qi_datetime_start_of_day(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let date = dt.date_naive();
            date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp()
        }
        None => 0,
    }
}

/// 获取当天结束时间戳（23时59分59秒）
#[no_mangle]
pub extern "C" fn qi_datetime_end_of_day(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let date = dt.date_naive();
            date.and_hms_opt(23, 59, 59).unwrap().and_utc().timestamp()
        }
        None => 0,
    }
}

/// 获取本周开始时间戳（周一0时0分0秒）
#[no_mangle]
pub extern "C" fn qi_datetime_start_of_week(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let weekday = dt.weekday().num_days_from_monday() as i64;
            let days_to_subtract = weekday;
            let start_timestamp = timestamp - (days_to_subtract * 86400);
            qi_datetime_start_of_day(start_timestamp)
        }
        None => 0,
    }
}

/// 获取本周结束时间戳（周日23时59分59秒）
#[no_mangle]
pub extern "C" fn qi_datetime_end_of_week(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let weekday = dt.weekday().num_days_from_monday() as i64;
            let days_to_add = 6 - weekday;
            let end_timestamp = timestamp + (days_to_add * 86400);
            qi_datetime_end_of_day(end_timestamp)
        }
        None => 0,
    }
}

/// 获取本月开始时间戳（1日0时0分0秒）
#[no_mangle]
pub extern "C" fn qi_datetime_start_of_month(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => qi_datetime_from_ymd(dt.year() as i64, dt.month() as i64, 1),
        None => 0,
    }
}

/// 获取本月结束时间戳（最后一日23时59分59秒）
#[no_mangle]
pub extern "C" fn qi_datetime_end_of_month(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let year = dt.year() as i64;
            let month = dt.month() as i64;
            let last_day = qi_datetime_days_in_month(year, month);
            let last_day_timestamp = qi_datetime_from_ymd(year, month, last_day);
            qi_datetime_end_of_day(last_day_timestamp)
        }
        None => 0,
    }
}

/// 获取本年开始时间戳（1月1日0时0分0秒）
#[no_mangle]
pub extern "C" fn qi_datetime_start_of_year(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => qi_datetime_from_ymd(dt.year() as i64, 1, 1),
        None => 0,
    }
}

/// 获取本年结束时间戳（12月31日23时59分59秒）
#[no_mangle]
pub extern "C" fn qi_datetime_end_of_year(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let year = dt.year() as i64;
            let last_day_timestamp = qi_datetime_from_ymd(year, 12, 31);
            qi_datetime_end_of_day(last_day_timestamp)
        }
        None => 0,
    }
}

/// 获取本季度开始时间戳
#[no_mangle]
pub extern "C" fn qi_datetime_start_of_quarter(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let year = dt.year() as i64;
            let quarter = qi_datetime_quarter(timestamp);
            let start_month = (quarter - 1) * 3 + 1;
            qi_datetime_from_ymd(year, start_month, 1)
        }
        None => 0,
    }
}

/// 获取本季度结束时间戳
#[no_mangle]
pub extern "C" fn qi_datetime_end_of_quarter(timestamp: i64) -> i64 {
    match DateTime::from_timestamp(timestamp, 0) {
        Some(dt) => {
            let year = dt.year() as i64;
            let quarter = qi_datetime_quarter(timestamp);
            let end_month = quarter * 3;
            let last_day = qi_datetime_days_in_month(year, end_month);
            let last_day_timestamp = qi_datetime_from_ymd(year, end_month, last_day);
            qi_datetime_end_of_day(last_day_timestamp)
        }
        None => 0,
    }
}

// ============================================================================
// 时间判断 (Time Comparison)
// ============================================================================

/// 判断时间戳是否在指定范围内（包含边界）
#[no_mangle]
pub extern "C" fn qi_datetime_is_between(timestamp: i64, start: i64, end: i64) -> i64 {
    if timestamp >= start && timestamp <= end {
        1
    } else {
        0
    }
}

/// 判断是否是今天
#[no_mangle]
pub extern "C" fn qi_datetime_is_today(timestamp: i64) -> i64 {
    let now = qi_datetime_now();
    let today_start = qi_datetime_start_of_day(now);
    let today_end = qi_datetime_end_of_day(now);
    qi_datetime_is_between(timestamp, today_start, today_end)
}

/// 判断是否是本周
#[no_mangle]
pub extern "C" fn qi_datetime_is_this_week(timestamp: i64) -> i64 {
    let now = qi_datetime_now();
    let week_start = qi_datetime_start_of_week(now);
    let week_end = qi_datetime_end_of_week(now);
    qi_datetime_is_between(timestamp, week_start, week_end)
}

/// 判断是否是本月
#[no_mangle]
pub extern "C" fn qi_datetime_is_this_month(timestamp: i64) -> i64 {
    let now = qi_datetime_now();
    let month_start = qi_datetime_start_of_month(now);
    let month_end = qi_datetime_end_of_month(now);
    qi_datetime_is_between(timestamp, month_start, month_end)
}

/// 判断是否是本年
#[no_mangle]
pub extern "C" fn qi_datetime_is_this_year(timestamp: i64) -> i64 {
    let now = qi_datetime_now();
    let year_start = qi_datetime_start_of_year(now);
    let year_end = qi_datetime_end_of_year(now);
    qi_datetime_is_between(timestamp, year_start, year_end)
}

/// 判断是否是周末（周六或周日）
#[no_mangle]
pub extern "C" fn qi_datetime_is_weekend(timestamp: i64) -> i64 {
    let weekday = qi_datetime_weekday(timestamp);
    if weekday == 6 || weekday == 7 {
        1
    } else {
        0
    }
}

/// 判断是否是工作日（周一到周五）
#[no_mangle]
pub extern "C" fn qi_datetime_is_weekday(timestamp: i64) -> i64 {
    let weekday = qi_datetime_weekday(timestamp);
    if weekday >= 1 && weekday <= 5 {
        1
    } else {
        0
    }
}

// ============================================================================
// 时间转换 (Time Conversion)
// ============================================================================

/// 秒转毫秒
#[no_mangle]
pub extern "C" fn qi_datetime_seconds_to_millis(seconds: i64) -> i64 {
    seconds * 1000
}

/// 毫秒转秒
#[no_mangle]
pub extern "C" fn qi_datetime_millis_to_seconds(millis: i64) -> i64 {
    millis / 1000
}

/// 秒转微秒
#[no_mangle]
pub extern "C" fn qi_datetime_seconds_to_micros(seconds: i64) -> i64 {
    seconds * 1_000_000
}

/// 微秒转秒
#[no_mangle]
pub extern "C" fn qi_datetime_micros_to_seconds(micros: i64) -> i64 {
    micros / 1_000_000
}

// ============================================================================
// 睡眠与延迟 (Sleep and Delay)
// ============================================================================

/// 睡眠指定秒数
#[no_mangle]
pub extern "C" fn qi_datetime_sleep_seconds(seconds: i64) {
    if seconds > 0 {
        thread::sleep(std::time::Duration::from_secs(seconds as u64));
    }
}

/// 睡眠指定毫秒数（同步阻塞 OS 线程）
#[no_mangle]
pub extern "C" fn qi_datetime_sleep_millis(millis: i64) {
    if millis > 0 {
        thread::sleep(std::time::Duration::from_millis(millis as u64));
    }
}

/// 异步睡眠（同步阻塞版）— 在 tokio task 里调走 block_in_place + block_on，
/// 在普通线程里走 thread::sleep。仍 pin 一个 worker。
#[no_mangle]
pub extern "C" fn qi_datetime_async_sleep_millis(millis: i64) {
    if millis <= 0 {
        return;
    }
    let dur = std::time::Duration::from_millis(millis as u64);
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| {
            handle.block_on(tokio::time::sleep(dur));
        });
    } else {
        thread::sleep(dur);
    }
}

/// 异步睡眠（**返回 未来<空>** 版）— 真正的 Future API。
///
/// 调用方式（qi 侧）：
/// ```qi
/// 等待 时间.异步睡眠未来(1000);  // 等 1 秒，CPU 占用 0
/// ```
///
/// 实现：返回一个 pending Future，spawn 一个 tokio task 跑 sleep，sleep 完成后
/// complete() 这个 future。qi `等待` 通过 Future::await_value（已改成 block_on）
/// 等待 Notify。
///
/// 注意：仍受语言限制 —— `等待` 在 sync wrapper 里 block 当前 worker。要 1000 个
/// 这种 sleep 在 12 个线程上并发完成，需要 compiler async/await 把 `等待` 真正
/// 编译成 .await（让 task 自己 yield）。这一步等下个 milestone。
#[no_mangle]
pub extern "C" fn qi_datetime_async_sleep_future(
    millis: i64,
) -> *mut crate::async_runtime::future::Future {
    use crate::async_runtime::future::{Future, FutureValue};
    let f = Box::new(Future::pending());
    let f_ptr = Box::into_raw(f);

    // clone Arc 字段供 task 用
    let state = unsafe { (*f_ptr).state.clone() };
    let value = unsafe { (*f_ptr).value.clone() };
    let notify = unsafe { (*f_ptr).notify.clone() };

    let dur = std::time::Duration::from_millis(millis.max(0) as u64);
    crate::async_runtime::ffi::全局异步运行时().spawn(async move {
        tokio::time::sleep(dur).await;
        *value.lock().unwrap() = Some(FutureValue::None);
        *state.lock().unwrap() = crate::async_runtime::future::FutureState::Completed;
        notify.notify_waiters();
    });

    f_ptr
}

/// 睡眠指定微秒数
#[no_mangle]
pub extern "C" fn qi_datetime_sleep_micros(micros: i64) {
    if micros > 0 {
        thread::sleep(std::time::Duration::from_micros(micros as u64));
    }
}

/// 释放字符串
#[no_mangle]
pub extern "C" fn qi_datetime_free_string(s: *mut c_char) {
    if s.is_null() {
        return;
    }
    unsafe {
        let _ = CString::from_raw(s);
    }
}

/// 判断两个时间戳是否在同一天
#[no_mangle]
pub extern "C" fn qi_datetime_is_same_day(timestamp1: i64, timestamp2: i64) -> i64 {
    let start1 = qi_datetime_start_of_day(timestamp1);
    let start2 = qi_datetime_start_of_day(timestamp2);
    if start1 == start2 {
        1
    } else {
        0
    }
}

/// 判断两个时间戳是否在同一月
#[no_mangle]
pub extern "C" fn qi_datetime_is_same_month(timestamp1: i64, timestamp2: i64) -> i64 {
    let start1 = qi_datetime_start_of_month(timestamp1);
    let start2 = qi_datetime_start_of_month(timestamp2);
    if start1 == start2 {
        1
    } else {
        0
    }
}

/// 判断两个时间戳是否在同一年
#[no_mangle]
pub extern "C" fn qi_datetime_is_same_year(timestamp1: i64, timestamp2: i64) -> i64 {
    let start1 = qi_datetime_start_of_year(timestamp1);
    let start2 = qi_datetime_start_of_year(timestamp2);
    if start1 == start2 {
        1
    } else {
        0
    }
}

/// 判断时间戳1是否在时间戳2之前
#[no_mangle]
pub extern "C" fn qi_datetime_is_before(timestamp1: i64, timestamp2: i64) -> i64 {
    if timestamp1 < timestamp2 {
        1
    } else {
        0
    }
}

/// 判断时间戳1是否在时间戳2之后
#[no_mangle]
pub extern "C" fn qi_datetime_is_after(timestamp1: i64, timestamp2: i64) -> i64 {
    if timestamp1 > timestamp2 {
        1
    } else {
        0
    }
}

/// 判断是否为工作日（非周末）
#[no_mangle]
pub extern "C" fn qi_datetime_is_business_day(timestamp: i64) -> i64 {
    qi_datetime_is_weekday(timestamp)
}

/// 获取下一个工作日
#[no_mangle]
pub extern "C" fn qi_datetime_next_business_day(timestamp: i64) -> i64 {
    let mut next = timestamp + 86400; // 加一天
    while qi_datetime_is_weekend(next) == 1 {
        next += 86400;
    }
    next
}

/// 获取上一个工作日
#[no_mangle]
pub extern "C" fn qi_datetime_prev_business_day(timestamp: i64) -> i64 {
    let mut prev = timestamp - 86400; // 减一天
    while qi_datetime_is_weekend(prev) == 1 {
        prev -= 86400;
    }
    prev
}

/// 计算两个时间戳之间的周数差
#[no_mangle]
pub extern "C" fn qi_datetime_diff_weeks(timestamp1: i64, timestamp2: i64) -> i64 {
    (timestamp2 - timestamp1) / (7 * 86400)
}

/// 计算两个时间戳之间的月数差（近似值）
#[no_mangle]
pub extern "C" fn qi_datetime_diff_months(timestamp1: i64, timestamp2: i64) -> i64 {
    (timestamp2 - timestamp1) / (30 * 86400)
}

/// 计算两个时间戳之间的年数差（近似值）
#[no_mangle]
pub extern "C" fn qi_datetime_diff_years(timestamp1: i64, timestamp2: i64) -> i64 {
    (timestamp2 - timestamp1) / (365 * 86400)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_datetime_now() {
        let now = qi_datetime_now();
        assert!(now > 0);

        let now_millis = qi_datetime_now_millis();
        assert!(now_millis > 0);
        assert!(now_millis > now * 1000);
    }

    #[test]
    fn test_datetime_format() {
        let timestamp = 1704556800; // 2024-01-06 16:00:00 UTC
        let format = CString::new("%Y-%m-%d").unwrap();
        let result = qi_datetime_format(timestamp, format.as_ptr());
        assert!(!result.is_null());

        let result_str = unsafe { CStr::from_ptr(result).to_str().unwrap() };
        assert_eq!(result_str, "2024-01-06");

        qi_datetime_free_string(result);
    }

    #[test]
    fn test_datetime_components() {
        let timestamp = 1704556800; // 2024-01-06 16:00:00 UTC (Saturday)
        assert_eq!(qi_datetime_year(timestamp), 2024);
        assert_eq!(qi_datetime_month(timestamp), 1);
        assert_eq!(qi_datetime_day(timestamp), 6);
        assert_eq!(qi_datetime_hour(timestamp), 16);
        assert_eq!(qi_datetime_quarter(timestamp), 1);
        assert_eq!(qi_datetime_weekday(timestamp), 6); // Saturday
        assert_eq!(qi_datetime_day_of_year(timestamp), 6);
    }

    #[test]
    fn test_leap_year() {
        assert_eq!(qi_datetime_is_leap_year(2024), 1);
        assert_eq!(qi_datetime_is_leap_year(2023), 0);
        assert_eq!(qi_datetime_is_leap_year(2000), 1);
        assert_eq!(qi_datetime_is_leap_year(1900), 0);
    }

    #[test]
    fn test_time_boundaries() {
        let timestamp = 1704556800; // 2024-01-06 16:00:00 UTC

        // Test start/end of day
        let day_start = qi_datetime_start_of_day(timestamp);
        let day_end = qi_datetime_end_of_day(timestamp);
        assert!(day_start < timestamp);
        assert!(day_end > timestamp);
        assert_eq!(qi_datetime_hour(day_start), 0);
        assert_eq!(qi_datetime_hour(day_end), 23);

        // Test start/end of month
        let month_start = qi_datetime_start_of_month(timestamp);
        let month_end = qi_datetime_end_of_month(timestamp);
        assert_eq!(qi_datetime_day(month_start), 1);
        assert_eq!(qi_datetime_day(month_end), 31);
    }

    #[test]
    fn test_time_comparison() {
        let timestamp = 1704556800; // 2024-01-06 16:00:00 UTC

        // Test is_between
        assert_eq!(
            qi_datetime_is_between(timestamp, timestamp - 100, timestamp + 100),
            1
        );
        assert_eq!(
            qi_datetime_is_between(timestamp, timestamp + 100, timestamp + 200),
            0
        );

        // Test weekend/weekday
        assert_eq!(qi_datetime_is_weekend(timestamp), 1); // Saturday
        assert_eq!(qi_datetime_is_weekday(timestamp), 0); // Saturday
    }

    #[test]
    fn test_time_conversion() {
        assert_eq!(qi_datetime_seconds_to_millis(10), 10000);
        assert_eq!(qi_datetime_millis_to_seconds(10000), 10);
        assert_eq!(qi_datetime_seconds_to_micros(1), 1_000_000);
        assert_eq!(qi_datetime_micros_to_seconds(1_000_000), 1);
    }

    #[test]
    fn test_datetime_add_operations() {
        let timestamp = 1704556800; // 2024-01-06 16:00:00 UTC

        // Test add days
        let tomorrow = qi_datetime_add_days(timestamp, 1);
        assert_eq!(qi_datetime_day(tomorrow), 7);

        // Test add hours
        let next_hour = qi_datetime_add_hours(timestamp, 1);
        assert_eq!(qi_datetime_hour(next_hour), 17);

        // Test add weeks
        let next_week = qi_datetime_add_weeks(timestamp, 1);
        assert_eq!(qi_datetime_diff_days(next_week, timestamp), 7);
    }

    #[test]
    fn test_quarter() {
        assert_eq!(qi_datetime_quarter(qi_datetime_from_ymd(2024, 1, 1)), 1);
        assert_eq!(qi_datetime_quarter(qi_datetime_from_ymd(2024, 4, 1)), 2);
        assert_eq!(qi_datetime_quarter(qi_datetime_from_ymd(2024, 7, 1)), 3);
        assert_eq!(qi_datetime_quarter(qi_datetime_from_ymd(2024, 10, 1)), 4);
    }
}
