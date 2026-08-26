// 连接句柄：qi 侧一个「库句柄」背后到底是什么东西。
//
// 池化前：句柄 = 一条物理连接 + 一把 Mutex，每次 `执行/查询` 都要抢这把锁。
// 对 SQLite 无所谓（本地文件、微秒级），对 PG/MySQL 是灾难 —— 锁被握住整个网络
// 往返，而应用侧的真实形态恰恰是「启动时 `数据库.连接()` 一次、句柄全局复用」
// （项目/看板/主程序.qi:49、项目/家有小奇/家庭.qi:46 …），于是 qi-web 并发下
// 所有请求都排在那一条连接上。
//
// 池化后：**只有网络后端**变成池，SQLite 一个字节都没动 —— 它是文件库，
// rusqlite 的 Connection 也不是那个用法，池化只会把 `database is locked` 引进来。
//
// 这一层还负责把两套事务 API 的语义收口，FFI 边界（database_ffi.rs）因此保持薄。

struct ConnectionState {
    conn: 后端,
    // 0 表示旧 API 开启的连接级事务，正数表示新事务句柄。
    active_transaction: Option<i64>,
}

/// 一个 qi 库句柄。
enum 连接句柄 {
    /// SQLite：一条连接一把锁，与池化前逐字节同义。
    单连接(Mutex<ConnectionState>),
    /// PostgreSQL / MySQL：一个池。
    #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
    池(网络池),
}

impl 连接句柄 {
    fn 打开(连接串: &str) -> Result<连接句柄, 数据库错误> {
        #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
        if 是网络连接串(连接串) {
            return 网络池::新建(连接串).map(连接句柄::池);
        }
        Ok(连接句柄::单连接(Mutex::new(ConnectionState {
            conn: 后端::打开(连接串)?,
            active_transaction: None,
        })))
    }

    fn 名称(&self) -> String {
        match self {
            连接句柄::单连接(状态) => 状态.lock().unwrap().conn.名称().to_string(),
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.名称().to_string(),
        }
    }

    // ── 连接句柄上的直接读写 ────────────────────────────────────────────────

    /// 无参执行。新 API 事务开着时，单连接后端拒绝（那条连接被事务独占）；
    /// 池后端不必拒绝 —— 事务占的是它自己那条，这里另借一条即可。
    fn 执行(&self, sql: &str) -> Result<usize, 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if matches!(状态.active_transaction, Some(事务号) if 事务号 > 0) {
                    return Err(数据库错误::新("连接正被事务句柄占用"));
                }
                状态.conn.执行(sql)
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.执行(sql),
        }
    }

    fn 查询(&self, sql: &str) -> Result<String, 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if matches!(状态.active_transaction, Some(事务号) if 事务号 > 0) {
                    return Err(数据库错误::新("连接正被事务句柄占用"));
                }
                状态.conn.查询(sql, &[])
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.查询(sql),
        }
    }

    fn 执行参数(&self, sql: &str, 参数: &[参数值]) -> Result<(usize, i64), 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if 状态.active_transaction.is_some() {
                    return Err(数据库错误::新("数据库连接正被事务占用"));
                }
                状态.conn.执行参数(sql, 参数)
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.执行参数(sql, 参数),
        }
    }

    fn 查询参数(&self, sql: &str, 参数: &[参数值]) -> Result<String, 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if 状态.active_transaction.is_some() {
                    return Err(数据库错误::新("数据库连接正被事务占用"));
                }
                状态.conn.查询(sql, 参数)
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.查询参数(sql, 参数),
        }
    }

    fn 最后插入id(&self) -> i64 {
        match self {
            连接句柄::单连接(状态) => 状态.lock().unwrap().conn.最后插入id(),
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.最后插入id(),
        }
    }

    // ── 旧 API 事务（开在连接句柄上）────────────────────────────────────────

    fn 开始连接级事务(&self) -> Result<(), 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if 状态.active_transaction.is_some() {
                    return Err(数据库错误::新("连接上已有活动事务"));
                }
                状态.conn.事务(事务动作::开始)?;
                状态.active_transaction = Some(0);
                Ok(())
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.开始连接级事务(),
        }
    }

    fn 结束连接级事务(&self, 动作: 事务动作) -> Result<(), 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if 状态.active_transaction != Some(0) {
                    return Err(数据库错误::新("连接上没有活动的连接级事务"));
                }
                状态.conn.事务(动作)?;
                状态.active_transaction = None;
                Ok(())
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.结束连接级事务(动作),
        }
    }

    // ── 新 API 事务（独占一条连接）──────────────────────────────────────────

    fn 开启事务(&self, 事务号: i64) -> Result<(), 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if 状态.active_transaction.is_some() {
                    return Err(数据库错误::新("连接上已有活动事务"));
                }
                状态.conn.事务(事务动作::独占开始)?;
                状态.active_transaction = Some(事务号);
                Ok(())
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.开启事务(事务号),
        }
    }

    fn 事务执行参数(
        &self,
        事务号: i64,
        sql: &str,
        参数: &[参数值],
    ) -> Result<(usize, i64), 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if 状态.active_transaction != Some(事务号) {
                    return Err(数据库错误::新("事务已结束"));
                }
                状态.conn.执行参数(sql, 参数)
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.事务执行参数(事务号, sql, 参数),
        }
    }

    fn 事务查询参数(
        &self,
        事务号: i64,
        sql: &str,
        参数: &[参数值],
    ) -> Result<String, 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if 状态.active_transaction != Some(事务号) {
                    return Err(数据库错误::新("事务已结束"));
                }
                状态.conn.查询(sql, 参数)
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.事务查询参数(事务号, sql, 参数),
        }
    }

    fn 结束事务(&self, 事务号: i64, 动作: 事务动作) -> Result<(), 数据库错误> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if 状态.active_transaction != Some(事务号) {
                    return Err(数据库错误::新("事务已结束"));
                }
                状态.conn.事务(动作)?;
                状态.active_transaction = None;
                Ok(())
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.结束事务(事务号, 动作),
        }
    }

    /// 关闭：回滚一切还开着的事务，返回要作废的事务句柄号。
    /// 池后端在这里把被事务独占的连接还回池子 —— 调用方忘了提交就断开时，
    /// 不还的话池子会慢慢漏干。
    fn 关闭(&self) -> Vec<i64> {
        match self {
            连接句柄::单连接(状态) => {
                let mut 状态 = 状态.lock().unwrap();
                if 状态.active_transaction.is_some() {
                    let _ = 状态.conn.事务(事务动作::回滚);
                }
                状态
                    .active_transaction
                    .take()
                    .filter(|事务号| *事务号 > 0)
                    .into_iter()
                    .collect()
            }
            #[cfg(any(feature = "db-postgres", feature = "db-mysql"))]
            连接句柄::池(池) => 池.关闭(),
        }
    }
}
