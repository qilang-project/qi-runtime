// 数据库模块 FFI
//
// 对外是 16 个 C 函数（14 个老的一字不改 + `后端` / `最后插入id` 两个只读新增），
// 对内按连接串 scheme 分发到 SQLite / PostgreSQL / MySQL 三个后端。
// 设计见 qi-web/docs/多数据库设计.md。
//
// 分文件：本文件只留「连接表 / 事务表 / JSON 窄腰 / FFI 边界」，
// 句柄语义（单连接 vs 连接池）在 database/句柄.rs，三个后端的实现在 database/ 下
// include! 进来（同一个模块命名空间，不是子模块 —— 原因写在 database/后端.rs 开头）。

use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{params_from_iter, Connection, Error as SqlError};
use serde_json::{json, Value as JsonValue};
use std::collections::HashMap;
use std::ffi::CStr;
#[cfg(not(feature = "runtime-rc-string"))]
use std::ffi::CString;
use std::os::raw::c_char;
use std::sync::{Arc, Mutex};

include!("database/后端.rs");

#[cfg(feature = "db-postgres")]
include!("database/后端_pg.rs");

#[cfg(feature = "db-mysql")]
include!("database/后端_mysql.rs");

#[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
include!("database/连接池.rs");

include!("database/句柄.rs");

lazy_static::lazy_static! {
    static ref CONNECTIONS: Mutex<HashMap<i64, Arc<连接句柄>>> =
        Mutex::new(HashMap::new());
    static ref TRANSACTIONS: Mutex<HashMap<i64, i64>> = Mutex::new(HashMap::new());
    static ref NEXT_CONNECTION_ID: Mutex<i64> = Mutex::new(1);
    static ref NEXT_TRANSACTION_ID: Mutex<i64> = Mutex::new(1);
}

fn next_id(counter: &Mutex<i64>) -> i64 {
    let mut value = counter.lock().unwrap();
    let id = *value;
    *value += 1;
    id
}

fn connection(conn_id: i64) -> Option<Arc<连接句柄>> {
    CONNECTIONS.lock().unwrap().get(&conn_id).cloned()
}

fn transaction(tx_id: i64) -> Option<Arc<连接句柄>> {
    let conn_id = *TRANSACTIONS.lock().unwrap().get(&tx_id)?;
    connection(conn_id)
}

fn c_string(value: String) -> *mut c_char {
    #[cfg(feature = "runtime-rc-string")]
    {
        return super::qi_str::rc_cstr_from_string(value);
    }
    #[cfg(not(feature = "runtime-rc-string"))]
    {
        CString::new(value)
            .map(CString::into_raw)
            .unwrap_or(std::ptr::null_mut())
    }
}

fn write_success(rows: usize, last_insert_id: i64) -> JsonValue {
    json!({
        "成功": 1,
        "影响行数": rows as i64,
        "最后插入id": last_insert_id,
        "错误码": 0,
        "错误": ""
    })
}

fn sqlite_error_code(error: &SqlError) -> i32 {
    error
        .sqlite_error()
        .map(|info| info.extended_code)
        .unwrap_or(-1)
}

fn write_failure(message: impl Into<String>, code: i32) -> JsonValue {
    json!({
        "成功": 0,
        "影响行数": 0,
        "最后插入id": 0,
        "错误码": code,
        "错误": message.into()
    })
}

fn write_result_json(result: Result<(usize, i64), 数据库错误>) -> *mut c_char {
    let value = match result {
        Ok((rows, last_insert_id)) => write_success(rows, last_insert_id),
        Err(错误) => write_failure(错误.消息, 错误.错误码),
    };
    c_string(value.to_string())
}

fn parse_params(params_json: &str) -> Result<Vec<参数值>, String> {
    let value: JsonValue =
        serde_json::from_str(params_json).map_err(|error| format!("参数 JSON 无效: {error}"))?;
    let values = value
        .as_array()
        .ok_or_else(|| "参数 JSON 必须是数组".to_string())?;

    values
        .iter()
        .map(|value| match value {
            JsonValue::Null => Ok(参数值::空),
            JsonValue::Bool(value) => Ok(参数值::布尔(*value)),
            JsonValue::Number(value) => {
                if let Some(integer) = value.as_i64() {
                    Ok(参数值::整数(integer))
                } else if let Some(float) = value.as_f64() {
                    Ok(参数值::浮点(float))
                } else {
                    Err("参数数字超出支持范围".to_string())
                }
            }
            JsonValue::String(value) => Ok(参数值::文本(value.clone())),
            JsonValue::Array(_) | JsonValue::Object(_) => {
                Err("参数只支持空值、布尔、整数、浮点和文本".to_string())
            }
        })
        .collect()
}

unsafe fn ffi_text<'a>(value: *const c_char) -> Option<std::borrow::Cow<'a, str>> {
    if value.is_null() {
        return None;
    }
    Some(CStr::from_ptr(value).to_string_lossy())
}

/// 连接数据库。
///
/// 连接串按 scheme 分发：`postgres://` / `postgresql://` → PostgreSQL，
/// `mysql://` → MySQL，`sqlite://` 或**任何无 scheme 的裸路径** → SQLite。
///
/// 网络后端拿到的句柄是**一个连接池**（默认 8 条，见 database/连接池.rs），
/// SQLite 仍是一条连接 —— 差异全部收在 `连接句柄` 里，FFI 签名与语义不变。
#[no_mangle]
pub extern "C" fn qi_db_connect(path: *const c_char) -> i64 {
    let Some(path) = (unsafe { ffi_text(path) }) else {
        return -1;
    };

    match 连接句柄::打开(path.as_ref()) {
        Ok(句柄) => {
            let id = next_id(&NEXT_CONNECTION_ID);
            CONNECTIONS.lock().unwrap().insert(id, Arc::new(句柄));
            id
        }
        Err(_) => -1,
    }
}

/// 执行 SQL（旧 API，保留兼容）
#[no_mangle]
pub extern "C" fn qi_db_execute(conn_id: i64, sql: *const c_char) -> i64 {
    let Some(sql) = (unsafe { ffi_text(sql) }) else {
        return -1;
    };
    let Some(句柄) = connection(conn_id) else {
        return -1;
    };
    句柄
        .执行(sql.as_ref())
        .map(|rows| rows as i64)
        .unwrap_or(-1)
}

/// 参数化执行，参数是 JSON 数组；返回结构化写结果 JSON。
#[no_mangle]
pub extern "C" fn qi_db_execute_params(
    conn_id: i64,
    sql: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    let Some(sql) = (unsafe { ffi_text(sql) }) else {
        return c_string(write_failure("SQL 为空", -1).to_string());
    };
    let Some(params_json) = (unsafe { ffi_text(params_json) }) else {
        return c_string(write_failure("参数 JSON 为空", -1).to_string());
    };
    let params = match parse_params(params_json.as_ref()) {
        Ok(params) => params,
        Err(error) => return c_string(write_failure(error, -1).to_string()),
    };
    let Some(句柄) = connection(conn_id) else {
        return c_string(write_failure("数据库连接无效", -1).to_string());
    };
    write_result_json(句柄.执行参数(sql.as_ref(), &params))
}

/// 查询 SQL（旧 API，保留兼容）
#[no_mangle]
pub extern "C" fn qi_db_query(conn_id: i64, sql: *const c_char) -> *mut c_char {
    let Some(sql) = (unsafe { ffi_text(sql) }) else {
        return std::ptr::null_mut();
    };
    let Some(句柄) = connection(conn_id) else {
        return std::ptr::null_mut();
    };
    句柄
        .查询(sql.as_ref())
        .map(c_string)
        .unwrap_or(std::ptr::null_mut())
}

/// 参数化查询，参数是 JSON 数组；成功返回 JSON 行数组，失败返回空指针。
#[no_mangle]
pub extern "C" fn qi_db_query_params(
    conn_id: i64,
    sql: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    let Some(sql) = (unsafe { ffi_text(sql) }) else {
        return std::ptr::null_mut();
    };
    let Some(params_json) = (unsafe { ffi_text(params_json) }) else {
        return std::ptr::null_mut();
    };
    let Ok(params) = parse_params(params_json.as_ref()) else {
        return std::ptr::null_mut();
    };
    let Some(句柄) = connection(conn_id) else {
        return std::ptr::null_mut();
    };
    句柄
        .查询参数(sql.as_ref(), &params)
        .map(c_string)
        .unwrap_or(std::ptr::null_mut())
}

/// 关闭数据库连接；有活动事务时先回滚并令事务句柄失效。
#[no_mangle]
pub extern "C" fn qi_db_close(conn_id: i64) -> i32 {
    let 句柄 = CONNECTIONS.lock().unwrap().remove(&conn_id);
    let Some(句柄) = 句柄 else {
        return -1;
    };
    let 已废事务 = 句柄.关闭();
    if !已废事务.is_empty() {
        let mut 事务表 = TRANSACTIONS.lock().unwrap();
        for tx_id in 已废事务 {
            事务表.remove(&tx_id);
        }
    }
    0
}

/// 当前连接的后端名："sqlite" / "postgres" / "mysql"。
///
/// 上层（`qi-web/迁移.qi` 的建表助手）靠它挑 DDL 方言 —— 驱动层不抹平 DDL 差异，
/// 这是设计文档 4.3 定的分工。句柄无效时返回空串，不返回空指针，免得调用侧崩。
#[no_mangle]
/// 句柄**既收连接也收事务**。
///
/// 迁移回调（qi-web/迁移.qi）签名是 `函数(事务句柄):整数`，拿不到连接句柄；
/// 而它恰恰是最需要问后端的地方 —— 建表 DDL 三家方言不通用。只认连接句柄的话，
/// 调用方得自己把连接透传进每个迁移回调，那是把驱动层的缺陷推给业务。
/// 事务表本来就是 tx_id → conn_id 的映射，顺着查一次即可。
pub extern "C" fn qi_db_backend(handle: i64) -> *mut c_char {
    let 句柄 = match connection(handle) {
        Some(c) => c,
        // 不是连接句柄，再按事务句柄查它所属的连接
        None => match transaction(handle) {
            Some(c) => c,
            None => return c_string(String::new()),
        },
    };
    c_string(句柄.名称())
}

/// 最后插入的自增主键。三家的取法不同（rowid / lastval() / LAST_INSERT_ID()），
/// 这里统一。句柄无效返回 -1。
///
/// 池化后这个值由句柄记住写语句刚返回的那个 id，而不是现问某条连接 ——
/// 它在三家后端都是会话态，一池多连接时问谁都可能答错。并发写同一句柄时它当然
/// 只是「最近一次」；要确定性就读 `执行参数` 返回 JSON 里的 `最后插入id`，
/// 那是执行那条语句的连接上当场取的。
#[no_mangle]
pub extern "C" fn qi_db_last_insert_id(conn_id: i64) -> i64 {
    let Some(句柄) = connection(conn_id) else {
        return -1;
    };
    句柄.最后插入id()
}

/// 开始连接级事务（旧 API，保留兼容）。
#[no_mangle]
pub extern "C" fn qi_db_begin_transaction(conn_id: i64) -> i32 {
    let Some(句柄) = connection(conn_id) else {
        return -1;
    };
    if 句柄.开始连接级事务().is_err() {
        return -1;
    }
    0
}

/// 提交连接级事务（旧 API，保留兼容）。
#[no_mangle]
pub extern "C" fn qi_db_commit(conn_id: i64) -> i32 {
    let Some(句柄) = connection(conn_id) else {
        return -1;
    };
    if 句柄.结束连接级事务(事务动作::提交).is_err() {
        return -1;
    }
    0
}

/// 回滚连接级事务（旧 API，保留兼容）。
#[no_mangle]
pub extern "C" fn qi_db_rollback(conn_id: i64) -> i32 {
    let Some(句柄) = connection(conn_id) else {
        return -1;
    };
    if 句柄.结束连接级事务(事务动作::回滚).is_err() {
        return -1;
    }
    0
}

/// 开启独占事务，返回正数事务句柄。
#[no_mangle]
pub extern "C" fn qi_db_transaction_open(conn_id: i64) -> i64 {
    let Some(句柄) = connection(conn_id) else {
        return -1;
    };
    // 事务号先分配再开事务：池后端要用它当独占连接的键。失败只是浪费一个号。
    let tx_id = next_id(&NEXT_TRANSACTION_ID);
    if 句柄.开启事务(tx_id).is_err() {
        return -1;
    }
    TRANSACTIONS.lock().unwrap().insert(tx_id, conn_id);
    tx_id
}

/// 在独占事务中参数化执行。
#[no_mangle]
pub extern "C" fn qi_db_transaction_execute_params(
    tx_id: i64,
    sql: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    let Some(sql) = (unsafe { ffi_text(sql) }) else {
        return c_string(write_failure("SQL 为空", -1).to_string());
    };
    let Some(params_json) = (unsafe { ffi_text(params_json) }) else {
        return c_string(write_failure("参数 JSON 为空", -1).to_string());
    };
    let params = match parse_params(params_json.as_ref()) {
        Ok(params) => params,
        Err(error) => return c_string(write_failure(error, -1).to_string()),
    };
    let Some(句柄) = transaction(tx_id) else {
        return c_string(write_failure("事务句柄无效", -1).to_string());
    };
    write_result_json(句柄.事务执行参数(tx_id, sql.as_ref(), &params))
}

/// 在独占事务中参数化查询。
#[no_mangle]
pub extern "C" fn qi_db_transaction_query_params(
    tx_id: i64,
    sql: *const c_char,
    params_json: *const c_char,
) -> *mut c_char {
    let Some(sql) = (unsafe { ffi_text(sql) }) else {
        return std::ptr::null_mut();
    };
    let Some(params_json) = (unsafe { ffi_text(params_json) }) else {
        return std::ptr::null_mut();
    };
    let Ok(params) = parse_params(params_json.as_ref()) else {
        return std::ptr::null_mut();
    };
    let Some(句柄) = transaction(tx_id) else {
        return std::ptr::null_mut();
    };
    句柄
        .事务查询参数(tx_id, sql.as_ref(), &params)
        .map(c_string)
        .unwrap_or(std::ptr::null_mut())
}

fn finish_transaction(tx_id: i64, 动作: 事务动作) -> i32 {
    let Some(句柄) = transaction(tx_id) else {
        return -1;
    };
    if 句柄.结束事务(tx_id, 动作).is_err() {
        return -1;
    }
    TRANSACTIONS.lock().unwrap().remove(&tx_id);
    0
}

#[no_mangle]
pub extern "C" fn qi_db_transaction_commit(tx_id: i64) -> i32 {
    finish_transaction(tx_id, 事务动作::提交)
}

#[no_mangle]
pub extern "C" fn qi_db_transaction_rollback(tx_id: i64) -> i32 {
    finish_transaction(tx_id, 事务动作::回滚)
}

/// 释放字符串
#[no_mangle]
pub extern "C" fn qi_db_free_string(value: *mut c_char) {
    #[cfg(feature = "runtime-rc-string")]
    {
        super::qi_str::rc_cstr_release(value);
    }
    #[cfg(not(feature = "runtime-rc-string"))]
    if !value.is_null() {
        unsafe {
            let _ = CString::from_raw(value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn text(value: &str) -> CString {
        CString::new(value).unwrap()
    }

    unsafe fn take_string(value: *mut c_char) -> String {
        assert!(!value.is_null());
        let result = CStr::from_ptr(value).to_string_lossy().into_owned();
        qi_db_free_string(value);
        result
    }

    #[test]
    fn database_operations_remain_compatible() {
        let path = text(":memory:");
        let conn_id = qi_db_connect(path.as_ptr());
        assert!(conn_id > 0);
        assert_eq!(
            qi_db_execute(
                conn_id,
                text("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")
                    .as_ptr()
            ),
            0
        );
        assert_eq!(
            qi_db_execute(
                conn_id,
                text("INSERT INTO users (name, age) VALUES ('Alice', 30)").as_ptr()
            ),
            1
        );
        let result = qi_db_query(conn_id, text("SELECT * FROM users").as_ptr());
        assert!(unsafe { take_string(result) }.contains("Alice"));
        assert_eq!(qi_db_close(conn_id), 0);
    }

    #[test]
    fn parameterized_queries_and_write_results_work() {
        let conn_id = qi_db_connect(text(":memory:").as_ptr());
        qi_db_execute(
            conn_id,
            text("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, active INTEGER)").as_ptr(),
        );
        let write = qi_db_execute_params(
            conn_id,
            text("INSERT INTO users (name, active) VALUES (?, ?)").as_ptr(),
            text(r#"["O'Reilly",true]"#).as_ptr(),
        );
        let write_json: JsonValue = serde_json::from_str(&unsafe { take_string(write) }).unwrap();
        assert_eq!(write_json["成功"], 1);
        assert_eq!(write_json["影响行数"], 1);
        assert_eq!(write_json["最后插入id"], 1);

        let rows = qi_db_query_params(
            conn_id,
            text("SELECT name, active FROM users WHERE name = ?").as_ptr(),
            text(r#"["O'Reilly"]"#).as_ptr(),
        );
        let rows_json: JsonValue = serde_json::from_str(&unsafe { take_string(rows) }).unwrap();
        assert_eq!(rows_json[0]["name"], "O'Reilly");
        assert_eq!(rows_json[0]["active"], 1);
        qi_db_close(conn_id);
    }

    #[test]
    fn transaction_handles_are_isolated_and_rollback() {
        let conn_id = qi_db_connect(text(":memory:").as_ptr());
        qi_db_execute(
            conn_id,
            text("CREATE TABLE items (id INTEGER PRIMARY KEY, quantity INTEGER)").as_ptr(),
        );
        qi_db_execute(
            conn_id,
            text("INSERT INTO items (quantity) VALUES (3)").as_ptr(),
        );

        let tx_id = qi_db_transaction_open(conn_id);
        assert!(tx_id > 0);
        assert_eq!(
            qi_db_execute(conn_id, text("UPDATE items SET quantity = 99").as_ptr()),
            -1
        );
        let result = qi_db_transaction_execute_params(
            tx_id,
            text("UPDATE items SET quantity = quantity - ? WHERE id = ? AND quantity >= ?")
                .as_ptr(),
            text("[2,1,2]").as_ptr(),
        );
        let result_json: JsonValue = serde_json::from_str(&unsafe { take_string(result) }).unwrap();
        assert_eq!(result_json["影响行数"], 1);
        assert_eq!(qi_db_transaction_rollback(tx_id), 0);

        let rows = qi_db_query_params(
            conn_id,
            text("SELECT quantity FROM items WHERE id = ?").as_ptr(),
            text("[1]").as_ptr(),
        );
        let rows_json: JsonValue = serde_json::from_str(&unsafe { take_string(rows) }).unwrap();
        assert_eq!(rows_json[0]["quantity"], 3);
        assert_eq!(qi_db_transaction_commit(tx_id), -1);
        qi_db_close(conn_id);
    }

    /// 裸路径不带 scheme —— 12+ 个现有应用都这么写，必须仍然落到 SQLite。
    #[test]
    fn 裸路径与sqlite前缀都落到sqlite后端() {
        let conn_id = qi_db_connect(text(":memory:").as_ptr());
        assert!(conn_id > 0);
        assert_eq!(unsafe { take_string(qi_db_backend(conn_id)) }, "sqlite");
        qi_db_close(conn_id);

        let 临时 = std::env::temp_dir().join("qi_db_scheme_测试.db");
        let _ = std::fs::remove_file(&临时);
        let 连接串 = format!("sqlite://{}", 临时.display());
        let conn_id = qi_db_connect(text(&连接串).as_ptr());
        assert!(conn_id > 0);
        assert_eq!(unsafe { take_string(qi_db_backend(conn_id)) }, "sqlite");
        assert_eq!(
            qi_db_execute(conn_id, text("CREATE TABLE t (id INTEGER)").as_ptr()),
            0
        );
        qi_db_close(conn_id);
        assert!(临时.exists(), "sqlite:// 前缀应当去掉后当成文件路径");
        let _ = std::fs::remove_file(&临时);
    }

    #[test]
    fn 最后插入id与后端名对无效句柄不崩() {
        assert_eq!(qi_db_last_insert_id(-99), -1);
        assert_eq!(unsafe { take_string(qi_db_backend(-99)) }, "");
    }

    #[test]
    fn 最后插入id跟随插入递增() {
        let conn_id = qi_db_connect(text(":memory:").as_ptr());
        qi_db_execute(
            conn_id,
            text("CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)").as_ptr(),
        );
        for 期望 in 1..=3 {
            let 写 = qi_db_execute_params(
                conn_id,
                text("INSERT INTO t (v) VALUES (?)").as_ptr(),
                text(r#"["x"]"#).as_ptr(),
            );
            let _ = unsafe { take_string(写) };
            assert_eq!(qi_db_last_insert_id(conn_id), 期望);
        }
        qi_db_close(conn_id);
    }

    /// 未编入的后端要给出**明确报错**（连接失败），不能悄悄退回 SQLite 去建个文件。
    #[test]
    fn 未知scheme不会被误当成文件路径() {
        // db-postgres / db-mysql 默认开着，这里连的是不存在的地址，必然失败；
        // 关键是它没有在当前目录建出一个名叫 "postgres://…" 的 SQLite 文件。
        let 连接串 = "postgres://qi:qi@127.0.0.1:1/不存在的库";
        let conn_id = qi_db_connect(text(连接串).as_ptr());
        assert_eq!(conn_id, -1);
        assert!(!std::path::Path::new(连接串).exists());
    }

    /// 池参数必须在发给驱动**之前**摘掉：postgres / mysql 两个 crate 见到不认识的
    /// 查询参数会直接拒连，漏一个就是「配了池参数反而连不上库」。
    #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
    #[test]
    fn 池参数从连接串里摘干净且不误伤驱动参数() {
        let (干净, 配置) =
            拆池参数("postgres://qi:pw@h/db?pool_max=3&sslmode=disable&pool_timeout_ms=1200");
        assert_eq!(干净, "postgres://qi:pw@h/db?sslmode=disable");
        assert_eq!(配置.最大连接数, 3);
        assert_eq!(配置.获取超时, std::time::Duration::from_millis(1200));

        // 只有池参数时，问号也要一起去掉 —— 留个空查询串照样会让驱动犯难
        let (干净, _) = 拆池参数("mysql://qi:pw@h/db?pool_max=2");
        assert_eq!(干净, "mysql://qi:pw@h/db");

        // 没有查询串就原样透传（12+ 个应用的连接串长这样）
        let (干净, 配置) = 拆池参数("postgres://qi:pw@h/db");
        assert_eq!(干净, "postgres://qi:pw@h/db");
        assert_eq!(配置.最大连接数, 8);

        // 写坏的池参数退回默认值：不该因为多打了个字母就整个应用连不上库
        let (_, 配置) = 拆池参数("postgres://h/db?pool_max=八条");
        assert_eq!(配置.最大连接数, 8);
    }

    /// 只有网络后端进池。裸路径 / sqlite:// 必须留在单连接那条路上。
    #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
    #[test]
    fn 只有网络连接串才进池() {
        assert!(是网络连接串("postgres://h/db"));
        assert!(是网络连接串("POSTGRESQL://h/db"));
        assert!(是网络连接串("mysql://h/db"));
        assert!(!是网络连接串("绘本.db"));
        assert!(!是网络连接串(":memory:"));
        assert!(!是网络连接串("sqlite:///tmp/a.db"));
    }
}
