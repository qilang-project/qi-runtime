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
static NEXT_CONN_ID: AtomicI64 = AtomicI64::new(3_000_000);
static NEXT_SUB_ID: AtomicI64 = AtomicI64::new(3_500_000);

static CONNS: OnceLock<Mutex<HashMap<i64, Arc<Conn>>>> = OnceLock::new();
static SUBS: OnceLock<Mutex<HashMap<i64, Arc<Sub>>>> = OnceLock::new();

/// 一条订阅最多攒多少条没被取走的消息。攒到这儿说明消费方卡住了。
const SUB_QUEUE_LIMIT: usize = 4096;

struct Conn {
    pool: r2d2::Pool<redis::Client>,
    last_error: Mutex<String>,
}

/// 一条订阅：读线程往队里塞，qi 侧从队里取。socket 只有读线程碰。
struct Sub {
    queue: Mutex<VecDeque<(String, String)>>,
    ready: Condvar,
    stopping: AtomicBool,
    alive: AtomicBool,
}

struct PoolConfig {
    max_size: u32,
    timeout: Duration,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_size: 8,
            timeout: Duration::from_millis(5000),
        }
    }
}

fn conns() -> &'static Mutex<HashMap<i64, Arc<Conn>>> {
    CONNS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn subs() -> &'static Mutex<HashMap<i64, Arc<Sub>>> {
    SUBS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn find_conn(id: i64) -> Option<Arc<Conn>> {
    conns()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&id)
        .cloned()
}

fn read_cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p).to_string_lossy().to_string() }
}

fn out_str(s: String) -> *mut c_char {
    rc_cstr_from_string(s)
}

fn empty_str() -> *mut c_char {
    out_str(String::new())
}

impl Conn {
    fn set_error(&self, message: impl Into<String>) {
        *self.last_error.lock().unwrap_or_else(|e| e.into_inner()) = message.into();
    }

    fn clear_error(&self) {
        self.last_error
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    /// 借一条连接干活。池子拿不到连接（全忙 / Redis 挂了）就记错返回 None。
    fn borrow_conn(&self) -> Option<r2d2::PooledConnection<redis::Client>> {
        match self.pool.get() {
            Ok(c) => Some(c),
            Err(e) => {
                self.set_error(format!("取连接失败: {}", e));
                None
            }
        }
    }
}

/// 池参数跟数据库层同一套写法：`redis://127.0.0.1:46379/0?pool_max=16`。
/// `pool_` 前缀的参数在交给 redis crate **之前**摘掉 —— 它见到不认识的
/// 查询参数会直接拒绝解析 URL。
fn split_pool_params(conn_str: &str) -> (String, PoolConfig) {
    let mut config = PoolConfig::default();
    let Some((prefix, query)) = conn_str.split_once('?') else {
        return (conn_str.to_string(), config);
    };

    let mut for_driver: Vec<&str> = Vec::new();
    for item in query.split('&').filter(|item| !item.is_empty()) {
        let (key, value) = item.split_once('=').unwrap_or((item, ""));
        match key {
            // 写错一个池参数不该让整个应用连不上库 —— 解析不出就用默认值
            "pool_max" => {
                if let Ok(n) = value.parse::<u32>() {
                    config.max_size = n.max(1);
                }
            }
            "pool_timeout_ms" => {
                if let Ok(ms) = value.parse::<u64>() {
                    config.timeout = Duration::from_millis(ms.max(1));
                }
            }
            _ => for_driver.push(item),
        }
    }

    let clean = if for_driver.is_empty() {
        prefix.to_string()
    } else {
        format!("{}?{}", prefix, for_driver.join("&"))
    };
    (clean, config)
}

// ── 连接 ────────────────────────────────────────────────────────

/// 连接（建池）。失败返回 -1。
///
/// `redis://` 明文，`rediss://` 走 TLS。**建池时就连一条**（min_idle=1），
/// 所以「Redis 没起来」在 连接() 这一步就报，而不是等第一次 取() 才炸。
#[no_mangle]
pub extern "C" fn qi_redis_connect(url: *const c_char) -> i64 {
    let raw = read_cstr(url);
    if raw.is_empty() {
        return -1;
    }
    let (clean, config) = split_pool_params(&raw);

    let client = match redis::Client::open(clean.as_str()) {
        Ok(c) => c,
        Err(_) => return -1,
    };
    let pool = match r2d2::Pool::builder()
        .max_size(config.max_size)
        .min_idle(Some(1))
        .connection_timeout(config.timeout)
        .build(client)
    {
        Ok(p) => p,
        Err(_) => return -1,
    };

    let id = NEXT_CONN_ID.fetch_add(1, Ordering::SeqCst);
    conns().lock().unwrap_or_else(|e| e.into_inner()).insert(
        id,
        Arc::new(Conn {
            pool,
            last_error: Mutex::new(String::new()),
        }),
    );
    id
}

/// 关闭：把句柄从表里摘掉，池随最后一个使用者析构。
#[no_mangle]
pub extern "C" fn qi_redis_close(id: i64) -> i64 {
    match conns()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&id)
    {
        Some(_) => 0,
        None => -1,
    }
}

/// PING。通返回 1，不通返回 0。
#[no_mangle]
pub extern "C" fn qi_redis_ping(id: i64) -> i64 {
    let Some(conn) = find_conn(id) else {
        return 0;
    };
    let Some(mut c) = conn.borrow_conn() else {
        return 0;
    };
    match redis::cmd("PING").query::<String>(&mut *c) {
        Ok(resp) if resp == "PONG" => {
            conn.clear_error();
            1
        }
        Ok(resp) => {
            conn.set_error(format!("PING 回了 {}", resp));
            0
        }
        Err(e) => {
            conn.set_error(e.to_string());
            0
        }
    }
}

/// 最近一次失败的说明。没有错误时返回空串。
#[no_mangle]
pub extern "C" fn qi_redis_last_error(id: i64) -> *mut c_char {
    let Some(conn) = find_conn(id) else {
        return out_str("句柄无效".to_string());
    };
    let err_text = conn
        .last_error
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .clone();
    out_str(err_text)
}

// ── 内部：把一次命令的结果收成 i64 / String ──────────────────────
//
// 每个 FFI 都要「查句柄 → 借连接 → 跑命令 → 记错」这四步，抽出来免得
// 二十几个函数各写一遍（写多了必然有一处忘了记错）。

fn do_int<F>(id: i64, work: F) -> i64
where
    F: FnOnce(&mut redis::Connection) -> redis::RedisResult<i64>,
{
    let Some(conn) = find_conn(id) else {
        return -1;
    };
    let Some(mut c) = conn.borrow_conn() else {
        return -1;
    };
    match work(&mut c) {
        Ok(v) => {
            conn.clear_error();
            v
        }
        Err(e) => {
            conn.set_error(e.to_string());
            -1
        }
    }
}

/// 字符串类结果。键不存在返回空串（**不算错**，最后错误会被清掉），
/// 值不是合法 UTF-8 也返回空串但记错 —— 两者靠 最后错误 区分。
fn do_opt_str<F>(id: i64, work: F) -> *mut c_char
where
    F: FnOnce(&mut redis::Connection) -> redis::RedisResult<Option<Vec<u8>>>,
{
    let Some(conn) = find_conn(id) else {
        return empty_str();
    };
    let Some(mut c) = conn.borrow_conn() else {
        return empty_str();
    };
    match work(&mut c) {
        Ok(None) => {
            conn.clear_error();
            empty_str()
        }
        Ok(Some(raw_bytes)) => match String::from_utf8(raw_bytes) {
            Ok(s) => {
                conn.clear_error();
                out_str(s)
            }
            Err(_) => {
                conn.set_error("值不是合法 UTF-8（要存二进制请先自己 base64）");
                empty_str()
            }
        },
        Err(e) => {
            conn.set_error(e.to_string());
            empty_str()
        }
    }
}

fn do_json<F>(id: i64, work: F) -> *mut c_char
where
    F: FnOnce(&mut redis::Connection) -> redis::RedisResult<JsonValue>,
{
    let Some(conn) = find_conn(id) else {
        return out_str("[]".to_string());
    };
    let Some(mut c) = conn.borrow_conn() else {
        return out_str("[]".to_string());
    };
    match work(&mut c) {
        Ok(v) => {
            conn.clear_error();
            out_str(v.to_string())
        }
        Err(e) => {
            conn.set_error(e.to_string());
            out_str("[]".to_string())
        }
    }
}

// ── 字符串键值 ──────────────────────────────────────────────────

/// SET。成功 1，失败 -1。
#[no_mangle]
pub extern "C" fn qi_redis_set(id: i64, key: *const c_char, value: *const c_char) -> i64 {
    let (k, v) = (read_cstr(key), read_cstr(value));
    do_int(id, |c| c.set::<_, _, ()>(&k, &v).map(|_| 1))
}

/// SET + 过期秒数（SETEX）。会话、验证码这类东西该用这个，
/// 而不是 设() 完再 设过期() —— 那中间断一下就留下一个永不过期的键。
#[no_mangle]
pub extern "C" fn qi_redis_set_ex(
    id: i64,
    key: *const c_char,
    value: *const c_char,
    secs: i64,
) -> i64 {
    let (k, v) = (read_cstr(key), read_cstr(value));
    if secs <= 0 {
        return -1;
    }
    do_int(id, move |c| {
        c.set_ex::<_, _, ()>(&k, &v, secs as u64).map(|_| 1)
    })
}

/// SET NX EX —— 抢到返回 1，已被别人占着返回 0，出错 -1。
/// 这是分布式锁 / 「同一本书只让一个进程做」那类事情的基本件。
#[no_mangle]
pub extern "C" fn qi_redis_set_nx(
    id: i64,
    key: *const c_char,
    value: *const c_char,
    secs: i64,
) -> i64 {
    let (k, v) = (read_cstr(key), read_cstr(value));
    do_int(id, move |c| {
        let mut cmd = redis::cmd("SET");
        cmd.arg(&k).arg(&v).arg("NX");
        if secs > 0 {
            cmd.arg("EX").arg(secs);
        }
        // 没抢到时 Redis 回 nil，收成 Option
        cmd.query::<Option<String>>(c)
            .map(|resp| if resp.is_some() { 1 } else { 0 })
    })
}

/// GET。键不存在返回空串 —— 要区分「不存在」和「存的就是空串」用 存在()。
#[no_mangle]
pub extern "C" fn qi_redis_get(id: i64, key: *const c_char) -> *mut c_char {
    let k = read_cstr(key);
    do_opt_str(id, |c| c.get::<_, Option<Vec<u8>>>(&k))
}

/// DEL，返回真删掉几个。
#[no_mangle]
pub extern "C" fn qi_redis_del(id: i64, key: *const c_char) -> i64 {
    let k = read_cstr(key);
    do_int(id, |c| c.del::<_, i64>(&k))
}

/// EXISTS，1/0。
#[no_mangle]
pub extern "C" fn qi_redis_exists(id: i64, key: *const c_char) -> i64 {
    let k = read_cstr(key);
    do_int(id, |c| {
        c.exists::<_, bool>(&k)
            .map(|exists| if exists { 1 } else { 0 })
    })
}

/// INCR，返回自增后的新值。**新值本身可能是负数**，所以别拿 -1 当失败判据，
/// 拿不准查 最后错误()。
#[no_mangle]
pub extern "C" fn qi_redis_incr(id: i64, key: *const c_char) -> i64 {
    let k = read_cstr(key);
    do_int(id, |c| c.incr::<_, i64, i64>(&k, 1))
}

/// INCRBY（增量可负，就是 DECRBY）。
#[no_mangle]
pub extern "C" fn qi_redis_incr_by(id: i64, key: *const c_char, delta: i64) -> i64 {
    let k = read_cstr(key);
    do_int(id, move |c| c.incr::<_, i64, i64>(&k, delta))
}

/// EXPIRE。设上了返回 1，键不存在返回 0。
#[no_mangle]
pub extern "C" fn qi_redis_expire(id: i64, key: *const c_char, secs: i64) -> i64 {
    let k = read_cstr(key);
    do_int(id, move |c| {
        c.expire::<_, bool>(&k, secs)
            .map(|ok| if ok { 1 } else { 0 })
    })
}

/// TTL：剩余秒数；-1 没设过期；-2 键不存在。
#[no_mangle]
pub extern "C" fn qi_redis_ttl(id: i64, key: *const c_char) -> i64 {
    let k = read_cstr(key);
    do_int(id, |c| c.ttl::<_, i64>(&k))
}

// ── 哈希 ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn qi_redis_hset(
    id: i64,
    key: *const c_char,
    field: *const c_char,
    value: *const c_char,
) -> i64 {
    let (k, f, v) = (read_cstr(key), read_cstr(field), read_cstr(value));
    do_int(id, |c| c.hset::<_, _, _, ()>(&k, &f, &v).map(|_| 1))
}

#[no_mangle]
pub extern "C" fn qi_redis_hget(id: i64, key: *const c_char, field: *const c_char) -> *mut c_char {
    let (k, f) = (read_cstr(key), read_cstr(field));
    do_opt_str(id, |c| c.hget::<_, _, Option<Vec<u8>>>(&k, &f))
}

/// HGETALL → JSON 对象。字段/值非 UTF-8 的那几项会被跳过（不是整条失败）。
#[no_mangle]
pub extern "C" fn qi_redis_hgetall(id: i64, key: *const c_char) -> *mut c_char {
    let k = read_cstr(key);
    do_json(id, |c| {
        let table: HashMap<String, String> = c.hgetall(&k)?;
        let mut out = JsonMap::new();
        for (field, value) in table {
            out.insert(field, JsonValue::String(value));
        }
        Ok(JsonValue::Object(out))
    })
}

#[no_mangle]
pub extern "C" fn qi_redis_hdel(id: i64, key: *const c_char, field: *const c_char) -> i64 {
    let (k, f) = (read_cstr(key), read_cstr(field));
    do_int(id, |c| c.hdel::<_, _, i64>(&k, &f))
}

// ── 列表 ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn qi_redis_lpush(id: i64, key: *const c_char, value: *const c_char) -> i64 {
    let (k, v) = (read_cstr(key), read_cstr(value));
    do_int(id, |c| c.lpush::<_, _, i64>(&k, &v))
}

#[no_mangle]
pub extern "C" fn qi_redis_rpush(id: i64, key: *const c_char, value: *const c_char) -> i64 {
    let (k, v) = (read_cstr(key), read_cstr(value));
    do_int(id, |c| c.rpush::<_, _, i64>(&k, &v))
}

#[no_mangle]
pub extern "C" fn qi_redis_lpop(id: i64, key: *const c_char) -> *mut c_char {
    let k = read_cstr(key);
    do_opt_str(id, |c| c.lpop::<_, Option<Vec<u8>>>(&k, None))
}

#[no_mangle]
pub extern "C" fn qi_redis_rpop(id: i64, key: *const c_char) -> *mut c_char {
    let k = read_cstr(key);
    do_opt_str(id, |c| c.rpop::<_, Option<Vec<u8>>>(&k, None))
}

#[no_mangle]
pub extern "C" fn qi_redis_llen(id: i64, key: *const c_char) -> i64 {
    let k = read_cstr(key);
    do_int(id, |c| c.llen::<_, i64>(&k))
}

/// LRANGE → JSON 数组。止 = -1 表示到末尾。
#[no_mangle]
pub extern "C" fn qi_redis_lrange(
    id: i64,
    key: *const c_char,
    start: i64,
    stop: i64,
) -> *mut c_char {
    let k = read_cstr(key);
    do_json(id, move |c| {
        let items: Vec<String> = c.lrange(&k, start as isize, stop as isize)?;
        Ok(JsonValue::Array(
            items.into_iter().map(JsonValue::String).collect(),
        ))
    })
}

// ── 集合 ────────────────────────────────────────────────────────

#[no_mangle]
pub extern "C" fn qi_redis_sadd(id: i64, key: *const c_char, member: *const c_char) -> i64 {
    let (k, m) = (read_cstr(key), read_cstr(member));
    do_int(id, |c| c.sadd::<_, _, i64>(&k, &m))
}

#[no_mangle]
pub extern "C" fn qi_redis_srem(id: i64, key: *const c_char, member: *const c_char) -> i64 {
    let (k, m) = (read_cstr(key), read_cstr(member));
    do_int(id, |c| c.srem::<_, _, i64>(&k, &m))
}

#[no_mangle]
pub extern "C" fn qi_redis_sismember(id: i64, key: *const c_char, member: *const c_char) -> i64 {
    let (k, m) = (read_cstr(key), read_cstr(member));
    do_int(id, |c| {
        c.sismember::<_, _, bool>(&k, &m)
            .map(|exists| if exists { 1 } else { 0 })
    })
}

#[no_mangle]
pub extern "C" fn qi_redis_smembers(id: i64, key: *const c_char) -> *mut c_char {
    let k = read_cstr(key);
    do_json(id, |c| {
        let member: Vec<String> = c.smembers(&k)?;
        Ok(JsonValue::Array(
            member.into_iter().map(JsonValue::String).collect(),
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
pub extern "C" fn qi_redis_scan(id: i64, pattern: *const c_char, limit: i64) -> *mut c_char {
    let p = read_cstr(pattern);
    let cap = if limit <= 0 { 1000 } else { limit as usize };
    do_json(id, move |c| {
        let mut cursor: u64 = 0;
        let mut out: Vec<JsonValue> = Vec::new();
        loop {
            let (next_cursor, batch): (u64, Vec<String>) = redis::cmd("SCAN")
                .arg(cursor)
                .arg("MATCH")
                .arg(&p)
                .arg("COUNT")
                .arg(200)
                .query(c)?;
            for key in batch {
                if out.len() >= cap {
                    return Ok(JsonValue::Array(out));
                }
                out.push(JsonValue::String(key));
            }
            cursor = next_cursor;
            if cursor == 0 {
                return Ok(JsonValue::Array(out));
            }
        }
    })
}

// ── 发布订阅 ────────────────────────────────────────────────────

/// PUBLISH，返回收到的订阅者数（0 表示当下没人听着，不是错）。
#[no_mangle]
pub extern "C" fn qi_redis_publish(id: i64, channels: *const c_char, msg: *const c_char) -> i64 {
    let (ch, msg) = (read_cstr(channels), read_cstr(msg));
    do_int(id, |c| c.publish::<_, _, i64>(&ch, &msg))
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
pub extern "C" fn qi_redis_subscribe(url: *const c_char, channel_list: *const c_char) -> i64 {
    let raw = read_cstr(url);
    let channels_text = read_cstr(channel_list);
    if raw.is_empty() || channels_text.is_empty() {
        return -1;
    }
    let (clean, _) = split_pool_params(&raw);
    let channels: Vec<String> = channels_text
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    if channels.is_empty() {
        return -1;
    }

    let state = Arc::new(Sub {
        queue: Mutex::new(VecDeque::new()),
        ready: Condvar::new(),
        stopping: AtomicBool::new(false),
        alive: AtomicBool::new(false),
    });
    let for_thread = state.clone();
    // 连不上要返回 -1，所以在这儿等线程把「订上了没有」回话
    let (reply, wait_reply) = std::sync::mpsc::channel::<bool>();
    std::thread::spawn(move || sub_thread(clean, channels, for_thread, reply));

    match wait_reply.recv_timeout(Duration::from_secs(10)) {
        Ok(true) => {}
        _ => return -1,
    }

    let id = NEXT_SUB_ID.fetch_add(1, Ordering::SeqCst);
    subs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .insert(id, state);
    id
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
fn sub_thread(
    conn_str: String,
    channels: Vec<String>,
    state: Arc<Sub>,
    reply: std::sync::mpsc::Sender<bool>,
) {
    let client = match redis::Client::open(conn_str.as_str()) {
        Ok(c) => c,
        Err(_) => {
            let _ = reply.send(false);
            return;
        }
    };
    let mut conn = match client.get_connection() {
        Ok(c) => c,
        Err(_) => {
            let _ = reply.send(false);
            return;
        }
    };

    let mut ps = conn.as_pubsub();
    for one in &channels {
        if ps.subscribe(one).is_err() {
            let _ = reply.send(false);
            return;
        }
    }
    if ps.set_read_timeout(Some(Duration::from_secs(5))).is_err() {
        let _ = reply.send(false);
        return;
    }
    state.alive.store(true, Ordering::SeqCst);
    let _ = reply.send(true);

    loop {
        if state.stopping.load(Ordering::SeqCst) {
            break;
        }
        match ps.get_message() {
            Ok(msg) => {
                let channel_name = msg.get_channel_name().to_string();
                let payload: String = msg.get_payload().unwrap_or_default();
                let mut queue = state.queue.lock().unwrap_or_else(|e| e.into_inner());
                // 队满了丢**最旧**的：这条路上跑的是页面重渲染帧，
                // 消费者卡住时最新那帧才是对的，留着一堆过期的没有意义。
                if queue.len() >= SUB_QUEUE_LIMIT {
                    queue.pop_front();
                }
                queue.push_back((channel_name, payload));
                drop(queue);
                state.ready.notify_all();
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
    state.alive.store(false, Ordering::SeqCst);
    state.ready.notify_all();
}

/// 收一条消息，最多等 `超时毫秒`。
///
/// 收到返回 `{"频道":"…","消息":"…"}`；**超时返回空串**（不是错，接着调就行）。
/// 这个形状是照 WebSocket.接收文本超时 来的：有超时才能让一个循环在等消息的
/// 间隙腾出手干别的（查邮箱、跑定时器），否则一订阅就把那条线程钉死了。
#[no_mangle]
pub extern "C" fn qi_redis_sub_recv(sub_id: i64, timeout_ms: i64) -> *mut c_char {
    let Some(state) = subs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&sub_id)
        .cloned()
    else {
        return empty_str();
    };

    let mut queue = state.queue.lock().unwrap_or_else(|e| e.into_inner());
    if queue.is_empty() {
        // 虚假唤醒会让这里比要求的更早返回空串。无所谓 —— 调用方本来就把
        // 空串当「这次没有」，接着调就是了，不值得为它多套一层重算截止时间。
        let (new_queue, _) = state
            .ready
            .wait_timeout(queue, Duration::from_millis(timeout_ms.max(1) as u64))
            .unwrap_or_else(|e| e.into_inner());
        queue = new_queue;
    }

    match queue.pop_front() {
        Some((channel_name, payload)) => {
            drop(queue);
            let mut out = JsonMap::new();
            out.insert("频道".to_string(), JsonValue::String(channel_name));
            out.insert("消息".to_string(), JsonValue::String(payload));
            out_str(JsonValue::Object(out).to_string())
        }
        None => empty_str(),
    }
}

/// 这条订阅还活着吗。1 活着，0 已经断了（或句柄无效）。
///
/// 长跑的中继循环该定期看一眼：连接断了 订阅接收 只会一直返回空串，
/// 跟「没人发消息」长得一模一样，不查这个就永远发现不了。
#[no_mangle]
pub extern "C" fn qi_redis_sub_alive(sub_id: i64) -> i64 {
    match subs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&sub_id)
    {
        Some(state) => {
            if state.alive.load(Ordering::SeqCst) {
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
pub extern "C" fn qi_redis_unsubscribe(sub_id: i64) -> i64 {
    match subs()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&sub_id)
    {
        Some(state) => {
            state.stopping.store(true, Ordering::SeqCst);
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
pub extern "C" fn qi_redis_command(id: i64, args_json: *const c_char) -> *mut c_char {
    let args_text = read_cstr(args_json);
    do_json(id, move |c| {
        let parsed: JsonValue = serde_json::from_str(&args_text).unwrap_or(JsonValue::Null);
        let JsonValue::Array(items) = parsed else {
            return Err(redis::RedisError::from((
                redis::ErrorKind::Client,
                "参数要是 JSON 数组",
            )));
        };
        let mut words: Vec<String> = Vec::new();
        for one in items {
            words.push(match one {
                JsonValue::String(s) => s,
                JsonValue::Number(n) => n.to_string(),
                JsonValue::Bool(b) => b.to_string(),
                other => other.to_string(),
            });
        }
        if words.is_empty() {
            return Err(redis::RedisError::from((
                redis::ErrorKind::Client,
                "命令是空的",
            )));
        }
        let mut cmd = redis::cmd(&words[0]);
        for one in &words[1..] {
            cmd.arg(one);
        }
        let value = cmd.query::<redis::Value>(c)?;
        Ok(value_to_json(&value))
    })
}

/// Redis 回包 → JSON。非 UTF-8 的大块串转成 null（而不是有损转换）。
fn value_to_json(value: &redis::Value) -> JsonValue {
    match value {
        redis::Value::Nil => JsonValue::Null,
        redis::Value::Int(n) => JsonValue::Number((*n).into()),
        redis::Value::BulkString(raw_bytes) => match std::str::from_utf8(raw_bytes) {
            Ok(s) => JsonValue::String(s.to_string()),
            Err(_) => JsonValue::Null,
        },
        redis::Value::Array(items) => JsonValue::Array(items.iter().map(value_to_json).collect()),
        redis::Value::SimpleString(s) => JsonValue::String(s.clone()),
        redis::Value::Okay => JsonValue::String("OK".to_string()),
        redis::Value::Map(pairs) => {
            let mut out = JsonMap::new();
            for (k, v) in pairs {
                let key = match value_to_json(k) {
                    JsonValue::String(s) => s,
                    other => other.to_string(),
                };
                out.insert(key, value_to_json(v));
            }
            JsonValue::Object(out)
        }
        redis::Value::Set(items) => JsonValue::Array(items.iter().map(value_to_json).collect()),
        redis::Value::Double(f) => serde_json::Number::from_f64(*f)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        redis::Value::Boolean(b) => JsonValue::Bool(*b),
        other => JsonValue::String(format!("{:?}", other)),
    }
}
