//! 流式 HTTP 读 —— 响应体边到边读，而不是等它整个收完。
//!
//! ## 为什么要有这个
//!
//! 在此之前 qi 侧的 HTTP 全是「一次要完整响应体」（qi_http_get 那一票，
//! 最后都落在 `响应.text()` 上）。这意味着 SSE、chunked 长响应、大文件下载
//! 在 qi 里根本表达不出来 —— 只能等服务端关连接，而 SSE 永远不关。
//!
//! 后果是所有流式的东西都必须在 Rust 里写完：llm_ffi.rs 那 3979 行里很大一块
//! 就是 SSE 分帧 + 增量拼接，而那本该是 qi 能写的纯文本处理。这个模块把
//! **传输**留在 Rust（TLS、chunked 解码、连接管理），把**分帧**交给 qi。
//!
//! ## 为什么是「后台线程 + 有界通道」
//!
//! reqwest 的 blocking Response 实现 std::io::Read，但 blocking 的
//! ClientBuilder **没有** read_timeout（只有 async 那边有，见 0.12.24 的
//! blocking/client.rs），`.timeout()` 是整个请求的总时长 —— 对 SSE 这种
//! 开着不动几分钟很正常的流没法用。
//!
//! 所以：一根后台线程死循环 `read()` 往有界通道里塞，FFI 侧 `recv_timeout`
//! 拿。这样「等 5 秒没数据就先返回、流还活着」才表达得出来。同仓的
//! llm_ffi 可靠流 也是这个结构，不是新发明。
//!
//! 通道**有界**（不是无界）：服务端推得比 qi 侧读得快时要有反压，
//! 否则一条慢消费的流能把内存吃光 —— 无界通道在这里等于没有上限的缓冲。
//!
//! ## UTF-8 边界
//!
//! 网络分块跟字符边界毫无关系：一个汉字三字节，可能第一块结尾拿到两个、
//! 第二块开头才是第三个。直接把块当字符串交给 qi 就是乱码（更糟的是
//! from_utf8_lossy 会把半个字符替换成 U+FFFD，**数据就此损坏**，而且
//! 后半截字节也跟着废掉，错误还不可见）。
//!
//! 所以文本读取会把**结尾不完整的那几个字节留在缓冲里**，等下一块补齐再交出去。
//! 需要原始字节（下载文件、二进制协议）就用 读取字节，那条不做任何解码。
//!
//! ## 取消的老实话
//!
//! 关闭时读线程可能正阻塞在 `read()` 上。blocking 客户端既没有 read_timeout，
//! 也没法从外面戳醒一个阻塞的 socket 读，所以那根线程会一直挂到「服务端发来
//! 点什么」或者「TCP 自己超时」为止，之后才发现接收端没了并退出。
//!
//! 句柄和缓冲在 关闭 时**立即**回收，qi 侧不会看到任何延迟；悬着的只是一根
//! 线程和它那个连接。要给这个兜底，开流时传 总时限毫秒 —— 它落到
//! `.timeout()` 上，到点整个请求被 reqwest 掐断，线程必然退出。

use std::collections::HashMap;
use std::io::Read;
use std::os::raw::c_char;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use crate::stdlib::qi_str::rc_cstr_from_string;

/// 上一次读取的结果。qi 侧靠 流状态() 取，因为「读到空串」本身有歧义 ——
/// 可能是超时、可能是流结束，也可能真的是个空块。
pub const 状态_有数据: i64 = 0;
pub const 状态_超时: i64 = 1;
pub const 状态_结束: i64 = 2;
pub const 状态_出错: i64 = 3;
/// 句柄根本不存在（已关闭 / 从来没有过）。跟「出错」分开：前者是调用方
/// 拿着个废句柄，后者是流真的坏了，两种要查的地方不一样。
pub const 状态_无此流: i64 = 4;

/// 通道容量。按 8KB 一块算，满载约 512KB 在途。
const 通道容量: usize = 64;
/// 单次 read 的缓冲大小。
const 读缓冲: usize = 8 * 1024;

/// 句柄从这里起，单调递增、**永不复用**。
///
/// 不复用是有代价换来的教训（见 邮箱 那边同样的做法）：句柄一复用，
/// 一个「关完了还留着旧句柄」的调用方就会静默读到**别人的流**，
/// 而不是拿到「无此流」。那种 bug 查起来极贵。
static 句柄计数器: AtomicI64 = AtomicI64::new(700_001);

enum 块 {
    数据(Vec<u8>),
    错误(String),
}

struct 流 {
    状态码: i64,
    响应头: String,
    接收器: crossbeam::channel::Receiver<块>,
    已取消: Arc<AtomicBool>,
    /// 上次读取的结果，供 流状态() 查。
    上次状态: Mutex<i64>,
    错误信息: Mutex<String>,
    /// 文本读取时结尾那几个凑不成完整字符的字节，留到下一块拼上。
    半个字符: Mutex<Vec<u8>>,
    /// 通道读空且发送端已断 → 流真结束。单独记是因为 recv_timeout 的
    /// Disconnected 只能看到一次，之后再问还得答「结束」而不是「超时」。
    已结束: Mutex<bool>,
}

static 流池: OnceLock<Mutex<HashMap<i64, Arc<流>>>> = OnceLock::new();

fn 池() -> &'static Mutex<HashMap<i64, Arc<流>>> {
    流池.get_or_init(|| Mutex::new(HashMap::new()))
}

fn 取流(句柄: i64) -> Option<Arc<流>> {
    池().lock().ok()?.get(&句柄).cloned()
}

fn 读C串(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_str()
        .ok()
        .map(|s| s.to_string())
}

fn 空串() -> *mut c_char {
    rc_cstr_from_string(String::new())
}

fn 记状态(流: &流, 状态: i64) {
    if let Ok(mut s) = 流.上次状态.lock() {
        *s = 状态;
    }
}

fn 记错误(流: &流, 消息: String) {
    if let Ok(mut e) = 流.错误信息.lock() {
        *e = 消息;
    }
    记状态(流, 状态_出错);
}

/// 打开一条流式 HTTP 请求。
///
/// **阻塞到响应头到达**（reqwest 的 send() 本来就是这个语义），所以返回之后
/// 状态码 / 响应头 立刻可读，响应体才由后台线程慢慢泵。这样 qi 侧能先看
/// 状态码决定要不要继续读，而不是稀里糊涂开始读一个 500 的错误页。
///
/// 参数：
/// - `method` 大小写不敏感，认 GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS
/// - `headers_json` JSON 对象 `{"名":"值"}`，可传空串
/// - `body` 请求体，空串表示没有
/// - `connect_timeout_ms` 连接超时；<=0 用默认 30 秒
/// - `total_timeout_ms` 整条请求的总时限；**<=0 表示不限**（SSE 常态）
///
/// 返回正数句柄；失败返回负数：
/// -1 参数无效 / -2 URL 或请求构建失败 / -3 连接或请求失败
#[no_mangle]
pub extern "C" fn qi_http_stream_open(
    method: *const c_char,
    url: *const c_char,
    headers_json: *const c_char,
    body: *const c_char,
    connect_timeout_ms: i64,
    total_timeout_ms: i64,
) -> i64 {
    let (Some(方法文本), Some(地址)) = (读C串(method), 读C串(url)) else {
        return -1;
    };
    if 地址.trim().is_empty() {
        return -1;
    }
    let 头文本 = 读C串(headers_json).unwrap_or_default();
    let 体 = 读C串(body).unwrap_or_default();

    let 方法 = match 方法文本.trim().to_ascii_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        "HEAD" => reqwest::Method::HEAD,
        "OPTIONS" => reqwest::Method::OPTIONS,
        _ => return -1,
    };

    let mut 构建 = reqwest::blocking::Client::builder().connect_timeout(Duration::from_millis(
        if connect_timeout_ms > 0 {
            connect_timeout_ms as u64
        } else {
            30_000
        },
    ));
    // 总时限只在显式要求时设。默认不设是**故意**的：`.timeout()` 管的是整条
    // 请求（含读体），给 SSE 设一个就等于给流规定了寿命，到点无差别掐断。
    if total_timeout_ms > 0 {
        构建 = 构建.timeout(Duration::from_millis(total_timeout_ms as u64));
    }
    let Ok(客户端) = 构建.build() else {
        return -2;
    };

    let mut 请求 = 客户端.request(方法, &地址);
    if !头文本.trim().is_empty() {
        match serde_json::from_str::<serde_json::Value>(&头文本) {
            Ok(serde_json::Value::Object(表)) => {
                for (名, 值) in 表 {
                    let 值文本 = match 值 {
                        serde_json::Value::String(s) => s,
                        其他 => 其他.to_string(),
                    };
                    请求 = 请求.header(名, 值文本);
                }
            }
            _ => return -1,
        }
    }
    if !体.is_empty() {
        请求 = 请求.body(体);
    }

    let 响应 = match 请求.send() {
        Ok(r) => r,
        Err(_) => return -3,
    };

    let 状态码 = 响应.status().as_u16() as i64;
    let 响应头 = {
        let mut 表 = serde_json::Map::new();
        for (名, 值) in 响应.headers().iter() {
            // 头名大小写在 HTTP 里不敏感，reqwest 给的是小写，直接用。
            //
            // 值按 HTTP 规范只能是可见 ASCII，`to_str()` 也只认这个。真遇到非 ASCII
            // （服务端不守规矩，比如把中文文件名塞进自定义头）**不能丢掉这一条** ——
            // 丢了之后 qi 侧看到的是「压根没有这个头」，会往「服务端没发」的方向查，
            // 而实际上发了、只是值不规范。lossy 解出来至少保住「它存在」这个事实。
            let 值文本 = match 值.to_str() {
                Ok(v) => v.to_string(),
                Err(_) => String::from_utf8_lossy(值.as_bytes()).into_owned(),
            };
            表.insert(名.as_str().to_string(), serde_json::Value::String(值文本));
        }
        serde_json::Value::Object(表).to_string()
    };

    let (发送器, 接收器) = crossbeam::channel::bounded::<块>(通道容量);
    let 已取消 = Arc::new(AtomicBool::new(false));
    let 线程取消 = 已取消.clone();

    std::thread::spawn(move || {
        let mut 响应 = 响应;
        let mut 缓冲 = vec![0u8; 读缓冲];
        loop {
            if 线程取消.load(Ordering::Relaxed) {
                return;
            }
            match 响应.read(&mut 缓冲) {
                Ok(0) => return, // EOF：发送器随本闭包一起 drop，接收端读到 Disconnected
                Ok(n) => {
                    // send 失败 = 接收端没了（流被关掉），退出即可，不是错误
                    if 发送器.send(块::数据(缓冲[..n].to_vec())).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    let _ = 发送器.send(块::错误(format!("读取响应体失败: {}", e)));
                    return;
                }
            }
        }
    });

    let 句柄 = 句柄计数器.fetch_add(1, Ordering::Relaxed);
    let 流对象 = Arc::new(流 {
        状态码,
        响应头,
        接收器,
        已取消,
        上次状态: Mutex::new(状态_有数据),
        错误信息: Mutex::new(String::new()),
        半个字符: Mutex::new(Vec::new()),
        已结束: Mutex::new(false),
    });
    match 池().lock() {
        Ok(mut p) => {
            p.insert(句柄, 流对象);
            句柄
        }
        Err(_) => -2,
    }
}

/// HTTP 状态码。句柄无效返回 -1。
#[no_mangle]
pub extern "C" fn qi_http_stream_status(handle: i64) -> i64 {
    match 取流(handle) {
        Some(s) => s.状态码,
        None => -1,
    }
}

/// 全部响应头，JSON 对象（头名小写）。句柄无效返回空串。
#[no_mangle]
pub extern "C" fn qi_http_stream_headers(handle: i64) -> *mut c_char {
    match 取流(handle) {
        Some(s) => rc_cstr_from_string(s.响应头.clone()),
        None => 空串(),
    }
}

/// 取一块原始字节。返回字节切片句柄；没有数据（超时/结束/出错）返回 0。
/// 读完必须问 流状态() 才知道是哪种情况。
#[no_mangle]
pub extern "C" fn qi_http_stream_read_bytes(handle: i64, timeout_ms: i64) -> i64 {
    let Some(流) = 取流(handle) else {
        return 0;
    };
    match 收一块(&流, timeout_ms) {
        Some(字节) => {
            记状态(&流, 状态_有数据);
            crate::stdlib::bytes_ffi::register_bytes(字节)
        }
        None => 0,
    }
}

/// 取一块文本。结尾不完整的多字节字符会留到下一次，不会交出半个字符。
/// 没有数据时返回空串 —— 用 流状态() 区分超时 / 结束 / 出错。
#[no_mangle]
pub extern "C" fn qi_http_stream_read(handle: i64, timeout_ms: i64) -> *mut c_char {
    let Some(流) = 取流(handle) else {
        return 空串();
    };
    let 新字节 = match 收一块(&流, timeout_ms) {
        Some(b) => b,
        None => {
            // 流正常结束时，缓冲里还剩着凑不齐的字节 = 响应体不是合法 UTF-8
            // （截断的多字节序列）。这必须报出来：无声吞掉的话调用方会拿到一段
            // 少了尾巴的文本，而且完全看不出少了。
            if 流.上次状态.lock().map(|s| *s).unwrap_or(状态_结束) == 状态_结束 {
                let 剩 = 流
                    .半个字符
                    .lock()
                    .map(|mut b| std::mem::take(&mut *b))
                    .unwrap_or_default();
                if !剩.is_empty() {
                    记错误(
                        &流,
                        format!(
                            "响应体结尾有 {} 个字节凑不成完整字符（不是合法 UTF-8）",
                            剩.len()
                        ),
                    );
                }
            }
            return 空串();
        }
    };

    let mut 待解码 = match 流.半个字符.lock() {
        Ok(mut 半) => {
            let mut v = std::mem::take(&mut *半);
            v.extend_from_slice(&新字节);
            v
        }
        Err(_) => 新字节,
    };

    // 切到最后一个完整字符处；余下的字节留给下一块。
    let 完整长度 = match std::str::from_utf8(&待解码) {
        Ok(_) => 待解码.len(),
        Err(e) => {
            if e.error_len().is_some() {
                // 真的非法（不是「还没读全」），这条流的字节流本身坏了。
                记错误(
                    &流,
                    format!("响应体含非法 UTF-8 字节（偏移 {}）", e.valid_up_to()),
                );
                return 空串();
            }
            e.valid_up_to()
        }
    };
    let 尾巴 = 待解码.split_off(完整长度);
    if let Ok(mut 半) = 流.半个字符.lock() {
        *半 = 尾巴;
    }

    记状态(&流, 状态_有数据);
    match String::from_utf8(待解码) {
        Ok(s) => rc_cstr_from_string(s),
        // 上面已经切到合法边界，走不到这里；真走到也不能 panic。
        Err(e) => rc_cstr_from_string(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

/// 从通道收一块。顺带把 上次状态 记好（超时 / 结束 / 出错）。
fn 收一块(流: &流, timeout_ms: i64) -> Option<Vec<u8>> {
    // 已经判定结束的流不要再去 recv —— 通道断开后 recv_timeout 立刻返回
    // Disconnected，会被当成又一次「结束」没问题，但白等一轮没意义，
    // 更重要的是保证反复问的答案稳定。
    if 流.已结束.lock().map(|v| *v).unwrap_or(false) {
        记状态(流, 状态_结束);
        return None;
    }
    let 等待 = Duration::from_millis(timeout_ms.max(0) as u64);
    match 流.接收器.recv_timeout(等待) {
        Ok(块::数据(b)) => Some(b),
        Ok(块::错误(e)) => {
            记错误(流, e);
            None
        }
        Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
            记状态(流, 状态_超时);
            None
        }
        Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
            if let Ok(mut 完) = 流.已结束.lock() {
                *完 = true;
            }
            记状态(流, 状态_结束);
            None
        }
    }
}

/// 上一次读取的结果：0 有数据 / 1 超时 / 2 结束 / 3 出错 / 4 无此流。
#[no_mangle]
pub extern "C" fn qi_http_stream_state(handle: i64) -> i64 {
    match 取流(handle) {
        Some(s) => s.上次状态.lock().map(|v| *v).unwrap_or(状态_出错),
        None => 状态_无此流,
    }
}

/// 出错时的说明；没出错返回空串。
#[no_mangle]
pub extern "C" fn qi_http_stream_error(handle: i64) -> *mut c_char {
    match 取流(handle) {
        Some(s) => rc_cstr_from_string(s.错误信息.lock().map(|v| v.clone()).unwrap_or_default()),
        None => 空串(),
    }
}

/// 关闭流，回收句柄。重复关闭无副作用。
///
/// 立即返回 —— 读线程可能还阻塞在 read() 上（见模块头「取消的老实话」），
/// 但 qi 侧不等它。
#[no_mangle]
pub extern "C" fn qi_http_stream_close(handle: i64) -> i64 {
    let 取出 = 池().lock().ok().and_then(|mut p| p.remove(&handle));
    match 取出 {
        Some(流) => {
            流.已取消.store(true, Ordering::Relaxed);
            1
        }
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::io::Write;
    use std::net::TcpListener;

    fn c(s: &str) -> CString {
        CString::new(s).unwrap()
    }

    fn 取串(p: *mut c_char) -> String {
        if p.is_null() {
            return String::new();
        }
        unsafe { std::ffi::CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned()
    }

    /// 起一个只服务一次的 HTTP 服务，按 分片 逐块写出、每块之间停 停顿。
    /// 返回端点地址。
    fn 起服务(分片: Vec<Vec<u8>>, 头: &'static str, 停顿: Duration) -> String {
        let 监听 = TcpListener::bind("127.0.0.1:0").unwrap();
        let 端口 = 监听.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut 连, _)) = 监听.accept() {
                let mut 丢 = [0u8; 4096];
                let _ = 连.read(&mut 丢);
                let _ = 连.write_all(头.as_bytes());
                let _ = 连.flush();
                for 片 in 分片 {
                    if !停顿.is_zero() {
                        std::thread::sleep(停顿);
                    }
                    if 连.write_all(&片).is_err() {
                        return;
                    }
                    let _ = 连.flush();
                }
            }
        });
        format!("http://127.0.0.1:{}/", 端口)
    }

    const 定长头: &str = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n";

    fn 开(端点: &str) -> i64 {
        qi_http_stream_open(
            c("GET").as_ptr(),
            c(端点).as_ptr(),
            c("").as_ptr(),
            c("").as_ptr(),
            5_000,
            0,
        )
    }

    /// 读到流结束，返回拼起来的全文。
    fn 读到底(流: i64) -> String {
        let mut 全 = String::new();
        for _ in 0..500 {
            全.push_str(&取串(qi_http_stream_read(流, 2_000)));
            let 态 = qi_http_stream_state(流);
            if 态 == 状态_结束 || 态 == 状态_出错 {
                break;
            }
        }
        全
    }

    #[test]
    fn 分块到达能逐块读出() {
        let 端点 = 起服务(
            vec![
                b"hello ".to_vec(),
                b"streaming ".to_vec(),
                b"world".to_vec(),
            ],
            定长头,
            Duration::from_millis(30),
        );
        let 流 = 开(&端点);
        assert!(流 > 0, "开流失败: {}", 流);
        assert_eq!(qi_http_stream_status(流), 200);
        assert_eq!(读到底(流), "hello streaming world");
        assert_eq!(qi_http_stream_state(流), 状态_结束);
        qi_http_stream_close(流);
    }

    /// 这条是整个模块存在的理由：一个汉字被拆在两个网络块里，
    /// 不能变成乱码，也不能变成 U+FFFD。
    #[test]
    fn 汉字跨块不被截断() {
        let 文 = "流式读取中文测试".as_bytes().to_vec();
        // 在第 4 个字节处切开 —— 正好落在第二个汉字中间
        let (甲, 乙) = 文.split_at(4);
        let 端点 = 起服务(
            vec![甲.to_vec(), 乙.to_vec()],
            定长头,
            Duration::from_millis(30),
        );
        let 流 = 开(&端点);
        assert!(流 > 0);
        let 全 = 读到底(流);
        assert_eq!(全, "流式读取中文测试");
        assert!(
            !全.contains('\u{fffd}'),
            "出现替换字符，说明半个字符被交出去了"
        );
        qi_http_stream_close(流);
    }

    /// 每个字节单独一块 —— 最坏情况，每个汉字都被拆成三块。
    #[test]
    fn 逐字节到达也不乱码() {
        let 文 = "汉字逐字节abc混排".as_bytes().to_vec();
        let 分片: Vec<Vec<u8>> = 文.iter().map(|b| vec![*b]).collect();
        let 端点 = 起服务(分片, 定长头, Duration::from_millis(1));
        let 流 = 开(&端点);
        assert!(流 > 0);
        assert_eq!(读到底(流), "汉字逐字节abc混排");
        qi_http_stream_close(流);
    }

    /// 服务端半天不说话时，读要能按时返回「超时」而不是挂住，且流还活着。
    #[test]
    fn 超时后流仍可继续读() {
        let 端点 = 起服务(vec![b"late".to_vec()], 定长头, Duration::from_millis(600));
        let 流 = 开(&端点);
        assert!(流 > 0);

        let 起 = std::time::Instant::now();
        let 首次 = 取串(qi_http_stream_read(流, 150));
        assert_eq!(首次, "");
        assert_eq!(qi_http_stream_state(流), 状态_超时);
        assert!(起.elapsed() < Duration::from_millis(500), "没有按时返回");

        // 流没被超时弄死，继续等就能拿到
        let mut 全 = String::new();
        for _ in 0..20 {
            全.push_str(&取串(qi_http_stream_read(流, 300)));
            if 全.contains("late") {
                break;
            }
        }
        assert_eq!(全, "late");
        qi_http_stream_close(流);
    }

    #[test]
    fn 读取字节不做任何解码() {
        // 故意不是合法 UTF-8
        let 原始 = vec![0xff, 0xfe, 0x00, 0x41, 0x42];
        let 端点 = 起服务(vec![原始.clone()], 定长头, Duration::ZERO);
        let 流 = 开(&端点);
        assert!(流 > 0);
        let mut 收 = Vec::new();
        for _ in 0..50 {
            let h = qi_http_stream_read_bytes(流, 1_000);
            if h != 0 {
                收.extend(crate::stdlib::bytes_ffi::clone_bytes(h).unwrap_or_default());
            }
            let 态 = qi_http_stream_state(流);
            if 态 == 状态_结束 || 态 == 状态_出错 {
                break;
            }
        }
        assert_eq!(收, 原始);
        qi_http_stream_close(流);
    }

    /// 非法 UTF-8 走文本读取时必须报错，不能悄悄替换成 U+FFFD。
    #[test]
    fn 文本读遇非法utf8要报错() {
        let 端点 = 起服务(vec![vec![b'a', 0xff, 0xfe, b'b']], 定长头, Duration::ZERO);
        let 流 = 开(&端点);
        assert!(流 > 0);
        let mut 出错了 = false;
        for _ in 0..50 {
            let _ = 取串(qi_http_stream_read(流, 1_000));
            let 态 = qi_http_stream_state(流);
            if 态 == 状态_出错 {
                出错了 = true;
                assert!(
                    取串(qi_http_stream_error(流)).contains("非法 UTF-8"),
                    "错误信息没说清楚"
                );
                break;
            }
            if 态 == 状态_结束 {
                break;
            }
        }
        assert!(出错了, "非法 UTF-8 被静默吞掉了");
        qi_http_stream_close(流);
    }

    #[test]
    fn 状态码与响应头可读() {
        let 端点 = 起服务(
            vec![b"nope".to_vec()],
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nX-Qi-Test: abc-123\r\nX-Qi-Bad: 值\r\nConnection: close\r\n\r\n",
            Duration::ZERO,
        );
        let 流 = 开(&端点);
        assert!(流 > 0);
        assert_eq!(qi_http_stream_status(流), 404);
        let 头: serde_json::Value =
            serde_json::from_str(&取串(qi_http_stream_headers(流))).unwrap();
        assert_eq!(头["content-type"], serde_json::json!("text/plain"));
        assert_eq!(头["x-qi-test"], serde_json::json!("abc-123"));
        // 非 ASCII 头值不合 HTTP 规范，但不能因此让这一条**消失** ——
        // 消失了 qi 侧会以为服务端没发这个头，查错方向。
        assert_eq!(
            头["x-qi-bad"],
            serde_json::json!("值"),
            "不规范的头值被丢掉了"
        );
        qi_http_stream_close(流);
    }

    #[test]
    fn 关闭后句柄立刻失效且重复关闭无害() {
        let 端点 = 起服务(vec![b"x".to_vec()], 定长头, Duration::ZERO);
        let 流 = 开(&端点);
        assert!(流 > 0);
        assert_eq!(qi_http_stream_close(流), 1);
        assert_eq!(qi_http_stream_close(流), 0, "重复关闭应无副作用");
        assert_eq!(qi_http_stream_state(流), 状态_无此流);
        assert_eq!(qi_http_stream_status(流), -1);
        assert_eq!(取串(qi_http_stream_read(流, 10)), "");
    }

    /// 句柄不复用：关掉一条再开一条，新句柄不能等于旧的。
    #[test]
    fn 句柄不复用() {
        let 端点 = 起服务(vec![b"a".to_vec()], 定长头, Duration::ZERO);
        let 甲 = 开(&端点);
        qi_http_stream_close(甲);
        let 端点2 = 起服务(vec![b"b".to_vec()], 定长头, Duration::ZERO);
        let 乙 = 开(&端点2);
        assert_ne!(甲, 乙, "句柄被复用了");
        qi_http_stream_close(乙);
    }

    #[test]
    fn 坏参数不崩() {
        assert_eq!(
            qi_http_stream_open(
                std::ptr::null(),
                c("http://x/").as_ptr(),
                c("").as_ptr(),
                c("").as_ptr(),
                1000,
                0
            ),
            -1
        );
        assert_eq!(
            qi_http_stream_open(
                c("GET").as_ptr(),
                c("").as_ptr(),
                c("").as_ptr(),
                c("").as_ptr(),
                1000,
                0
            ),
            -1
        );
        // 不认识的方法
        assert_eq!(
            qi_http_stream_open(
                c("TELEPORT").as_ptr(),
                c("http://127.0.0.1:1/").as_ptr(),
                c("").as_ptr(),
                c("").as_ptr(),
                1000,
                0
            ),
            -1
        );
        // 连不上
        assert_eq!(
            qi_http_stream_open(
                c("GET").as_ptr(),
                c("http://127.0.0.1:1/").as_ptr(),
                c("").as_ptr(),
                c("").as_ptr(),
                500,
                0
            ),
            -3
        );
    }
}
