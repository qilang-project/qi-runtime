// MySQL 后端（同步 `mysql` crate）。
//
// 整份文件被 database_ffi.rs 里 `#[cfg(feature = "db-mysql")] include!` 进来，
// 所以内部不再逐项加 cfg。占位符与 SQLite 同为 `?`，不需要方言改写。

use mysql::consts::ColumnType;
use mysql::prelude::Queryable;
use mysql::{Conn, Opts, Params, Row as MySQLRow, Value as MySQLValue};

/// `最后插入` 自己缓存，不直接透传 `Conn::last_insert_id()`。
///
/// 后者读的是**最近一个 OK 包**里的值：紧跟一条 SELECT 或 COMMIT 之后就归 0，
/// 而 SQLite 的 `last_insert_rowid()` 和 PG 的 `lastval()` 都是会话内持续有效的。
/// 三个后端语义要一致，所以只在拿到非 0 时更新。
struct MySQL连接 {
    连接: Conn,
    最后插入: i64,
}

fn 转mysql错误(错误: mysql::Error) -> 数据库错误 {
    数据库错误::新(错误.to_string())
}

impl MySQL连接 {
    fn 打开(连接串: &str) -> Result<MySQL连接, 数据库错误> {
        let 选项 = Opts::from_url(连接串).map_err(|错误| 数据库错误::新(错误.to_string()))?;
        Ok(MySQL连接 {
            连接: Conn::new(选项).map_err(转mysql错误)?,
            最后插入: 0,
        })
    }

    fn 执行(&mut self, sql: &str) -> Result<usize, 数据库错误> {
        // 文本协议：DDL 和多语句都能过，返回值只要影响行数。
        self.连接.query_drop(sql).map_err(转mysql错误)?;
        self.记住插入id();
        Ok(self.连接.affected_rows() as usize)
    }

    fn 执行参数(
        &mut self,
        sql: &str,
        参数: &[参数值],
    ) -> Result<(usize, i64), 数据库错误> {
        self.连接
            .exec_drop(sql, 转mysql参数(参数))
            .map_err(转mysql错误)?;
        let 行数 = self.连接.affected_rows() as usize;
        self.记住插入id();
        Ok((行数, self.最后插入))
    }

    fn 记住插入id(&mut self) {
        let 新值 = self.连接.last_insert_id() as i64;
        if 新值 != 0 {
            self.最后插入 = 新值;
        }
    }

    fn 查询(&mut self, sql: &str, 参数: &[参数值]) -> Result<String, 数据库错误> {
        // 一律走预处理（二进制协议）：文本协议把整数也当字节串返回，
        // JSON 里就会变成 "3" 而不是 3，跟 SQLite/PG 的结果对不上。
        let 行集: Vec<MySQLRow> = self
            .连接
            .exec(sql, 转mysql参数(参数))
            .map_err(转mysql错误)?;
        let 结果: Vec<JsonValue> = 行集.iter().map(mysql行转json).collect();
        Ok(JsonValue::Array(结果).to_string())
    }

    fn 事务(&mut self, 动作: 事务动作) -> Result<(), 数据库错误> {
        let sql = match 动作 {
            事务动作::开始 | 事务动作::独占开始 => "START TRANSACTION",
            事务动作::提交 => "COMMIT",
            事务动作::回滚 => "ROLLBACK",
        };
        self.连接.query_drop(sql).map_err(转mysql错误)
    }

    fn 最后插入id(&mut self) -> i64 {
        self.最后插入
    }

    /// COM_PING 是最便宜的往返（不解析 SQL、不走查询路径），服务端重启后
    /// 这里会立刻报错，连接池据此丢弃僵尸连接。
    fn 探活(&mut self) -> Result<(), 数据库错误> {
        self.连接.ping().map_err(转mysql错误)
    }
}

fn 转mysql参数(参数: &[参数值]) -> Params {
    if 参数.is_empty() {
        return Params::Empty;
    }
    Params::Positional(
        参数
            .iter()
            .map(|值| match 值 {
                参数值::空 => MySQLValue::NULL,
                // MySQL 没有真正的布尔，TINYINT(1) 就是 0/1，与 SQLite 一致。
                参数值::布尔(值) => MySQLValue::Int(i64::from(*值)),
                参数值::整数(值) => MySQLValue::Int(*值),
                参数值::浮点(值) => MySQLValue::Double(*值),
                参数值::文本(值) => MySQLValue::Bytes(值.as_bytes().to_vec()),
            })
            .collect(),
    )
}

fn mysql行转json(行: &MySQLRow) -> JsonValue {
    let mut 对象 = serde_json::Map::new();
    let 列表: Vec<(String, ColumnType)> = 行
        .columns_ref()
        .iter()
        .map(|列| (列.name_str().to_string(), 列.column_type()))
        .collect();
    for (序号, (列名, 列类型)) in 列表.iter().enumerate() {
        let 值 = 行
            .as_ref(序号)
            .map_or(JsonValue::Null, |值| 解码mysql值(*列类型, 值));
        对象.insert(列名.clone(), 值);
    }
    JsonValue::Object(对象)
}

fn 解码mysql值(列类型: ColumnType, 值: &MySQLValue) -> JsonValue {
    match 值 {
        MySQLValue::NULL => JsonValue::Null,
        MySQLValue::Int(值) => json!(值),
        MySQLValue::UInt(值) => json!(值),
        MySQLValue::Float(值) => json!(f64::from(*值)),
        MySQLValue::Double(值) => json!(值),
        // 年月日时分秒微秒 → 有时分秒才带出来，纯 DATE 列不该拖个 00:00:00 尾巴。
        MySQLValue::Date(年, 月, 日, 时, 分, 秒, 微秒) => {
            if *时 == 0
                && *分 == 0
                && *秒 == 0
                && *微秒 == 0
                && 列类型 == ColumnType::MYSQL_TYPE_DATE
            {
                json!(format!("{年:04}-{月:02}-{日:02}"))
            } else if *微秒 == 0 {
                json!(format!("{年:04}-{月:02}-{日:02} {时:02}:{分:02}:{秒:02}"))
            } else {
                json!(format!(
                    "{年:04}-{月:02}-{日:02} {时:02}:{分:02}:{秒:02}.{微秒:06}"
                ))
            }
        }
        MySQLValue::Time(负, 天, 时, 分, 秒, 微秒) => {
            let 小时 = u32::from(*时) + 天 * 24;
            let 符号 = if *负 { "-" } else { "" };
            if *微秒 == 0 {
                json!(format!("{符号}{小时:02}:{分:02}:{秒:02}"))
            } else {
                json!(format!("{符号}{小时:02}:{分:02}:{秒:02}.{微秒:06}"))
            }
        }
        MySQLValue::Bytes(字节) => 解码mysql字节(列类型, 字节),
    }
}

fn 解码mysql字节(列类型: ColumnType, 字节: &[u8]) -> JsonValue {
    // DECIMAL 走的就是这条：MySQL 以文本回传，直接当字符串留住精度（设计文档 4.2）。
    if 列类型 == ColumnType::MYSQL_TYPE_JSON {
        return std::str::from_utf8(字节)
            .ok()
            .and_then(|文本| serde_json::from_str(文本).ok())
            .unwrap_or(JsonValue::Null);
    }
    match std::str::from_utf8(字节) {
        Ok(文本) => JsonValue::String(文本.to_string()),
        // 真二进制列（BLOB / VARBINARY）转 base64，别丢数据也别塞坏 UTF-8。
        Err(_) => JsonValue::String(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            字节,
        )),
    }
}
