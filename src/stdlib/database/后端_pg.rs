// PostgreSQL 后端（同步 `postgres` crate）。
//
// 整份文件被 database_ffi.rs 里 `#[cfg(feature = "db-postgres")] include!` 进来，
// 所以内部不再逐项加 cfg。

use bytes::BytesMut;
use postgres::types::{Format, FromSql, IsNull, ToSql, Type};
use postgres::{Client, NoTls, Row};

/// 一条 PG 连接。
///
/// `事务中` 自己记而不是问 ConnectionState：取 `lastval()` 时要不要套 SAVEPOINT
/// 只跟连接自身的事务状态有关，让后端自洽比跨层传参可靠。
struct PG连接 {
    客户端: Client,
    事务中: bool,
    最后插入: i64,
}

fn 转pg错误(错误: postgres::Error) -> 数据库错误 {
    数据库错误::新(错误.to_string())
}

impl PG连接 {
    fn 打开(连接串: &str) -> Result<PG连接, 数据库错误> {
        // NoTls：S1 只解决「能连」。要 TLS 时走 qi-runtime 的 tls_client（设计文档第七节）。
        let 客户端 = Client::connect(连接串, NoTls).map_err(转pg错误)?;
        Ok(PG连接 {
            客户端,
            事务中: false,
            最后插入: 0,
        })
    }

    fn 执行(&mut self, sql: &str) -> Result<usize, 数据库错误> {
        match self.客户端.execute(sql, &[]) {
            Ok(行数) => {
                if 是插入语句(sql) {
                    self.最后插入 = self.读lastval();
                }
                Ok(行数 as usize)
            }
            Err(错误) => {
                // PG 的扩展查询协议一次只收一条语句，而建表脚本常是分号连写的多条。
                // 这种情况 prepare 阶段就失败了、一条都没执行，重试 batch_execute 是安全的。
                if 错误.to_string().contains("cannot insert multiple commands") {
                    self.客户端.batch_execute(sql).map_err(转pg错误)?;
                    Ok(0)
                } else {
                    Err(转pg错误(错误))
                }
            }
        }
    }

    fn 执行参数(
        &mut self,
        sql: &str,
        参数: &[参数值],
    ) -> Result<(usize, i64), 数据库错误> {
        let 语句 = 改写pg占位符(sql, 参数.len())?;
        let 绑定: Vec<PG参数<'_>> = 参数.iter().map(PG参数).collect();
        let 引用: Vec<&(dyn ToSql + Sync)> =
            绑定.iter().map(|值| 值 as &(dyn ToSql + Sync)).collect();
        let 行数 = self
            .客户端
            .execute(语句.as_str(), &引用)
            .map_err(转pg错误)? as usize;
        // 只在 INSERT 后取，别的语句取了也只是上一次的陈值，还白费一次往返。
        if 是插入语句(&语句) {
            self.最后插入 = self.读lastval();
        }
        Ok((行数, self.最后插入))
    }

    fn 查询(&mut self, sql: &str, 参数: &[参数值]) -> Result<String, 数据库错误> {
        let 语句 = 改写pg占位符(sql, 参数.len())?;
        let 绑定: Vec<PG参数<'_>> = 参数.iter().map(PG参数).collect();
        let 引用: Vec<&(dyn ToSql + Sync)> =
            绑定.iter().map(|值| 值 as &(dyn ToSql + Sync)).collect();
        let 行集 = self.客户端.query(语句.as_str(), &引用).map_err(转pg错误)?;
        let 结果: Vec<JsonValue> = 行集.iter().map(pg行转json).collect();
        Ok(JsonValue::Array(结果).to_string())
    }

    fn 事务(&mut self, 动作: 事务动作) -> Result<(), 数据库错误> {
        // PG 没有 SQLite 的 BEGIN IMMEDIATE —— 它靠行锁，不需要提前占写锁。
        let sql = match 动作 {
            事务动作::开始 | 事务动作::独占开始 => "BEGIN",
            事务动作::提交 => "COMMIT",
            事务动作::回滚 => "ROLLBACK",
        };
        self.客户端.batch_execute(sql).map_err(转pg错误)?;
        self.事务中 = matches!(动作, 事务动作::开始 | 事务动作::独占开始);
        Ok(())
    }

    fn 最后插入id(&mut self) -> i64 {
        self.最后插入
    }

    /// 空的 simple_query 就是一次完整往返，但不产生任何服务端工作 ——
    /// rust-postgres 官方的池适配器（r2d2_postgres）也是这么探活的。
    fn 探活(&mut self) -> Result<(), 数据库错误> {
        self.客户端.simple_query("").map(|_| ()).map_err(转pg错误)
    }

    fn 已断开(&mut self) -> bool {
        self.客户端.is_closed()
    }

    /// PG 没有 `last_insert_rowid()`。`lastval()` 返回本会话最近一次 `nextval()`
    /// 的结果，插入自增主键后正是新 id —— 语义与 MySQL 的 `LAST_INSERT_ID()` 一致。
    ///
    /// 坑：本会话还没用过序列时 `lastval()` 会报错，而 PG 里事务中的任何错误都会把
    /// 整个事务打成 aborted。所以事务内必须用 SAVEPOINT 兜住这次试探。
    fn 读lastval(&mut self) -> i64 {
        if !self.事务中 {
            return self
                .客户端
                .query_one("SELECT lastval()", &[])
                .ok()
                .and_then(|行| 行.try_get::<_, i64>(0).ok())
                .unwrap_or(0);
        }
        if self.客户端.batch_execute("SAVEPOINT _qi_lastval").is_err() {
            return 0;
        }
        match self.客户端.query_one("SELECT lastval()", &[]) {
            Ok(行) => {
                let _ = self.客户端.batch_execute("RELEASE SAVEPOINT _qi_lastval");
                行.try_get::<_, i64>(0).unwrap_or(0)
            }
            Err(_) => {
                let _ = self
                    .客户端
                    .batch_execute("ROLLBACK TO SAVEPOINT _qi_lastval");
                0
            }
        }
    }
}

fn 是插入语句(sql: &str) -> bool {
    sql.trim_start()
        .get(..6)
        .is_some_and(|头| 头.eq_ignore_ascii_case("insert"))
}

/// `?` → `$n`。没有参数就没有占位符可改：此时 SQL 里的 `?` 只可能是 jsonb 运算符，
/// 一律不动，免得改坏（设计文档八节风险表：改写宁可不猜）。
fn 改写pg占位符(sql: &str, 参数个数: usize) -> Result<String, 数据库错误> {
    if 参数个数 == 0 {
        return Ok(sql.to_string());
    }
    super::sql_dialect::改写为PG占位符(sql).map_err(数据库错误::新)
}

// ── 参数绑定 ────────────────────────────────────────────────────────────────

/// 参数一律以**文本格式**发出去。
///
/// 二进制格式要求 Rust 类型和列类型严格对上（i64 绑到 int4 列会被 rust-postgres
/// 直接拒掉），而我们的窄腰只有 5 种类型、根本不知道列是 int2 还是 numeric 还是
/// timestamptz。文本格式把解析交给 PG 自己 —— 这也正是 libpq/PDO 的做法。
#[derive(Debug)]
struct PG参数<'a>(&'a 参数值);

impl ToSql for PG参数<'_> {
    fn to_sql(
        &self,
        _类型: &Type,
        输出: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        let 文本 = match self.0 {
            参数值::空 => return Ok(IsNull::Yes),
            参数值::布尔(值) => (if *值 { "true" } else { "false" }).to_string(),
            参数值::整数(值) => 值.to_string(),
            参数值::浮点(值) => 值.to_string(),
            参数值::文本(值) => 值.clone(),
        };
        输出.extend_from_slice(文本.as_bytes());
        Ok(IsNull::No)
    }

    fn accepts(_类型: &Type) -> bool {
        true
    }

    fn to_sql_checked(
        &self,
        类型: &Type,
        输出: &mut BytesMut,
    ) -> Result<IsNull, Box<dyn std::error::Error + Sync + Send>> {
        self.to_sql(类型, 输出)
    }

    fn encode_format(&self, _类型: &Type) -> Format {
        Format::Text
    }
}

// ── 结果解码 ────────────────────────────────────────────────────────────────

/// 拿列的原始字节。rust-postgres 固定用二进制结果格式，而我们要按设计文档 4.2 节
/// 自己决定降级方式（尤其 numeric 必须转字符串），所以绕过它的 FromSql 实现，
/// 只把裸字节要出来。
struct PG原始值(Option<Vec<u8>>);

impl FromSql<'_> for PG原始值 {
    fn from_sql(
        _类型: &Type,
        原始: &[u8],
    ) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(PG原始值(Some(原始.to_vec())))
    }

    fn from_sql_null(_类型: &Type) -> Result<Self, Box<dyn std::error::Error + Sync + Send>> {
        Ok(PG原始值(None))
    }

    fn accepts(_类型: &Type) -> bool {
        true
    }
}

fn pg行转json(行: &Row) -> JsonValue {
    let mut 对象 = serde_json::Map::new();
    for (序号, 列) in 行.columns().iter().enumerate() {
        let 值 = match 行.try_get::<_, PG原始值>(序号) {
            Ok(PG原始值(Some(原始))) => 解码pg值(列.type_(), &原始),
            _ => JsonValue::Null,
        };
        对象.insert(列.name().to_string(), 值);
    }
    JsonValue::Object(对象)
}

fn 取i16(原始: &[u8]) -> i64 {
    let mut 字节 = [0u8; 2];
    let 长度 = 2.min(原始.len());
    字节[..长度].copy_from_slice(&原始[..长度]);
    i64::from(i16::from_be_bytes(字节))
}

fn 取i32(原始: &[u8]) -> i64 {
    let mut 字节 = [0u8; 4];
    字节[..4.min(原始.len())].copy_from_slice(&原始[..4.min(原始.len())]);
    i64::from(i32::from_be_bytes(字节))
}

fn 取i64(原始: &[u8]) -> i64 {
    let mut 字节 = [0u8; 8];
    字节[..8.min(原始.len())].copy_from_slice(&原始[..8.min(原始.len())]);
    i64::from_be_bytes(字节)
}

fn 宽松文本(原始: &[u8]) -> JsonValue {
    // 设计文档 4.2：其余一切转字符串，转不动则 null。
    match std::str::from_utf8(原始) {
        Ok(文本) => JsonValue::String(文本.to_string()),
        Err(_) => JsonValue::Null,
    }
}

fn 解码pg值(类型: &Type, 原始: &[u8]) -> JsonValue {
    if *类型 == Type::BOOL {
        return json!(原始.first().copied().unwrap_or(0) != 0);
    }
    if *类型 == Type::INT2 {
        return json!(取i16(原始));
    }
    if *类型 == Type::INT4 || *类型 == Type::OID {
        return json!(取i32(原始));
    }
    if *类型 == Type::INT8 {
        return json!(取i64(原始));
    }
    if *类型 == Type::FLOAT4 {
        return json!(f64::from(f32::from_bits(取i32(原始) as u32)));
    }
    if *类型 == Type::FLOAT8 {
        return json!(f64::from_bits(取i64(原始) as u64));
    }
    if *类型 == Type::NUMERIC {
        // 金额靠它。JSON number 是 f64，会丢精度 —— 必须字符串。
        return 解码numeric(原始).map_or(JsonValue::Null, JsonValue::String);
    }
    if *类型 == Type::TEXT
        || *类型 == Type::VARCHAR
        || *类型 == Type::BPCHAR
        || *类型 == Type::NAME
        || *类型 == Type::CHAR
        || *类型 == Type::UNKNOWN
    {
        return 宽松文本(原始);
    }
    if *类型 == Type::UUID {
        return 解码uuid(原始);
    }
    if *类型 == Type::JSON {
        return 解码json(原始);
    }
    if *类型 == Type::JSONB {
        // jsonb 二进制首字节是格式版本号（目前恒为 1），后面才是 JSON 文本。
        return 原始
            .split_first()
            .map_or(JsonValue::Null, |(_, 正文)| 解码json(正文));
    }
    if *类型 == Type::BYTEA {
        return JsonValue::String(base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            原始,
        ));
    }
    if *类型 == Type::DATE {
        return 纪元日期(取i32(原始)).map_or(JsonValue::Null, |日期| {
            json!(日期.format("%Y-%m-%d").to_string())
        });
    }
    if *类型 == Type::TIMESTAMP {
        return 纪元时刻(取i64(原始)).map_or(JsonValue::Null, |时刻| {
            json!(时刻.format("%Y-%m-%d %H:%M:%S%.f").to_string())
        });
    }
    if *类型 == Type::TIMESTAMPTZ {
        // 带时区的一律以 UTC 的 RFC3339 呈现，免得两端各自猜本地时区。
        return 纪元时刻(取i64(原始))
            .map_or(JsonValue::Null, |时刻| json!(时刻.and_utc().to_rfc3339()));
    }
    if *类型 == Type::TIME {
        let 微秒 = 取i64(原始);
        return json!(format!(
            "{:02}:{:02}:{:02}{}",
            微秒 / 3_600_000_000,
            微秒 / 60_000_000 % 60,
            微秒 / 1_000_000 % 60,
            if 微秒 % 1_000_000 == 0 {
                String::new()
            } else {
                format!(".{:06}", 微秒 % 1_000_000)
            }
        ));
    }
    宽松文本(原始)
}

fn 解码json(原始: &[u8]) -> JsonValue {
    std::str::from_utf8(原始)
        .ok()
        .and_then(|文本| serde_json::from_str(文本).ok())
        .unwrap_or(JsonValue::Null)
}

fn 解码uuid(原始: &[u8]) -> JsonValue {
    if 原始.len() != 16 {
        return JsonValue::Null;
    }
    let 十六进制: String = 原始.iter().map(|字节| format!("{字节:02x}")).collect();
    JsonValue::String(format!(
        "{}-{}-{}-{}-{}",
        &十六进制[0..8],
        &十六进制[8..12],
        &十六进制[12..16],
        &十六进制[16..20],
        &十六进制[20..32]
    ))
}

/// PG 的日期/时间戳纪元是 2000-01-01，不是 Unix 纪元。
fn pg纪元() -> chrono::NaiveDateTime {
    chrono::NaiveDate::from_ymd_opt(2000, 1, 1)
        .unwrap()
        .and_hms_opt(0, 0, 0)
        .unwrap()
}

fn 纪元日期(天数: i64) -> Option<chrono::NaiveDate> {
    pg纪元()
        .date()
        .checked_add_signed(chrono::Duration::days(天数))
}

fn 纪元时刻(微秒: i64) -> Option<chrono::NaiveDateTime> {
    pg纪元().checked_add_signed(chrono::Duration::microseconds(微秒))
}

/// numeric 的二进制布局：ndigits / weight / sign / dscale 四个 i16，
/// 后面跟 ndigits 个 base-10000 的数字组，值 = Σ digits[i] × 10000^(weight-i)。
/// 手写解码是为了拿到**精确十进制字符串**（不引 rust_decimal —— 它上限 28 位，
/// 而 PG numeric 可以更长）。
fn 解码numeric(原始: &[u8]) -> Option<String> {
    if 原始.len() < 8 {
        return None;
    }
    let 组数 = i16::from_be_bytes([原始[0], 原始[1]]) as i32;
    let 权重 = i16::from_be_bytes([原始[2], 原始[3]]) as i32;
    let 符号 = u16::from_be_bytes([原始[4], 原始[5]]);
    let 小数位 = u16::from_be_bytes([原始[6], 原始[7]]) as usize;
    if 符号 == 0xC000 {
        return Some("NaN".to_string());
    }
    if 原始.len() < 8 + 组数 as usize * 2 {
        return None;
    }
    let 组 = |序号: i32| -> i32 {
        if 序号 < 0 || 序号 >= 组数 {
            return 0;
        }
        let 偏移 = 8 + 序号 as usize * 2;
        i16::from_be_bytes([原始[偏移], 原始[偏移 + 1]]) as i32
    };

    let mut 结果 = String::new();
    if 符号 == 0x4000 {
        结果.push('-');
    }
    if 权重 < 0 {
        结果.push('0');
    } else {
        for 序号 in 0..=权重 {
            if 序号 == 0 {
                结果.push_str(&组(序号).to_string());
            } else {
                结果.push_str(&format!("{:04}", 组(序号)));
            }
        }
    }
    if 小数位 > 0 {
        let mut 小数 = String::new();
        let mut 序号 = 权重 + 1;
        while 小数.len() < 小数位 {
            小数.push_str(&format!("{:04}", 组(序号)));
            序号 += 1;
        }
        小数.truncate(小数位);
        结果.push('.');
        结果.push_str(&小数);
    }
    Some(结果)
}
