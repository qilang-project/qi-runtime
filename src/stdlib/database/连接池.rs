// 网络后端（PostgreSQL / MySQL）的连接池。
//
// 为什么池在这一层而不在驱动里：设计文档原则 2 —— Go 的 `database/sql` 统一池化，
// JDBC 不管于是每个项目自己选 HikariCP。这里跟 Go 走。
//
// 为什么只用 r2d2 核心、不用 r2d2_postgres / r2d2_mysql：`后端` 枚举已经把两家
// 驱动收成同一组方法，要池化的是 `后端` 而不是 `postgres::Client`。用官方适配器
// 反而得按后端切成两个不同类型的池，把刚统一好的分发再拆开一遍；而
// `ManageConnection` 一共就三个方法，自己实现比拆分发便宜得多。
//
// SQLite **不进这里**。它是文件库，rusqlite 的 Connection 也不是那个用法，
// 池化只会把 `database is locked` 引进来（见 database/句柄.rs 的分派）。

use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

type 借出连接 = r2d2::PooledConnection<后端管理器>;

/// 一个事务句柄独占的连接。
///
/// 放进 `Arc<Mutex<..>>` 而不是直接塞进事务表：塞进表的话，用事务时要么全程握着
/// 表锁（所有事务被串成一条队），要么把连接从表里摘出来用完再放回（这期间别处看
/// 这个事务句柄就像「已结束」）。一层薄锁两个问题都没有。
/// `None` 表示已提交/回滚，连接已归还池子。
type 事务槽 = Arc<Mutex<Option<借出连接>>>;

/// 池参数。**默认值刻意保守** —— PG 的 `max_connections` 默认才 100，
/// 一个应用吃掉几十条不礼貌；qi 应用普遍是「一个句柄全局复用」，8 条足够把
/// 请求并行度撑起来。
struct 池配置 {
    最大连接数: u32,
    获取超时: Duration,
}

impl Default for 池配置 {
    fn default() -> Self {
        Self {
            最大连接数: 8,
            获取超时: Duration::from_millis(5000),
        }
    }
}

/// 池参数走**连接串查询串**：
///
/// ```text
/// postgres://qi:pw@127.0.0.1:45432/qi_dev?pool_max=8&pool_timeout_ms=5000
/// mysql://qi:pw@127.0.0.1:43306/qi_dev?pool_max=4
/// ```
///
/// 选查询串而不选环境变量：一个进程连多个库是常态（主库 + 统计库），
/// 环境变量没法分别配。`pool_` 前缀的参数在发给驱动**之前**摘掉 ——
/// postgres / mysql 两个 crate 见到不认识的参数会直接拒连，不摘就连不上。
/// 其余查询参数（`sslmode`、`connect_timeout`…）原样透传给驱动。
fn 拆池参数(连接串: &str) -> (String, 池配置) {
    let mut 配置 = 池配置::default();
    let Some((前段, 查询)) = 连接串.split_once('?') else {
        return (连接串.to_string(), 配置);
    };

    let mut 留给驱动: Vec<&str> = Vec::new();
    for 一项 in 查询.split('&').filter(|一项| !一项.is_empty()) {
        let (键, 值) = 一项.split_once('=').unwrap_or((一项, ""));
        match 键 {
            // 解析不出数字就用默认值：连接串里写错一个池参数，不该让整个应用连不上库。
            "pool_max" => {
                if let Ok(数) = 值.parse::<u32>() {
                    配置.最大连接数 = 数.max(1);
                }
            }
            "pool_timeout_ms" => {
                if let Ok(毫秒) = 值.parse::<u64>() {
                    配置.获取超时 = Duration::from_millis(毫秒.max(1));
                }
            }
            _ => 留给驱动.push(一项),
        }
    }

    let 干净串 = if 留给驱动.is_empty() {
        前段.to_string()
    } else {
        format!("{}?{}", 前段, 留给驱动.join("&"))
    };
    (干净串, 配置)
}

/// 只有网络后端进池。判据与 `后端::打开` 的 scheme 分发保持一致。
fn 是网络连接串(连接串: &str) -> bool {
    let 小写 = 连接串.to_ascii_lowercase();
    小写.starts_with("postgres://") || 小写.starts_with("postgresql://") || 小写.starts_with("mysql://")
}

/// r2d2 的连接管理器。建连接就是走原来那条 `后端::打开`，所以池化没有引入
/// 第二套连接逻辑（TLS、参数解析、scheme 分发都还是那一份）。
struct 后端管理器 {
    连接串: String,
}

impl r2d2::ManageConnection for 后端管理器 {
    type Connection = 后端;
    type Error = 数据库错误;

    fn connect(&self) -> Result<后端, 数据库错误> {
        后端::打开(&self.连接串)
    }

    fn is_valid(&self, 连接: &mut 后端) -> Result<(), 数据库错误> {
        连接.探活()
    }

    fn has_broken(&self, 连接: &mut 后端) -> bool {
        连接.已断开()
    }
}

/// 一个 qi 库句柄背后的池。
struct 网络池 {
    池: r2d2::Pool<后端管理器>,
    名称: &'static str,
    /// 旧 API（`开始事务` → `执行` → `提交`）钉住的那条连接。
    ///
    /// 旧 API 的事务开在**连接句柄**上，之后每一句 `执行(库, …)` 都必须落到同一条
    /// 物理连接：BEGIN 在连接 A、INSERT 跑到连接 B、COMMIT 又在连接 C 的话，
    /// 数据静默写错还不报错。所以这条连接被独占到提交/回滚为止，期间整个句柄的
    /// 无参 `执行/查询` 都走它 —— 与池化前逐字节同义。
    连接级事务: Mutex<Option<借出连接>>,
    /// 新 API（`开启事务` → `事务执行参数` → `提交事务`）：事务句柄 → 独占的连接。
    /// 与旧 API 不同，这些事务**彼此并行**，各占一条连接 —— 这正是池化的目的
    /// （qi-web 每个请求一个事务，池化前它们全排在唯一那条连接上）。
    事务表: Mutex<HashMap<i64, 事务槽>>,
    /// 「最后插入 id」在三家后端都是**会话态**（rowid / lastval() / LAST_INSERT_ID()），
    /// 一池多连接之后再问哪条连接都可能答错，所以在句柄这层记住写语句刚返回的那个值。
    最后插入: AtomicI64,
}

impl 网络池 {
    fn 新建(连接串: &str) -> Result<网络池, 数据库错误> {
        let (干净串, 配置) = 拆池参数(连接串);
        let 名称 = if 干净串.to_ascii_lowercase().starts_with("mysql://") {
            "mysql"
        } else {
            "postgres"
        };

        let 池 = r2d2::Pool::builder()
            .max_size(配置.最大连接数)
            // 常备一条：`数据库.连接()` 必须在连不上时当场返回 -1（12+ 个应用都靠这个
            // 判断），而不是先给个句柄、等第一次查询才炸。min_idle=1 让 build() 去建
            // 那一条并把失败带回来。
            .min_idle(Some(1))
            .connection_timeout(配置.获取超时)
            // 借出前探活：服务端重启 / idle timeout 之后池里会留僵尸连接，
            // 不探活就是把「连接已关闭」直接甩给业务。
            .test_on_check_out(true)
            .build(后端管理器 { 连接串: 干净串 })
            .map_err(|错误| 数据库错误::新(format!("连接池初始化失败: {错误}")))?;

        Ok(网络池 {
            池,
            名称,
            连接级事务: Mutex::new(None),
            事务表: Mutex::new(HashMap::new()),
            最后插入: AtomicI64::new(0),
        })
    }

    fn 名称(&self) -> &'static str {
        self.名称
    }

    fn 借出(&self) -> Result<借出连接, 数据库错误> {
        self.池
            .get()
            .map_err(|错误| 数据库错误::新(format!("从连接池取连接失败: {错误}")))
    }

    /// 0 表示这条语句没产生自增主键（或这条物理连接压根没插过），
    /// 写进去只会把上一次的真值冲掉。
    fn 记住插入id(&self, 编号: i64) {
        if 编号 != 0 {
            self.最后插入.store(编号, Ordering::Relaxed);
        }
    }

    /// 无参语句的落点：连接级事务开着就走钉住的那条，否则临时借一条。
    fn 在钉住或借出的连接上<T>(
        &self,
        操作: impl FnOnce(&mut 后端) -> Result<T, 数据库错误>,
    ) -> Result<T, 数据库错误> {
        let mut 钉住 = self.连接级事务.lock().unwrap();
        if let Some(连接) = 钉住.as_mut() {
            return 操作(连接);
        }
        drop(钉住);
        let mut 连接 = self.借出()?;
        操作(&mut 连接)
    }

    fn 执行(&self, sql: &str) -> Result<usize, 数据库错误> {
        let (行数, 编号) = self.在钉住或借出的连接上(|连接| {
            let 行数 = 连接.执行(sql)?;
            Ok((行数, 连接.最后插入id()))
        })?;
        self.记住插入id(编号);
        Ok(行数)
    }

    fn 查询(&self, sql: &str) -> Result<String, 数据库错误> {
        self.在钉住或借出的连接上(|连接| 连接.查询(sql, &[]))
    }

    /// 参数化语句在连接级事务期间一律拒绝 —— 与池化前的单连接语义一字不差
    /// （`ConnectionState.active_transaction.is_some()` 时直接失败）。
    fn 拒绝连接级事务(&self) -> Result<(), 数据库错误> {
        if self.连接级事务.lock().unwrap().is_some() {
            return Err(数据库错误::新("数据库连接正被事务占用"));
        }
        Ok(())
    }

    fn 执行参数(
        &self,
        sql: &str,
        参数: &[参数值],
    ) -> Result<(usize, i64), 数据库错误> {
        self.拒绝连接级事务()?;
        let mut 连接 = self.借出()?;
        let 结果 = 连接.执行参数(sql, 参数)?;
        self.记住插入id(结果.1);
        Ok(结果)
    }

    fn 查询参数(&self, sql: &str, 参数: &[参数值]) -> Result<String, 数据库错误> {
        self.拒绝连接级事务()?;
        let mut 连接 = self.借出()?;
        连接.查询(sql, 参数)
    }

    fn 最后插入id(&self) -> i64 {
        self.最后插入.load(Ordering::Relaxed)
    }

    // ── 旧 API：事务开在连接句柄上 ──────────────────────────────────────────

    fn 开始连接级事务(&self) -> Result<(), 数据库错误> {
        let mut 钉住 = self.连接级事务.lock().unwrap();
        if 钉住.is_some() {
            return Err(数据库错误::新("连接上已有活动事务"));
        }
        if !self.事务表.lock().unwrap().is_empty() {
            return Err(数据库错误::新("连接正被事务句柄占用"));
        }
        let mut 连接 = self.借出()?;
        连接.事务(事务动作::开始)?;
        *钉住 = Some(连接);
        Ok(())
    }

    fn 结束连接级事务(&self, 动作: 事务动作) -> Result<(), 数据库错误> {
        let mut 钉住 = self.连接级事务.lock().unwrap();
        let Some(连接) = 钉住.as_mut() else {
            return Err(数据库错误::新("连接上没有活动事务"));
        };
        连接.事务(动作)?;
        // 成功才归还；失败时保持钉住（与单连接一致：提交失败不清 active_transaction，
        // 调用方还能回滚）。
        钉住.take();
        Ok(())
    }

    // ── 新 API：事务句柄独占一条连接 ────────────────────────────────────────

    fn 开启事务(&self, 事务号: i64) -> Result<(), 数据库错误> {
        if self.连接级事务.lock().unwrap().is_some() {
            return Err(数据库错误::新("连接正被连接级事务占用"));
        }
        let mut 连接 = self.借出()?;
        连接.事务(事务动作::独占开始)?;
        self.事务表
            .lock()
            .unwrap()
            .insert(事务号, Arc::new(Mutex::new(Some(连接))));
        Ok(())
    }

    fn 取事务槽(&self, 事务号: i64) -> Result<事务槽, 数据库错误> {
        self.事务表
            .lock()
            .unwrap()
            .get(&事务号)
            .cloned()
            .ok_or_else(|| 数据库错误::新("事务句柄无效"))
    }

    fn 事务执行参数(
        &self,
        事务号: i64,
        sql: &str,
        参数: &[参数值],
    ) -> Result<(usize, i64), 数据库错误> {
        let 槽 = self.取事务槽(事务号)?;
        let mut 守 = 槽.lock().unwrap();
        let 连接 = 守.as_mut().ok_or_else(|| 数据库错误::新("事务已结束"))?;
        let 结果 = 连接.执行参数(sql, 参数)?;
        drop(守);
        self.记住插入id(结果.1);
        Ok(结果)
    }

    fn 事务查询参数(
        &self,
        事务号: i64,
        sql: &str,
        参数: &[参数值],
    ) -> Result<String, 数据库错误> {
        let 槽 = self.取事务槽(事务号)?;
        let mut 守 = 槽.lock().unwrap();
        let 连接 = 守.as_mut().ok_or_else(|| 数据库错误::新("事务已结束"))?;
        连接.查询(sql, 参数)
    }

    fn 结束事务(&self, 事务号: i64, 动作: 事务动作) -> Result<(), 数据库错误> {
        let 槽 = self
            .事务表
            .lock()
            .unwrap()
            .remove(&事务号)
            .ok_or_else(|| 数据库错误::新("事务句柄无效"))?;
        let mut 守 = 槽.lock().unwrap();
        let mut 连接 = 守.take().ok_or_else(|| 数据库错误::新("事务已结束"))?;
        let 结果 = 连接.事务(动作);
        // 提交失败的连接可能还挂着事务，直接还回池子会污染下一个借它的人。
        if 结果.is_err() {
            let _ = 连接.事务(事务动作::回滚);
        }
        结果 // 连接 在此 drop —— 归还池子
    }

    /// 关闭句柄：把所有被事务独占的连接回滚并还回池子（否则池子慢慢漏干），
    /// 返回需要作废的事务句柄号。
    fn 关闭(&self) -> Vec<i64> {
        if let Some(mut 连接) = self.连接级事务.lock().unwrap().take() {
            let _ = 连接.事务(事务动作::回滚);
        }
        let 事务表 = std::mem::take(&mut *self.事务表.lock().unwrap());
        事务表
            .into_iter()
            .map(|(事务号, 槽)| {
                if let Some(mut 连接) = 槽.lock().unwrap().take() {
                    let _ = 连接.事务(事务动作::回滚);
                }
                事务号
            })
            .collect()
    }
}
