// 数据库后端分发层：按连接串 scheme 选驱动，把三家驱动的差异收口成同一组方法。
//
// 为什么单独成文件：database_ffi.rs 原本 591 行，三家驱动全塞进去必破
// 「单文件 < 1000 行」。
// 为什么用 include! 拼进 database_ffi 模块而不是声明成 mod 子模块：
// qi-runtime/src/stdlib/database_ffi.rs 是 include! 同一份源码的薄壳，
// `mod 后端;` 在两个 crate 里会去各自的目录找文件，只有一份源码时必然有一边落空；
// 而嵌套的 include! 按「宏调用所在文件」解析相对路径，两个 crate 都指向这里。

/// 参数窄腰 —— FFI 只能递 JSON，所以就这 5 种（同 Go `driver.Value` 的思路）。
/// 各后端在自己的边界上转成驱动原生类型，复杂类型一律由驱动降级。
#[derive(Debug, Clone)]
enum 参数值 {
    空,
    布尔(bool),
    整数(i64),
    浮点(f64),
    文本(String),
}

/// 统一错误。
///
/// 错误码对 SQLite 保持 `extended_code` 原样 —— 它会写进 `执行参数` 返回的
/// 结果 JSON，应用侧（如唯一约束冲突判断）依赖这个数字，不能变。
/// 其它后端统一 -1：PG 的 SQLSTATE 是五位字母数字，塞不进 i32，也不该自己乱编号。
///
/// 实现 `std::error::Error` 是给连接池用的：r2d2 的 `ManageConnection::Error`
/// 要求 `error::Error + 'static`，不实现就得在池那层再套一层错误类型。
#[derive(Debug)]
struct 数据库错误 {
    消息: String,
    错误码: i32,
}

impl std::fmt::Display for 数据库错误 {
    fn fmt(&self, 输出: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(输出, "{}", self.消息)
    }
}

impl std::error::Error for 数据库错误 {}

impl 数据库错误 {
    fn 新(消息: impl Into<String>) -> Self {
        Self {
            消息: 消息.into(),
            错误码: -1,
        }
    }
}

impl From<SqlError> for 数据库错误 {
    fn from(错误: SqlError) -> Self {
        Self {
            错误码: sqlite_error_code(&错误),
            消息: 错误.to_string(),
        }
    }
}

/// 事务控制语句。三家方言不同（`BEGIN IMMEDIATE` 只有 SQLite 有，
/// MySQL 要 `START TRANSACTION`），所以层间传枚举而不是裸 SQL 串。
#[derive(Clone, Copy, PartialEq, Eq)]
enum 事务动作 {
    开始,
    独占开始,
    提交,
    回滚,
}

/// 三个后端。SQLite 这一支的每一次调用都与改造前逐字节一致。
///
/// PG/MySQL 装箱：两个网络驱动的连接结构体（读写缓冲、预处理语句表）比
/// `rusqlite::Connection` 那个裸指针大出两个数量级，不装箱的话整个枚举被撑大，
/// 只连 SQLite 的程序也得为此掏内存。
enum 后端 {
    SQLite(Connection),
    #[cfg(feature = "db-postgres")]
    PG(Box<PG连接>),
    #[cfg(feature = "db-mysql")]
    MySQL(Box<MySQL连接>),
}

impl 后端 {
    /// 按连接串 scheme 分发。
    ///
    /// **无 scheme 的裸串原样交给 SQLite** —— 现有 12+ 个应用写的都是
    /// `绘本.db` / `:memory:` 这种路径，这条分支不能有任何额外解析。
    fn 打开(连接串: &str) -> Result<后端, 数据库错误> {
        let 小写 = 连接串.to_ascii_lowercase();

        if 小写.starts_with("postgres://") || 小写.starts_with("postgresql://") {
            #[cfg(feature = "db-postgres")]
            return PG连接::打开(连接串).map(|连接| 后端::PG(Box::new(连接)));
            #[cfg(not(feature = "db-postgres"))]
            return Err(数据库错误::新(
                "本运行时未编入 PostgreSQL 后端（db-postgres）",
            ));
        }

        if 小写.starts_with("mysql://") {
            #[cfg(feature = "db-mysql")]
            return MySQL连接::打开(连接串).map(|连接| 后端::MySQL(Box::new(连接)));
            #[cfg(not(feature = "db-mysql"))]
            return Err(数据库错误::新(
                "本运行时未编入 MySQL 后端（db-mysql）",
            ));
        }

        // sqlite:// 是显式写法；sqlite:///绝对/路径 去掉前缀后正好剩下 /绝对/路径。
        let 路径 = 连接串
            .get("sqlite://".len()..)
            .filter(|_| 小写.starts_with("sqlite://"));
        Ok(后端::SQLite(Connection::open(路径.unwrap_or(连接串))?))
    }

    /// 后端名，给 `数据库.后端()` 用，也是上层挑 DDL 方言的依据。
    fn 名称(&self) -> &'static str {
        match self {
            后端::SQLite(_) => "sqlite",
            #[cfg(feature = "db-postgres")]
            后端::PG(_) => "postgres",
            #[cfg(feature = "db-mysql")]
            后端::MySQL(_) => "mysql",
        }
    }

    /// 无参执行，返回影响行数。
    fn 执行(&mut self, sql: &str) -> Result<usize, 数据库错误> {
        match self {
            后端::SQLite(连接) => Ok(连接.execute(sql, [])?),
            #[cfg(feature = "db-postgres")]
            后端::PG(连接) => 连接.执行(sql),
            #[cfg(feature = "db-mysql")]
            后端::MySQL(连接) => 连接.执行(sql),
        }
    }

    /// 参数化执行，返回（影响行数，最后插入 id）。
    fn 执行参数(
        &mut self,
        sql: &str,
        参数: &[参数值],
    ) -> Result<(usize, i64), 数据库错误> {
        match self {
            后端::SQLite(连接) => {
                let 值 = 转sqlite参数(参数);
                let 行数 = 连接.execute(sql, params_from_iter(值.iter()))?;
                Ok((行数, 连接.last_insert_rowid()))
            }
            #[cfg(feature = "db-postgres")]
            后端::PG(连接) => 连接.执行参数(sql, 参数),
            #[cfg(feature = "db-mysql")]
            后端::MySQL(连接) => 连接.执行参数(sql, 参数),
        }
    }

    /// 参数化查询，返回 JSON 行数组的字符串。
    fn 查询(&mut self, sql: &str, 参数: &[参数值]) -> Result<String, 数据库错误> {
        match self {
            后端::SQLite(连接) => Ok(query_json(连接, sql, &转sqlite参数(参数))?),
            #[cfg(feature = "db-postgres")]
            后端::PG(连接) => 连接.查询(sql, 参数),
            #[cfg(feature = "db-mysql")]
            后端::MySQL(连接) => 连接.查询(sql, 参数),
        }
    }

    fn 事务(&mut self, 动作: 事务动作) -> Result<(), 数据库错误> {
        match self {
            后端::SQLite(连接) => {
                let sql = match 动作 {
                    事务动作::开始 => "BEGIN TRANSACTION",
                    事务动作::独占开始 => "BEGIN IMMEDIATE",
                    事务动作::提交 => "COMMIT",
                    事务动作::回滚 => "ROLLBACK",
                };
                连接.execute(sql, [])?;
                Ok(())
            }
            #[cfg(feature = "db-postgres")]
            后端::PG(连接) => 连接.事务(动作),
            #[cfg(feature = "db-mysql")]
            后端::MySQL(连接) => 连接.事务(动作),
        }
    }

    /// 探活。连接池在**借出前**调它（`test_on_check_out`）。
    ///
    /// 服务端重启、idle timeout、网络断，都会让池里留下僵尸连接 —— 只有真发一条
    /// 语句才认得出来。坏连接由 r2d2 当场丢弃并新建，业务侧看不到这次失败。
    #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
    fn 探活(&mut self) -> Result<(), 数据库错误> {
        match self {
            // SQLite 不进池（见 database/句柄.rs），这条分支只是让 match 完整。
            后端::SQLite(连接) => Ok(连接.execute_batch("SELECT 1")?),
            #[cfg(feature = "db-postgres")]
            后端::PG(连接) => 连接.探活(),
            #[cfg(feature = "db-mysql")]
            后端::MySQL(连接) => 连接.探活(),
        }
    }

    /// 连接是否已经确定断了。归还池子时问一次，true 就直接扔掉不复用。
    #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
    fn 已断开(&mut self) -> bool {
        match self {
            后端::SQLite(_) => false,
            #[cfg(feature = "db-postgres")]
            后端::PG(连接) => 连接.已断开(),
            // mysql crate 没有「连接是否已关闭」的只读查询；漏判的那条僵尸连接
            // 会在下次借出时被 探活 拦下，代价只是一次重建。
            #[cfg(feature = "db-mysql")]
            后端::MySQL(_) => false,
        }
    }

    /// 统一的「最后插入 id」：SQLite 读 rowid，PG 读会话 `lastval()`，
    /// MySQL 读 `LAST_INSERT_ID()`。
    fn 最后插入id(&mut self) -> i64 {
        match self {
            后端::SQLite(连接) => 连接.last_insert_rowid(),
            #[cfg(feature = "db-postgres")]
            后端::PG(连接) => 连接.最后插入id(),
            #[cfg(feature = "db-mysql")]
            后端::MySQL(连接) => 连接.最后插入id(),
        }
    }
}

/// 窄腰参数 → rusqlite 值。布尔按老规矩降成 0/1 整数（SQLite 没有布尔类型）。
fn 转sqlite参数(参数: &[参数值]) -> Vec<SqlValue> {
    参数
        .iter()
        .map(|值| match 值 {
            参数值::空 => SqlValue::Null,
            参数值::布尔(值) => SqlValue::Integer(i64::from(*值)),
            参数值::整数(值) => SqlValue::Integer(*值),
            参数值::浮点(值) => SqlValue::Real(*值),
            参数值::文本(值) => SqlValue::Text(值.clone()),
        })
        .collect()
}

fn row_value(value: ValueRef<'_>) -> JsonValue {
    match value {
        ValueRef::Null => JsonValue::Null,
        ValueRef::Integer(value) => json!(value),
        ValueRef::Real(value) => json!(value),
        ValueRef::Text(value) => json!(String::from_utf8_lossy(value)),
        ValueRef::Blob(value) => json!(value),
    }
}

fn query_json(conn: &Connection, sql: &str, params: &[SqlValue]) -> Result<String, SqlError> {
    let mut stmt = conn.prepare(sql)?;
    let column_count = stmt.column_count();
    let column_names: Vec<String> = (0..column_count)
        .map(|index| stmt.column_name(index).unwrap_or("").to_string())
        .collect();
    let mut rows = stmt.query(params_from_iter(params.iter()))?;
    let mut results = Vec::new();

    while let Some(row) = rows.next()? {
        let mut row_map = serde_json::Map::new();
        for (index, column_name) in column_names.iter().enumerate() {
            row_map.insert(column_name.clone(), row_value(row.get_ref(index)?));
        }
        results.push(JsonValue::Object(row_map));
    }

    Ok(JsonValue::Array(results).to_string())
}
