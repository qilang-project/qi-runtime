//! 数据库模块 FFI
//!
//! 提供 SQLite 数据库操作功能

use rusqlite::Connection;
use std::collections::HashMap;
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::Mutex;

lazy_static::lazy_static! {
    static ref CONNECTIONS: Mutex<HashMap<i64, Connection>> = Mutex::new(HashMap::new());
    static ref NEXT_ID: Mutex<i64> = Mutex::new(1);
}

/// 连接数据库
#[no_mangle]
pub extern "C" fn qi_db_connect(path: *const c_char) -> i64 {
    if path.is_null() {
        return -1;
    }

    unsafe {
        let path_str = CStr::from_ptr(path).to_string_lossy();

        match Connection::open(path_str.as_ref()) {
            Ok(conn) => {
                let mut next_id = NEXT_ID.lock().unwrap();
                let id = *next_id;
                *next_id += 1;

                let mut connections = CONNECTIONS.lock().unwrap();
                connections.insert(id, conn);

                id
            }
            Err(_) => -1,
        }
    }
}

/// 执行 SQL（INSERT, UPDATE, DELETE）
#[no_mangle]
pub extern "C" fn qi_db_execute(conn_id: i64, sql: *const c_char) -> i64 {
    if sql.is_null() {
        return -1;
    }

    unsafe {
        let sql_str = CStr::from_ptr(sql).to_string_lossy();

        let connections = CONNECTIONS.lock().unwrap();
        if let Some(conn) = connections.get(&conn_id) {
            match conn.execute(&sql_str, []) {
                Ok(rows) => rows as i64,
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// 查询 SQL（SELECT，返回 JSON 数组）
#[no_mangle]
pub extern "C" fn qi_db_query(conn_id: i64, sql: *const c_char) -> *mut c_char {
    if sql.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let sql_str = CStr::from_ptr(sql).to_string_lossy();

        let connections = CONNECTIONS.lock().unwrap();
        if let Some(conn) = connections.get(&conn_id) {
            match conn.prepare(&sql_str) {
                Ok(mut stmt) => {
                    let column_count = stmt.column_count();
                    let column_names: Vec<String> = (0..column_count)
                        .map(|i| stmt.column_name(i).unwrap_or("").to_string())
                        .collect();

                    match stmt.query([]) {
                        Ok(mut rows) => {
                            let mut results = Vec::new();

                            while let Ok(Some(row)) = rows.next() {
                                let mut row_map = serde_json::Map::new();

                                for (i, col_name) in column_names.iter().enumerate() {
                                    // 尝试读取不同类型的值
                                    let value: serde_json::Value =
                                        if let Ok(v) = row.get::<_, i64>(i) {
                                            serde_json::json!(v)
                                        } else if let Ok(v) = row.get::<_, f64>(i) {
                                            serde_json::json!(v)
                                        } else if let Ok(v) = row.get::<_, String>(i) {
                                            serde_json::json!(v)
                                        } else {
                                            serde_json::Value::Null
                                        };

                                    row_map.insert(col_name.clone(), value);
                                }

                                results.push(serde_json::Value::Object(row_map));
                            }

                            match serde_json::to_string(&results) {
                                Ok(json) => match CString::new(json) {
                                    Ok(c_str) => return c_str.into_raw(),
                                    Err(_) => return std::ptr::null_mut(),
                                },
                                Err(_) => return std::ptr::null_mut(),
                            }
                        }
                        Err(_) => return std::ptr::null_mut(),
                    }
                }
                Err(_) => return std::ptr::null_mut(),
            }
        } else {
            std::ptr::null_mut()
        }
    }
}

/// 关闭数据库连接
#[no_mangle]
pub extern "C" fn qi_db_close(conn_id: i64) -> i32 {
    let mut connections = CONNECTIONS.lock().unwrap();
    if connections.remove(&conn_id).is_some() {
        0
    } else {
        -1
    }
}

/// 开始事务
#[no_mangle]
pub extern "C" fn qi_db_begin_transaction(conn_id: i64) -> i32 {
    let connections = CONNECTIONS.lock().unwrap();
    if let Some(conn) = connections.get(&conn_id) {
        match conn.execute("BEGIN TRANSACTION", []) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    } else {
        -1
    }
}

/// 提交事务
#[no_mangle]
pub extern "C" fn qi_db_commit(conn_id: i64) -> i32 {
    let connections = CONNECTIONS.lock().unwrap();
    if let Some(conn) = connections.get(&conn_id) {
        match conn.execute("COMMIT", []) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    } else {
        -1
    }
}

/// 回滚事务
#[no_mangle]
pub extern "C" fn qi_db_rollback(conn_id: i64) -> i32 {
    let connections = CONNECTIONS.lock().unwrap();
    if let Some(conn) = connections.get(&conn_id) {
        match conn.execute("ROLLBACK", []) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    } else {
        -1
    }
}

/// 释放字符串
#[no_mangle]
pub extern "C" fn qi_db_free_string(s: *mut c_char) {
    if !s.is_null() {
        unsafe {
            let _ = CString::from_raw(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_database_operations() {
        let path = CString::new(":memory:").unwrap();
        let conn_id = qi_db_connect(path.as_ptr());
        assert!(conn_id > 0);

        // 创建表
        let create_sql =
            CString::new("CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, age INTEGER)")
                .unwrap();
        assert_eq!(qi_db_execute(conn_id, create_sql.as_ptr()), 0);

        // 插入数据
        let insert_sql =
            CString::new("INSERT INTO users (name, age) VALUES ('Alice', 30)").unwrap();
        assert_eq!(qi_db_execute(conn_id, insert_sql.as_ptr()), 1);

        // 查询数据
        let query_sql = CString::new("SELECT * FROM users").unwrap();
        let result = qi_db_query(conn_id, query_sql.as_ptr());
        assert!(!result.is_null());

        unsafe {
            let result_str = CStr::from_ptr(result).to_string_lossy();
            assert!(result_str.contains("Alice"));
            qi_db_free_string(result);
        }

        // 关闭连接
        assert_eq!(qi_db_close(conn_id), 0);
    }
}
