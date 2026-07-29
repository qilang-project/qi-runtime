//! 邮箱 FFI — 单消费者消息队列 + 定时投递
//!
//! 为「服务端主动推」提供地基（对标 Erlang 进程邮箱 / Phoenix `handle_info`）：
//! 一个长期循环（比如一条 WebSocket 连接）持有一个邮箱，任何线程都可以往里投消息，
//! 循环在等客户端事件的间隙把邮箱抽干、按自己的节奏处理。
//!
//! 为什么放运行时而不是用 JSON 句柄 + 同步锁在 qi 里拼：
//!   **生命周期**。定时器醒来时那条连接可能早断了。qi 侧无论怎么写，都存在
//!   「检查邮箱还活着」和「往邮箱里写」之间的窗口。这里句柄单调递增、永不复用，
//!   关闭就是从池里摘掉，迟到的投递查不到句柄直接返回 0 —— 没有窗口，也不泄漏。
//!
//! 消息形如 (名字, 载荷)，取出时序列化成 `{"m":"名字","p":"载荷"}`，
//! 载荷**当字符串**放（不是嵌套 JSON），所以载荷是什么文本都不会破坏这一层。

#![allow(non_snake_case)]

use std::collections::{BinaryHeap, HashMap, VecDeque};
use std::ffi::CStr;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// 单个邮箱能积压的消息上限。
///
/// 满了之后**拒收新消息**（投递返回 0），而不是丢最老的：邮箱堆积说明消费方卡住了，
/// 静默丢消息只会把问题挪到更难查的地方，让投递方自己知道推不进去。
const 邮箱上限: usize = 4096;

struct 邮箱 {
    队: Mutex<VecDeque<(String, String)>>,
}

/// 句柄从 900000 起，和 WS(1000+)/TCP 句柄错开，混用时一眼能看出是谁。
static 句柄计数器: AtomicI64 = AtomicI64::new(900_000);
static 邮箱池: OnceLock<Mutex<HashMap<i64, Arc<邮箱>>>> = OnceLock::new();

fn 取池() -> &'static Mutex<HashMap<i64, Arc<邮箱>>> {
    邮箱池.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 取邮箱(句柄: i64) -> Option<Arc<邮箱>> {
    取池()
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

/// 新建一个邮箱，返回句柄（永不复用）。
#[no_mangle]
pub extern "C" fn qi_mailbox_create() -> i64 {
    let 句柄 = 句柄计数器.fetch_add(1, Ordering::SeqCst);
    取池().lock().unwrap_or_else(|e| e.into_inner()).insert(
        句柄,
        Arc::new(邮箱 {
            队: Mutex::new(VecDeque::new()),
        }),
    );
    句柄
}

/// 投递一条消息。返回 1 成功；邮箱已关闭或积压满了返回 0。
#[no_mangle]
pub extern "C" fn qi_mailbox_post(
    句柄: i64, 名字: *const c_char, 载荷: *const c_char
) -> i64 {
    let Some(mb) = 取邮箱(句柄) else {
        return 0;
    };
    let mut 队 = mb.队.lock().unwrap_or_else(|e| e.into_inner());
    if 队.len() >= 邮箱上限 {
        return 0;
    }
    队.push_back((读C串(名字), 读C串(载荷)));
    1
}

/// 取出最早的一条消息，序列化成 `{"m":名字,"p":载荷}`。
/// 邮箱空或已关闭返回空串 —— 空串不是合法消息，调用方据此判断「没了」。
#[no_mangle]
pub extern "C" fn qi_mailbox_take(句柄: i64) -> *mut c_char {
    let Some(mb) = 取邮箱(句柄) else {
        return crate::stdlib::qi_str::rc_cstr_from_str("");
    };
    let 取出 = {
        let mut 队 = mb.队.lock().unwrap_or_else(|e| e.into_inner());
        队.pop_front()
    };
    match 取出 {
        Some((名字, 载荷)) => {
            let 文本 = serde_json::json!({ "m": 名字, "p": 载荷 }).to_string();
            crate::stdlib::qi_str::rc_cstr_from_string(文本)
        }
        None => crate::stdlib::qi_str::rc_cstr_from_str(""),
    }
}

/// 当前积压条数（已关闭的邮箱返回 0）
#[no_mangle]
pub extern "C" fn qi_mailbox_count(句柄: i64) -> i64 {
    match 取邮箱(句柄) {
        Some(mb) => mb.队.lock().unwrap_or_else(|e| e.into_inner()).len() as i64,
        None => 0,
    }
}

/// 关闭邮箱：从池里摘掉，未取的消息一并丢弃。
///
/// 之后所有投递（含已经排好队的定时投递）都会返回 0 —— 句柄单调递增不复用，
/// 所以迟到的投递绝不会误伤后来新建的邮箱。
#[no_mangle]
pub extern "C" fn qi_mailbox_close(句柄: i64) -> i64 {
    match 取池()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&句柄)
    {
        Some(_) => 1,
        None => 0,
    }
}

// ── 定时投递 ────────────────────────────────────────────────────────────
//
// 全进程**一根**定时线程 + 一个最小堆。不给每个定时器起线程/协程：
// 一个每秒 tick 的时钟页面开 100 个连接就是 100 根线程在睡觉，
// 而这里始终是 1 根，睡到最近一个到期时刻醒一次。

struct 定时项 {
    到期: Instant,
    序号: u64, // 同一时刻到期时保持投递顺序（Instant 相同时按先来后到）
    邮箱: i64,
    名字: String,
    载荷: String,
}

impl PartialEq for 定时项 {
    fn eq(&self, 另: &Self) -> bool {
        self.到期 == 另.到期 && self.序号 == 另.序号
    }
}
impl Eq for 定时项 {}
impl Ord for 定时项 {
    fn cmp(&self, 另: &Self) -> std::cmp::Ordering {
        // BinaryHeap 是大顶堆，这里反过来 → 堆顶是最早到期的
        另.到期
            .cmp(&self.到期)
            .then_with(|| 另.序号.cmp(&self.序号))
    }
}
impl PartialOrd for 定时项 {
    fn partial_cmp(&self, 另: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(另))
    }
}

struct 定时器 {
    堆: Mutex<BinaryHeap<定时项>>,
    有事: Condvar,
}

static 定时器实例: OnceLock<Arc<定时器>> = OnceLock::new();
static 定时序号: AtomicI64 = AtomicI64::new(0);

fn 取定时器() -> &'static Arc<定时器> {
    定时器实例.get_or_init(|| {
        let 它 = Arc::new(定时器 {
            堆: Mutex::new(BinaryHeap::new()),
            有事: Condvar::new(),
        });
        let 线程用 = Arc::clone(&它);
        std::thread::Builder::new()
            .name("qi-邮箱定时".to_string())
            .spawn(move || 定时循环(线程用))
            .ok();
        它
    })
}

fn 定时循环(它: Arc<定时器>) {
    let mut 堆 = 它.堆.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        let 现在 = Instant::now();
        if 堆.peek().is_some_and(|顶| 顶.到期 <= 现在) {
            // 到期了：出堆，**在锁外投递**（投递要拿邮箱池锁，别嵌套持锁）
            let 项 = 堆.pop().expect("刚 peek 到，必有");
            drop(堆);
            let 名字 = std::ffi::CString::new(项.名字).unwrap_or_default();
            let 载荷 = std::ffi::CString::new(项.载荷).unwrap_or_default();
            qi_mailbox_post(项.邮箱, 名字.as_ptr(), 载荷.as_ptr());
            堆 = 它.堆.lock().unwrap_or_else(|e| e.into_inner());
            continue;
        }
        // 堆空 → 一直睡到有人加定时器；否则睡到最近一个到期时刻
        let 等多久 = 堆.peek().map(|顶| 顶.到期 - 现在);
        堆 = match 等多久 {
            Some(时长) => {
                let (守卫, _) = 它
                    .有事
                    .wait_timeout(堆, 时长)
                    .unwrap_or_else(|e| e.into_inner());
                守卫
            }
            None => 它.有事.wait(堆).unwrap_or_else(|e| e.into_inner()),
        };
    }
}

/// 延迟 delay_ms 毫秒后往邮箱投一条消息（对标 `Process.send_after`）。
///
/// 返回 1 表示已排期。**排期成功不等于一定送到**：到期时邮箱可能已经关了
/// （连接断了），那时这条消息直接丢弃 —— 这正是我们要的语义。
#[no_mangle]
pub extern "C" fn qi_mailbox_post_after(
    句柄: i64,
    延迟毫秒: i64,
    名字: *const c_char,
    载荷: *const c_char,
) -> i64 {
    let 它 = 取定时器();
    let 项 = 定时项 {
        到期: Instant::now() + Duration::from_millis(延迟毫秒.max(0) as u64),
        序号: 定时序号.fetch_add(1, Ordering::SeqCst) as u64,
        邮箱: 句柄,
        名字: 读C串(名字),
        载荷: 读C串(载荷),
    };
    {
        let mut 堆 = 它.堆.lock().unwrap_or_else(|e| e.into_inner());
        堆.push(项);
    }
    // 新排的可能比当前等待时限更早，叫醒定时线程重算
    它.有事.notify_one();
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn 投(句柄: i64, 名: &str, 载: &str) -> i64 {
        let n = CString::new(名).unwrap();
        let p = CString::new(载).unwrap();
        qi_mailbox_post(句柄, n.as_ptr(), p.as_ptr())
    }

    fn 取(句柄: i64) -> String {
        let p = qi_mailbox_take(句柄);
        unsafe { CStr::from_ptr(p).to_string_lossy().to_string() }
    }

    #[test]
    fn 先进先出() {
        let mb = qi_mailbox_create();
        assert_eq!(投(mb, "一", "{}"), 1);
        assert_eq!(投(mb, "二", "{}"), 1);
        assert_eq!(qi_mailbox_count(mb), 2);
        assert!(取(mb).contains("\"一\""));
        assert!(取(mb).contains("\"二\""));
        assert_eq!(取(mb), "");
        qi_mailbox_close(mb);
    }

    #[test]
    fn 载荷当字符串放_引号不会破坏帧() {
        let mb = qi_mailbox_create();
        投(mb, "回答", r#"{"文本":"他说\"好\""}"#);
        let 文本 = 取(mb);
        let 值: serde_json::Value = serde_json::from_str(&文本).expect("必须是合法 JSON");
        assert_eq!(值["m"], "回答");
        assert_eq!(值["p"], r#"{"文本":"他说\"好\""}"#);
        qi_mailbox_close(mb);
    }

    #[test]
    fn 关闭后投递失败而不是崩() {
        let mb = qi_mailbox_create();
        assert_eq!(qi_mailbox_close(mb), 1);
        assert_eq!(投(mb, "迟到的定时器", "{}"), 0);
        assert_eq!(取(mb), "");
        assert_eq!(qi_mailbox_count(mb), 0);
    }

    #[test]
    fn 句柄不复用_关掉再建拿到的是新号() {
        let 甲 = qi_mailbox_create();
        qi_mailbox_close(甲);
        let 乙 = qi_mailbox_create();
        assert_ne!(甲, 乙, "复用句柄会让迟到的投递打进别人的邮箱");
    }

    #[test]
    fn 满了拒收而不是丢老的() {
        let mb = qi_mailbox_create();
        for i in 0..邮箱上限 {
            assert_eq!(投(mb, &format!("第{i}"), "{}"), 1);
        }
        assert_eq!(投(mb, "溢出", "{}"), 0);
        assert!(取(mb).contains("第0"), "最老的那条要还在");
        qi_mailbox_close(mb);
    }

    #[test]
    fn 定时投递按到期先后() {
        let mb = qi_mailbox_create();
        let 名慢 = CString::new("慢").unwrap();
        let 名快 = CString::new("快").unwrap();
        let 空 = CString::new("{}").unwrap();
        qi_mailbox_post_after(mb, 120, 名慢.as_ptr(), 空.as_ptr());
        qi_mailbox_post_after(mb, 10, 名快.as_ptr(), 空.as_ptr());
        std::thread::sleep(Duration::from_millis(300));
        assert!(取(mb).contains("快"), "后排但更早到期的要先出来");
        assert!(取(mb).contains("慢"));
        qi_mailbox_close(mb);
    }

    #[test]
    fn 定时到期时邮箱已关_安静丢弃() {
        let mb = qi_mailbox_create();
        let 名 = CString::new("tick").unwrap();
        let 空 = CString::new("{}").unwrap();
        qi_mailbox_post_after(mb, 30, 名.as_ptr(), 空.as_ptr());
        qi_mailbox_close(mb);
        std::thread::sleep(Duration::from_millis(120)); // 没崩就算过
    }
}
