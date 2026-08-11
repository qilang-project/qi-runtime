//! Redis FFI —— 键值 / 哈希 / 列表 / 集合 / 发布订阅
//!
//! ── 为什么 qi 需要它 ────────────────────────────────────────────
//!
//! qi-web 的会话存储到今天为止是**进程内的 hashmap**（qi-web/会话.qi），
//! 广播也只到本进程为止。也就是说 qi 写的 Web 应用没法起第二个实例：
//! 用户在 A 进程登录，下一个请求打到 B 就掉登录。Redis 补的是这块地基，
//! 不是「多一个数据库」。
//!
//! ── 跟 标准库.数据库 的关系：不共用那套接口 ─────────────────────
//!
//! 数据库那层的形状是 `执行/查询/事务` + 返回 JSON 行，是 SQL 的形状。
//! Redis 是命令 + 类型化的值，硬套进去只会两边都别扭。所以这是独立一层，
//! 只有两件事照抄：**r2d2 连接池**、**`?pool_max=` 走连接串**（一个进程
//! 连多个实例是常态，环境变量没法分别配）。
//!
//! ── 值一律当 UTF-8 文本 ──────────────────────────────────────────
//!
//! Redis 的值本身是二进制安全的，qi 这一侧的字符串是 C 串（不能含 NUL）。
//! 取到非 UTF-8 的值时返回空串并置最后错误，**不做有损转换** —— 静默
//! 把字节改掉比报错难查得多。要存二进制先自己 base64。
//!
//! ── 错误怎么让 qi 看见 ──────────────────────────────────────────
//!
//! 整数返回的操作失败给 -1，字符串返回的失败给 ""。但 "" 同时也是
//! 「键不存在」，`自增` 的 -1 也可能是合法结果，所以每个连接挂一条
//! `最后错误`：拿不准的时候查它。这跟数据库层「-1 + 自己判」的做法
//! 是同一个路子，只是多给了一句人话。

#![allow(non_snake_case)]

use std::collections::{HashMap, VecDeque};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::Duration;

use redis::Commands;
use serde_json::{Map as JsonMap, Value as JsonValue};

use crate::stdlib::qi_str::rc_cstr_from_string;

/// 连接句柄从 3_000_000 起，订阅句柄从 3_500_000 起 —— 跟邮箱(900_000)、
/// tokio TCP(1_000_000)、监听器(2_000_000) 的段位错开，混用时一眼看得出是谁。
static 连接计数器: AtomicI64 = AtomicI64::new(3_000_000);
static 订阅计数器: AtomicI64 = AtomicI64::new(3_500_000);

static 连接池表: OnceLock<Mutex<HashMap<i64, Arc<连接>>>> = OnceLock::new();
static 订阅表: OnceLock<Mutex<HashMap<i64, Arc<订阅>>>> = OnceLock::new();

/// 一条订阅最多攒多少条没被取走的消息。攒到这儿说明消费方卡住了。
const 订阅队上限: usize = 4096;

struct 连接 {
    池: r2d2::Pool<redis::Client>,
    最后错误: Mutex<String>,
}

/// 一条订阅：读线程往队里塞，qi 侧从队里取。socket 只有读线程碰。
struct 订阅 {
    队: Mutex<VecDeque<(String, String)>>,
    有货: Condvar,
    要停: AtomicBool,
    活着: AtomicBool,
}

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

fn 取连接表() -> &'static Mutex<HashMap<i64, Arc<连接>>> {
    连接池表.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 取订阅表() -> &'static Mutex<HashMap<i64, Arc<订阅>>> {
    订阅表.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 查连接(句柄: i64) -> Option<Arc<连接>> {
    取连接表()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&句柄)
        .cloned()
}

fn 读C串(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p).to_string_lossy().to_string() }
}

fn 出串(s: String) -> *mut c_char {
    rc_cstr_from_string(s)
}

fn 空串() -> *mut c_char {
    出串(String::new())
}

impl 连接 {
    fn 记错(&self, 消息: impl Into<String>) {
        *self.最后错误.lock().unwrap_or_else(|e| e.into_inner()) = 消息.into();
    }

    fn 清错(&self) {
        self.最后错误
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// 借一条连接干活。池子拿不到连接（全忙 / Redis 挂了）就记错返回 None。
    fn 借用(&self) -> Option<r2d2::PooledConnection<redis::Client>> {
        match self.池.get() {
            Ok(c) => Some(c),
            Err(e) => {
                self.记错(format!("取连接失败: {}", e));
                None
            }
        }
    }
}

/// 池参数跟数据库层同一套写法：`redis://127.0.0.1:46379/0?pool_max=16`。
/// `pool_` 前缀的参数在交给 redis crate **之前**摘掉 —— 它见到不认识的
/// 查询参数会直接拒绝解析 URL。
fn 拆池参数(连接串: &str) -> (String, 池配置) {
    let mut 配置 = 池配置::default();
    let Some((前段, 查询)) = 连接串.split_once('?') else {
        return (连接串.to_string(), 配置);
    };

    let mut 留给驱动: Vec<&str> = Vec::new();
    for 一项 in 查询.split('&').filter(|一项| !一项.is_empty()) {
        let (键, 值) = 一项.split_once('=').unwrap_or((一项, ""));
        match 键 {
            // 写错一个池参数不该让整个应用连不上库 —— 解析不出就用默认值
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

// ── 连接 ────────────────────────────────────────────────────────

/// 连接（建池）。失败返回 -1。
///
/// `redis://` 明文，`rediss://` 走 TLS。**建池时就连一条**（min_idle=1），
/// 所以「Redis 没起来」在 连接() 这一步就报，而不是等第一次 取() 才炸。
#[no_mangle]
pub extern "C" fn qi_redis_connect(url: *const c_char) -> i64 {
    let 原串 = 读C串(url);
    if 原串.is_empty() {
        return -1;
    }
    let (干净串, 配置) = 拆池参数(&原串);

    let 客户端 = match redis::Client::open(干净串.as_str()) {
        Ok(c) => c,
        Err(_) => return -1,
    };
    let 池 = match r2d2::Pool::builder()
        .max_size(配置.最大连接数)
        .min_idle(Some(1))
        .connection_timeout(配置.获取超时)
        .build(客户端)
    {
        Ok(p) => p,
        Err(_) => return -1,
    };

    let 句柄 = 连接计数器.fetch_add(1, Ordering::SeqCst);
    取连接表().lock().unwrap_or_else(|e| e.into_inner()).insert(
        句柄,
        Arc::new(连接 {
            池,
            最后错误: Mutex::new(String::new()),
        }),
    );
    句柄
}

/// 关闭：把句柄从表里摘掉，池随最后一个使用者析构。
#[no_mangle]
pub extern "C" fn qi_redis_close(句柄: i64) -> i64 {
    match 取连接表()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&句柄)
    {
        Some(_) => 0,
        None => -1,
    }
}

/// PING。通返回 1，不通返回 0。
#[no_mangle]
pub extern "C" fn qi_redis_ping(句柄: i64) -> i64 {
    let Some(连) = 查连接(句柄) else {
        return 0;
    };
    let Some(mut c) = 连.借用() else {
        return 0;
    };
    match redis::cmd("PING").query::<String>(&mut *c) {
        Ok(回) if 回 == "PONG" => {
            连.清错();
            1
        }
        Ok(回) => {
            连.记错(format!("PING 回了 {}", 回));
            0
        }
        Err(e) => {
            连.记错(e.to_string());
            0
        }
    }
}

/// 最近一次失败的说明。没有错误时返回空串。
#[no_mangle]
pub extern "C" fn qi_redis_last_error(句柄: i64) -> *mut c_char {
    let Some(连) = 查连接(句柄) else {
        return 出串("句柄无效".to_string());
    };
    let 错 = 连
        .最后错误
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    出串(错)
}

// ── 内部：把一次命令的结果收成 i64 / String ──────────────────────
//
// 每个 FFI 都要「查句柄 → 借连接 → 跑命令 → 记错」这四步，抽出来免得
// 二十几个函数各写一遍（写多了必然有一处忘了记错）。

fn 做整数<F>(句柄: i64, 干: F) -> i64
where
    F: FnOnce(&mut redis::Connection) -> redis::RedisResult<i64>,
{
    let Some(连) = 查连接(句柄) else {
        return -1;
    };
    let Some(mut c) = 连.借用() else {
        return -1;
    };
    match 干(&mut c) {
        Ok(v) => {
            连.清错();
            v
        }
        Err(e) => {
            连.记错(e.to_string());
            -1
        }
    }
}

/// 字符串类结果。键不存在返回空串（**不算错**，最后错误会被清掉），
/// 值不是合法 UTF-8 也返回空串但记错 —— 两者靠 最后错误 区分。
fn 做可选串<F>(句柄: i64, 干: F) -> *mut c_char
where
    F: FnOnce(&mut redis::Connection) -> redis::RedisResult<Option<Vec<u8>>>,
{
    let Some(连) = 查连接(句柄) else {
        return 空串();
    };
    let Some(mut c) = 连.借用() else {
        return 空串();
    };
    match 干(&mut c) {
        Ok(None) => {
            连.清错();
            空串()
        }
        Ok(Some(字节)) => match String::from_utf8(字节) {
            Ok(s) => {
                连.清错();
                出串(s)
            }
            Err(_) => {
                连.记错("值不是合法 UTF-8（要存二进制请先自己 base64）");
                空串()
            }
        },
        Err(e) => {
            连.记错(e.to_string());
            空串()
        }
    }
}

fn 做JSON<F>(句柄: i64, 干: F) -> *mut c_char
where
    F: FnOnce(&mut redis::Connection) -> redis::RedisResult<JsonValue>,
{
    let Some(连) = 查连接(句柄) else {
        return 出串("[]".to_string());
    };
    let Some(mut c) = 连.借用() else {
        return 出串("[]".to_string());
    };
    match 干(&mut c) {
        Ok(v) => {
            连.清错();
            出串(v.to_string())
        }
        Err(e) => {
            连.记错(e.to_string());
            出串("[]".to_string())
        }
    }
}

// ── 字符串键值 ──────────────────────────────────────────────────

/// SET。成功 1，失败 -1。
#[no_mangle]
pub extern "C" fn qi_redis_set(句柄: i64, 键: *const c_char, 值: *const c_char) -> i64 {
    let (k, v) = (读C串(键), 读C串(值));
    做整数(句柄, |c| c.set::<_, _, ()>(&k, &v).map(|_| 1))
}

/// SET + 过期秒数（SETEX）。会话、验证码这类东西该用这个，
/// 而不是 设() 完再 设过期() —— 那中间断一下就留下一个永不过期的键。
#[no_mangle]
pub extern "C" fn qi_redis_set_ex(
    句柄: i64, 键: *const c_char, 值: *const c_char, 秒: i64
) -> i64 {
    let (k, v) = (读C串(键), 读C串(值));
    if 秒 <= 0 {
        return -1;
    }
    做整数(句柄, move |c| {
        c.set_ex::<_, _, ()>(&k, &v, 秒 as u64).map(|_| 1)
    })
}

/// SET NX EX —— 抢到返回 1，已被别人占着返回 0，出错 -1。
/// 这是分布式锁 / 「同一本书只让一个进程做」那类事情的基本件。
#[no_mangle]
pub extern "C" fn qi_redis_set_nx(
    句柄: i64, 键: *const c_char, 值: *const c_char, 秒: i64
) -> i64 {
    let (k, v) = (读C串(键), 读C串(值));
    做整数(句柄, move |c| {
        let mut 命令 = redis::cmd("SET");
        命令.arg(&k).arg(&v).arg("NX");
        if 秒 > 0 {
            命令.arg("EX").arg(秒);
        }
        // 没抢到时 Redis 回 nil，收成 Option
        命令
            .query::<Option<String>>(c)
            .map(|回| if 回.is_some() { 1 } else { 0 })
    })
}

/// GET。键不存在返回空串 —— 要区分「不存在」和「存的就是空串」用 存在()。
#[no_mangle]
pub extern "C" fn qi_redis_get(句柄: i64, 键: *const c_char) -> *mut c_char {
    let k = 读C串(键);
    做可选串(句柄, |c| c.get::<_, Option<Vec<u8>>>(&k))
}

/// DEL，返回真删掉几个。
#[no_mangle]
pub extern "C" fn qi_redis_del(句柄: i64, 键: *const c_char) -> i64 {
    let k = 读C串(键);
    做整数(句柄, |c| c.del::<_, i64>(&k))
}

/// EXISTS，1/0。
#[no_mangle]
pub extern "C" fn qi_redis_exists(句柄: i64, 键: *const c_char) -> i64 {
    let k = 读C串(键);
    做整数(句柄, |c| {
        c.exists::<_, bool>(&k).map(|有| if 有 { 1 } else { 0 })
    })
}

/// INCR，返回自增后的新值。**新值本身可能是负数**，所以别拿 -1 当失败判据，
/// 拿不准查 最后错误()。
#[no_mangle]
pub extern "C" fn qi_redis_incr(句柄: i64, 键: *const c_char) -> i64 {
    let k = 读C串(键);
    做整数(句柄, |c| c.incr::<_, i64, i64>(&k, 1))
}

/// INCRBY（增量可负，就是 DECRBY）。
#[no_mangle]
pub extern "C" fn qi_redis_incr_by(句柄: i64, 键: *const c_char, 增量: i64) -> i64 {
    let k = 读C串(键);
    做整数(句柄, move |c| c.incr::<_, i64, i64>(&k, 增量))
}

/// EXPIRE。设上了返回 1，键不存在返回 0。
#[no_mangle]
pub extern "C" fn qi_redis_expire(句柄: i64, 键: *const c_char, 秒: i64) -> i64 {
    let k = 读C串(键);
    做整数(句柄, move |c| {
        c.expire::<_, bool>(&k, 秒).map(|成| if 成 { 1 } else { 0 })
    })
}

/// TTL：剩余秒数；-1 没设过期；-2 键不存在。
#[no_mangle]
pub extern "C" fn qi_redis_ttl(句柄: i64, 键: *const c_char) -> i64 {
    let k = 读C串(键);
    做整数(句柄, |c| c.ttl::<_, i64>(&k))
}

// ── 哈希 ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn qi_redis_hset(
    句柄: i64,
    键: *const c_char,
    字段: *const c_char,
    值: *const c_char,
) -> i64 {
    let (k, f, v) = (读C串(键), 读C串(字段), 读C串(值));
    做整数(句柄, |c| c.hset::<_, _, _, ()>(&k, &f, &v).map(|_| 1))
}

#[no_mangle]
pub extern "C" fn qi_redis_hget(
    句柄: i64, 键: *const c_char, 字段: *const c_char
) -> *mut c_char {
    let (k, f) = (读C串(键), 读C串(字段));
    做可选串(句柄, |c| c.hget::<_, _, Option<Vec<u8>>>(&k, &f))
}

/// HGETALL → JSON 对象。字段/值非 UTF-8 的那几项会被跳过（不是整条失败）。
#[no_mangle]
pub extern "C" fn qi_redis_hgetall(句柄: i64, 键: *const c_char) -> *mut c_char {
    let k = 读C串(键);
    做JSON(句柄, |c| {
        let 表: HashMap<String, String> = c.hgetall(&k)?;
        let mut 出 = JsonMap::new();
        for (字段, 值) in 表 {
            出.insert(字段, JsonValue::String(值));
        }
        Ok(JsonValue::Object(出))
    })
}

#[no_mangle]
pub extern "C" fn qi_redis_hdel(句柄: i64, 键: *const c_char, 字段: *const c_char) -> i64 {
    let (k, f) = (读C串(键), 读C串(字段));
    做整数(句柄, |c| c.hdel::<_, _, i64>(&k, &f))
}

// ── 列表 ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn qi_redis_lpush(句柄: i64, 键: *const c_char, 值: *const c_char) -> i64 {
    let (k, v) = (读C串(键), 读C串(值));
    做整数(句柄, |c| c.lpush::<_, _, i64>(&k, &v))
}

#[no_mangle]
pub extern "C" fn qi_redis_rpush(句柄: i64, 键: *const c_char, 值: *const c_char) -> i64 {
    let (k, v) = (读C串(键), 读C串(值));
    做整数(句柄, |c| c.rpush::<_, _, i64>(&k, &v))
}

#[no_mangle]
pub extern "C" fn qi_redis_lpop(句柄: i64, 键: *const c_char) -> *mut c_char {
    let k = 读C串(键);
    做可选串(句柄, |c| c.lpop::<_, Option<Vec<u8>>>(&k, None))
}

#[no_mangle]
pub extern "C" fn qi_redis_rpop(句柄: i64, 键: *const c_char) -> *mut c_char {
    let k = 读C串(键);
    做可选串(句柄, |c| c.rpop::<_, Option<Vec<u8>>>(&k, None))
}

#[no_mangle]
pub extern "C" fn qi_redis_llen(句柄: i64, 键: *const c_char) -> i64 {
    let k = 读C串(键);
    做整数(句柄, |c| c.llen::<_, i64>(&k))
}

/// LRANGE → JSON 数组。止 = -1 表示到末尾。
#[no_mangle]
pub extern "C" fn qi_redis_lrange(
    句柄: i64, 键: *const c_char, 起: i64, 止: i64
) -> *mut c_char {
    let k = 读C串(键);
    做JSON(句柄, move |c| {
        let 项: Vec<String> = c.lrange(&k, 起 as isize, 止 as isize)?;
        Ok(JsonValue::Array(
            项.into_iter().map(JsonValue::String).collect(),
        ))
    })
}

// ── 集合 ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn qi_redis_sadd(句柄: i64, 键: *const c_char, 成员: *const c_char) -> i64 {
    let (k, m) = (读C串(键), 读C串(成员));
    做整数(句柄, |c| c.sadd::<_, _, i64>(&k, &m))
}

#[no_mangle]
pub extern "C" fn qi_redis_srem(句柄: i64, 键: *const c_char, 成员: *const c_char) -> i64 {
    let (k, m) = (读C串(键), 读C串(成员));
    做整数(句柄, |c| c.srem::<_, _, i64>(&k, &m))
}

#[no_mangle]
pub extern "C" fn qi_redis_sismember(
    句柄: i64, 键: *const c_char, 成员: *const c_char
) -> i64 {
    let (k, m) = (读C串(键), 读C串(成员));
    做整数(句柄, |c| {
        c.sismember::<_, _, bool>(&k, &m)
            .map(|有| if 有 { 1 } else { 0 })
    })
}

#[no_mangle]
pub extern "C" fn qi_redis_smembers(句柄: i64, 键: *const c_char) -> *mut c_char {
    let k = 读C串(键);
    做JSON(句柄, |c| {
        let 成员: Vec<String> = c.smembers(&k)?;
        Ok(JsonValue::Array(
            成员.into_iter().map(JsonValue::String).collect(),
        ))
    })
}

// ── 扫描 ────────────────────────────────────────────────────────

/// 按模式列键 → JSON 数组。
///
/// 内部走 **SCAN 游标**而不是 KEYS：KEYS 在大库上是一次全表遍历，
/// 单线程的 Redis 会被它卡住整整几百毫秒，把整个应用一起拖下水。
/// `上限` 是给自己的刹车，扫够了就停（<=0 时按 1000 算）。
#[no_mangle]
pub extern "C" fn qi_redis_scan(句柄: i64, 模式: *const c_char, 上限: i64) -> *mut c_char {
    let p = 读C串(模式);
    let 顶 = if 上限 <= 0 { 1000 } else { 上限 as usize };
    做JSON(句柄, move |c| {
        let mut 游标: u64 = 0;
        let mut 出: Vec<JsonValue> = Vec::new();
        loop {
            let (下一个, 这批): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(游标)
                .arg("MATCH")
                .arg(&p)
                .arg("COUNT")
                .arg(200)
                .query(c)?;
            for 键 in 这批 {
                if 出.len() >= 顶 {
                    return Ok(JsonValue::Array(出));
                }
                出.push(JsonValue::String(键));
            }
            游标 = 下一个;
            if 游标 == 0 {
                return Ok(JsonValue::Array(出));
            }
        }
    })
}

// ── 发布订阅 ────────────────────────────────────────────────────

/// PUBLISH，返回收到的订阅者数（0 表示当下没人听着，不是错）。
#[no_mangle]
pub extern "C" fn qi_redis_publish(
    句柄: i64, 频道: *const c_char, 消息: *const c_char
) -> i64 {
    let (ch, msg) = (读C串(频道), 读C串(消息));
    做整数(句柄, |c| c.publish::<_, _, i64>(&ch, &msg))
}

/// 订阅一批频道，返回订阅句柄；失败 -1。
///
/// ── 为什么要专起一个读线程 ──────────────────────────────────
///
/// redis-rs 有两条读路径，差别是致命的：
///   `Connection::recv_response()`  = read(**true**)  —— 请求/应答用
///   `PubSub::get_message()`        = read(**false**) —— 订阅用
///
/// read(true) 在**超时**时会 `messages_to_skip += 1`：它以为这是一次超时的
/// 请求，那条应答迟早会来、要丢掉。于是接下来的真消息被当成迟到的应答
/// **静默吃掉**。第一版就是拿 recv_response 写的，症状是「空收几次之后
/// 就再也收不到东西」，而且一声不吭 —— 单测里超时排在最后一步，正好没照到。
///
/// 但 read(false) 只能通过 `PubSub` 拿到，而 `PubSub` 的 Drop 会退订。
/// 要长期持有订阅，守卫就不能析构 —— 所以让它活在一个专职读线程的栈上：
/// 线程从头到尾攥着守卫阻塞读，读到就塞进队列；qi 那边只跟队列打交道。
///
/// 附带好处：qi 侧的 订阅接收 不再碰 socket，多个 goroutine 同时收也不会
/// 互相抢那条连接。
///
/// 频道用逗号分隔：`"房间:1,房间:2"`。
#[no_mangle]
pub extern "C" fn qi_redis_subscribe(url: *const c_char, 频道表: *const c_char) -> i64 {
    let 原串 = 读C串(url);
    let 频道串 = 读C串(频道表);
    if 原串.is_empty() || 频道串.is_empty() {
        return -1;
    }
    let (干净串, _) = 拆池参数(&原串);
    let 频道: Vec<String> = 频道串
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if 频道.is_empty() {
        return -1;
    }

    let 状态 = Arc::new(订阅 {
        队: Mutex::new(VecDeque::new()),
        有货: Condvar::new(),
        要停: AtomicBool::new(false),
        活着: AtomicBool::new(false),
    });
    let 给线程 = 状态.clone();
    // 连不上要返回 -1，所以在这儿等线程把「订上了没有」回话
    let (回信, 等回信) = std::sync::mpsc::channel::<bool>();
    std::thread::spawn(move || 订阅线程(干净串, 频道, 给线程, 回信));

    match 等回信.recv_timeout(Duration::from_secs(10)) {
        Ok(true) => {}
        _ => return -1,
    }

    let 句柄 = 订阅计数器.fetch_add(1, Ordering::SeqCst);
    取订阅表()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(句柄, 状态);
    句柄
}

/// 读线程：攥着 PubSub 守卫读到死。
///
/// 读超时设 5 秒，纯粹是为了周期性看一眼「该停了吗」——不设的话 退订()
/// 之后这个线程永远钉在 read 上。超时对 read(false) 是无害的（不碰
/// messages_to_skip）。
///
/// 除超时以外的任何错误都当成这条连接废了：置 活着=false 退出，让 qi 侧
/// 能靠 订阅活着() 发现并重订。**不在这儿自动重连** —— 重连要重放哪些
/// 消息、算不算丢，是业务决定，运行时替它决定只会掩盖问题。
fn 订阅线程(
    连接串: String,
    频道: Vec<String>,
    状态: Arc<订阅>,
    回信: std::sync::mpsc::Sender<bool>,
) {
    let 客户端 = match redis::Client::open(连接串.as_str()) {
        Ok(c) => c,
        Err(_) => {
            let _ = 回信.send(false);
            return;
        }
    };
    let mut 连 = match 客户端.get_connection() {
        Ok(c) => c,
        Err(_) => {
            let _ = 回信.send(false);
            return;
        }
    };

    let mut ps = 连.as_pubsub();
    for 一个 in &频道 {
        if ps.subscribe(一个).is_err() {
            let _ = 回信.send(false);
            return;
        }
    }
    if ps.set_read_timeout(Some(Duration::from_secs(5))).is_err() {
        let _ = 回信.send(false);
        return;
    }
    状态.活着.store(true, Ordering::SeqCst);
    let _ = 回信.send(true);

    loop {
        if 状态.要停.load(Ordering::SeqCst) {
            break;
        }
        match ps.get_message() {
            Ok(消息) => {
                let 频道名 = 消息.get_channel_name().to_string();
                let 载荷: String = 消息.get_payload().unwrap_or_default();
                let mut 队 = 状态.队.lock().unwrap_or_else(|e| e.into_inner());
                // 队满了丢**最旧**的：这条路上跑的是页面重渲染帧，
                // 消费者卡住时最新那帧才是对的，留着一堆过期的没有意义。
                if 队.len() >= 订阅队上限 {
                    队.pop_front();
                }
                队.push_back((频道名, 载荷));
                drop(队);
                状态.有货.notify_all();
            }
            Err(e) => {
                // 超时是正常的（就是为了回来看 要停），其余都算连接废了
                if e.is_timeout() {
                    continue;
                }
                break;
            }
        }
    }
    状态.活着.store(false, Ordering::SeqCst);
    状态.有货.notify_all();
}

/// 收一条消息，最多等 `超时毫秒`。
///
/// 收到返回 `{"频道":"…","消息":"…"}`；**超时返回空串**（不是错，接着调就行）。
/// 这个形状是照 WebSocket.接收文本超时 来的：有超时才能让一个循环在等消息的
/// 间隙腾出手干别的（查邮箱、跑定时器），否则一订阅就把那条线程钉死了。
#[no_mangle]
pub extern "C" fn qi_redis_sub_recv(订阅句柄: i64, 超时毫秒: i64) -> *mut c_char {
    let Some(状态) = 取订阅表()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&订阅句柄)
        .cloned()
    else {
        return 空串();
    };

    let mut 队 = 状态.队.lock().unwrap_or_else(|e| e.into_inner());
    if 队.is_empty() {
        // 虚假唤醒会让这里比要求的更早返回空串。无所谓 —— 调用方本来就把
        // 空串当「这次没有」，接着调就是了，不值得为它多套一层重算截止时间。
        let (新队, _) = 状态
            .有货
            .wait_timeout(队, Duration::from_millis(超时毫秒.max(1) as u64))
            .unwrap_or_else(|e| e.into_inner());
        队 = 新队;
    }

    match 队.pop_front() {
        Some((频道名, 载荷)) => {
            drop(队);
            let mut 出 = JsonMap::new();
            出.insert("频道".to_string(), JsonValue::String(频道名));
            出.insert("消息".to_string(), JsonValue::String(载荷));
            出串(JsonValue::Object(出).to_string())
        }
        None => 空串(),
    }
}

/// 这条订阅还活着吗。1 活着，0 已经断了（或句柄无效）。
///
/// 长跑的中继循环该定期看一眼：连接断了 订阅接收 只会一直返回空串，
/// 跟「没人发消息」长得一模一样，不查这个就永远发现不了。
#[no_mangle]
pub extern "C" fn qi_redis_sub_alive(订阅句柄: i64) -> i64 {
    match 取订阅表()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&订阅句柄)
    {
        Some(状态) => {
            if 状态.活着.load(Ordering::SeqCst) {
                1
            } else {
                0
            }
        }
        None => 0,
    }
}

/// 退订并关掉那条连接。读线程最多再过一个读超时（5 秒）就退出。
#[no_mangle]
pub extern "C" fn qi_redis_unsubscribe(订阅句柄: i64) -> i64 {
    match 取订阅表()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&订阅句柄)
    {
        Some(状态) => {
            状态.要停.store(true, Ordering::SeqCst);
            0
        }
        None => -1,
    }
}

// ── 逃生口 ──────────────────────────────────────────────────────

/// 跑任意命令：参数是 JSON 数组，`["SETRANGE","k","5","hi"]`。
///
/// 有它在，我们没包到的命令（SETRANGE、GETDEL、Lua 脚本、ZSET 全家…）
/// 不用等下一版运行时。返回值按 Redis 的回包类型转 JSON：
/// 整数→数字，简单串/大块串→字符串，数组→数组，nil→null。
#[no_mangle]
pub extern "C" fn qi_redis_command(句柄: i64, 参数JSON: *const c_char) -> *mut c_char {
    let 参数串 = 读C串(参数JSON);
    做JSON(句柄, move |c| {
        let 解析: JsonValue = serde_json::from_str(&参数串).unwrap_or(JsonValue::Null);
        let JsonValue::Array(项) = 解析 else {
            return Err(redis::RedisError::from((
                redis::ErrorKind::Client,
                "参数要是 JSON 数组",
            )));
        };
        let mut 词: Vec<String> = Vec::new();
        for 一个 in 项 {
            词.push(match 一个 {
                JsonValue::String(s) => s,
                JsonValue::Number(n) => n.to_string(),
                JsonValue::Bool(b) => b.to_string(),
                其他 => 其他.to_string(),
            });
        }
        if 词.is_empty() {
            return Err(redis::RedisError::from((
                redis::ErrorKind::Client,
                "命令是空的",
            )));
        }
        let mut 命令 = redis::cmd(&词[0]);
        for 一个 in &词[1..] {
            命令.arg(一个);
        }
        let 值 = 命令.query::<redis::Value>(c)?;
        Ok(转JSON(&值))
    })
}

/// Redis 回包 → JSON。非 UTF-8 的大块串转成 null（而不是有损转换）。
fn 转JSON(值: &redis::Value) -> JsonValue {
    match 值 {
        redis::Value::Nil => JsonValue::Null,
        redis::Value::Int(n) => JsonValue::Number((*n).into()),
        redis::Value::BulkString(字节) => match std::str::from_utf8(字节) {
            Ok(s) => JsonValue::String(s.to_string()),
            Err(_) => JsonValue::Null,
        },
        redis::Value::Array(项) => JsonValue::Array(项.iter().map(转JSON).collect()),
        redis::Value::SimpleString(s) => JsonValue::String(s.clone()),
        redis::Value::Okay => JsonValue::String("OK".to_string()),
        redis::Value::Map(对) => {
            let mut 出 = JsonMap::new();
            for (k, v) in 对 {
                let 键 = match 转JSON(k) {
                    JsonValue::String(s) => s,
                    其他 => 其他.to_string(),
                };
                出.insert(键, 转JSON(v));
            }
            JsonValue::Object(出)
        }
        redis::Value::Set(项) => JsonValue::Array(项.iter().map(转JSON).collect()),
        redis::Value::Double(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        redis::Value::Boolean(b) => JsonValue::Bool(*b),
        其他 => JsonValue::String(format!("{:?}", 其他)),
    }
}
