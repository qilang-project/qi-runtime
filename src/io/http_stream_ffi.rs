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
//! from_utf8_lossy 会把partial_char替换成 U+FFFD，**数据就此损坏**，而且
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

/// 上一次读取的结果。qi 侧靠 流状态() 取，因为「读到empty_cstr」本身有歧义 ——
/// 可能是超时、可能是流结束，也可能真的是个空块。
pub const STATE_DATA: i64 = 0;
pub const STATE_TIMEOUT: i64 = 1;
pub const STATE_EOF: i64 = 2;
pub const STATE_ERROR: i64 = 3;
/// 句柄根本不存在（已关闭 / 从来没有过）。跟「出错」分开：前者是调用方
/// 拿着个废句柄，后者是流真的坏了，两种要查的地方不一样。
pub const STATE_NO_STREAM: i64 = 4;

/// CHANNEL_CAP。按 8KB 一块算，满载约 512KB 在途。
const CHANNEL_CAP: usize = 64;
/// 单次 read 的缓冲大小。
const READ_BUF: usize = 8 * 1024;

/// 句柄从这里起，单调递增、**永不复用**。
///
/// 不复用是有代价换来的教训（见 邮箱 那边同样的做法）：句柄一复用，
/// 一个「关完了还留着旧句柄」的调用方就会静默读到**别人的流**，
/// 而不是拿到「无此流」。那种 bug 查起来极贵。
static NEXT_HANDLE: AtomicI64 = AtomicI64::new(700_001);

enum Chunk {
    Data(Vec<u8>),
    Err_(String),
}

struct stream {
    status: i64,
    headers: String,
    receiver: crossbeam::channel::Receiver<Chunk>,
    cancelled: Arc<AtomicBool>,
    /// 上次读取的结果，供 流状态() 查。
    last_state: Mutex<i64>,
    error_msg: Mutex<String>,
    /// 文本读取时结尾那几个凑不成完整字符的字节，留到下一块拼上。
    partial_char: Mutex<Vec<u8>>,
    /// 通道读空且发送端已断 → 流真结束。单独记是因为 recv_timeout 的
    /// Disconnected 只能看到一次，之后再问还得答「结束」而不是「超时」。
    finished: Mutex<bool>,
}

static STREAMS: OnceLock<Mutex<HashMap<i64, Arc<stream>>>> = OnceLock::new();

fn pool() -> &'static Mutex<HashMap<i64, Arc<stream>>> {
    STREAMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn get_stream(handle: i64) -> Option<Arc<stream>> {
    pool().lock().ok()?.get(&handle).cloned()
}

fn read_cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    unsafe { std::ffi::CStr::from_ptr(p) }
        .to_str()
        .ok()
        .map(|s| s.to_string())
}

fn empty_cstr() -> *mut c_char {
    rc_cstr_from_string(String::new())
}

fn set_state(stream: &stream, 状态: i64) {
    if let Ok(mut s) = stream.last_state.lock() {
        *s = 状态;
    }
}

fn set_error(stream: &stream, 消息: String) {
    if let Ok(mut e) = stream.error_msg.lock() {
        *e = 消息;
    }
    set_state(stream, STATE_ERROR);
}

/// 打开一条流式 HTTP 请求。
///
/// **阻塞到headers到达**（reqwest 的 send() 本来就是这个语义），所以返回之后
/// status / headers 立刻可读，响应体才由后台线程慢慢泵。这样 qi 侧能先看
/// status决定要不要继续读，而不是稀里糊涂开始读一个 500 的错误页。
///
/// 参数：
/// - `method` 大小写不敏感，认 GET/POST/PUT/PATCH/DELETE/HEAD/OPTIONS
/// - `headers_json` JSON 对象 `{"名":"值"}`，可传empty_cstr
/// - `body` 请求体，empty_cstr表示没有
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
    let (Some(method_text), Some(url_text)) = (read_cstr(method), read_cstr(url)) else {
        return -1;
    };
    if url_text.trim().is_empty() {
        return -1;
    }
    let headers_text = read_cstr(headers_json).unwrap_or_default();
    let body_text = read_cstr(body).unwrap_or_default();

    let method = match method_text.trim().to_ascii_uppercase().as_str() {
        "GET" => reqwest::Method::GET,
        "POST" => reqwest::Method::POST,
        "PUT" => reqwest::Method::PUT,
        "PATCH" => reqwest::Method::PATCH,
        "DELETE" => reqwest::Method::DELETE,
        "HEAD" => reqwest::Method::HEAD,
        "OPTIONS" => reqwest::Method::OPTIONS,
        _ => return -1,
    };

    let mut builder = reqwest::blocking::Client::builder().connect_timeout(Duration::from_millis(
        if connect_timeout_ms > 0 {
            connect_timeout_ms as u64
        } else {
            30_000
        },
    ));
    // 总时限只在显式要求时设。默认不设是**故意**的：`.timeout()` 管的是整条
    // 请求（含读体），给 SSE 设一个就等于给流规定了寿命，到点无差别掐断。
    if total_timeout_ms > 0 {
        builder = builder.timeout(Duration::from_millis(total_timeout_ms as u64));
    }
    let Ok(client) = builder.build() else {
        return -2;
    };

    let mut req = client.request(method, &url_text);
    if !headers_text.trim().is_empty() {
        match serde_json::from_str::<serde_json::Value>(&headers_text) {
            Ok(serde_json::Value::Object(map)) => {
                for (k, v) in map {
                    let value_text = match v {
                        serde_json::Value::String(s) => s,
                        其他 => 其他.to_string(),
                    };
                    req = req.header(k, value_text);
                }
            }
            _ => return -1,
        }
    }
    if !body_text.is_empty() {
        req = req.body(body_text);
    }

    let response = match req.send() {
        Ok(r) => r,
        Err(_) => return -3,
    };

    register_response(response)
}

/// 把一个**已经拿到**的 blocking Response 接进STREAMS，返回句柄。
///
/// 给需要自己发请求的调用方用（LLM 流式要按 provider 加鉴权头、走会话endpoint，
/// 那套逻辑在 llm_ffi 里，不该在这儿重复一遍）。接进来之后读取/超时/关闭
/// 全走同一套，UTF-8 边界处理也一样。
pub(crate) fn register_response(response: reqwest::blocking::Response) -> i64 {
    let status = response.status().as_u16() as i64;
    let headers = {
        let mut map = serde_json::Map::new();
        for (k, v) in response.headers().iter() {
            // 头名大小写在 HTTP 里不敏感，reqwest 给的是小写，直接用。
            //
            // 值按 HTTP 规范只能是可见 ASCII，`to_str()` 也只认这个。真遇到非 ASCII
            // （服务端不守规矩，比如把中文文件名塞进自定义头）**不能丢掉这一条** ——
            // 丢了之后 qi 侧看到的是「压根没有这个头」，会往「服务端没发」的方向查，
            // 而实际上发了、只是值不规范。lossy 解出来至少保住「它存在」这个事实。
            let value_text = match v.to_str() {
                Ok(v) => v.to_string(),
                Err(_) => String::from_utf8_lossy(v.as_bytes()).into_owned(),
            };
            map.insert(
                k.as_str().to_string(),
                serde_json::Value::String(value_text),
            );
        }
        serde_json::Value::Object(map).to_string()
    };

    let (sender, receiver) = crossbeam::channel::bounded::<Chunk>(CHANNEL_CAP);
    let cancelled = Arc::new(AtomicBool::new(false));
    let thread_cancelled = cancelled.clone();

    std::thread::spawn(move || {
        let mut response = response;
        let mut buf = vec![0u8; READ_BUF];
        loop {
            if thread_cancelled.load(Ordering::Relaxed) {
                return;
            }
            match response.read(&mut buf) {
                Ok(0) => return, // EOF：sender随本闭包一起 drop，接收端读到 Disconnected
                Ok(n) => {
                    // send 失败 = 接收端没了（流被关掉），退出即可，不是错误
                    if sender.send(Chunk::Data(buf[..n].to_vec())).is_err() {
                        return;
                    }
                }
                Err(e) => {
                    let _ = sender.send(Chunk::Err_(format!("读取响应体失败: {}", e)));
                    return;
                }
            }
        }
    });

    let handle = NEXT_HANDLE.fetch_add(1, Ordering::Relaxed);
    let stream = Arc::new(stream {
        status,
        headers,
        receiver,
        cancelled,
        last_state: Mutex::new(STATE_DATA),
        error_msg: Mutex::new(String::new()),
        partial_char: Mutex::new(Vec::new()),
        finished: Mutex::new(false),
    });
    match pool().lock() {
        Ok(mut p) => {
            p.insert(handle, stream);
            handle
        }
        Err(_) => -2,
    }
}

/// HTTP status。句柄无效返回 -1。
#[no_mangle]
pub extern "C" fn qi_http_stream_status(handle: i64) -> i64 {
    match get_stream(handle) {
        Some(s) => s.status,
        None => -1,
    }
}

/// 全部headers，JSON 对象（头名小写）。句柄无效返回empty_cstr。
#[no_mangle]
pub extern "C" fn qi_http_stream_headers(handle: i64) -> *mut c_char {
    match get_stream(handle) {
        Some(s) => rc_cstr_from_string(s.headers.clone()),
        None => empty_cstr(),
    }
}

/// 取一块原始字节。返回字节切片句柄；没有数据（超时/结束/出错）返回 0。
/// 读完必须问 流状态() 才知道是哪种情况。
#[no_mangle]
pub extern "C" fn qi_http_stream_read_bytes(handle: i64, timeout_ms: i64) -> i64 {
    let Some(stream) = get_stream(handle) else {
        return 0;
    };
    match recv_chunk(&stream, timeout_ms) {
        Some(bytes) => {
            set_state(&stream, STATE_DATA);
            crate::stdlib::bytes_ffi::register_bytes(bytes)
        }
        None => 0,
    }
}

/// 取一块文本。结尾不完整的多字节字符会留到下一次，不会交出partial_char。
/// 没有数据时返回empty_cstr —— 用 流状态() 区分超时 / 结束 / 出错。
#[no_mangle]
pub extern "C" fn qi_http_stream_read(handle: i64, timeout_ms: i64) -> *mut c_char {
    let Some(stream) = get_stream(handle) else {
        return empty_cstr();
    };
    let new_bytes = match recv_chunk(&stream, timeout_ms) {
        Some(b) => b,
        None => {
            // 流正常结束时，缓冲里还剩着凑不齐的字节 = 响应体不是合法 UTF-8
            // （截断的多字节序列）。这必须报出来：无声吞掉的话调用方会拿到一段
            // 少了tail的文本，而且完全看不出少了。
            if stream.last_state.lock().map(|s| *s).unwrap_or(STATE_EOF) == STATE_EOF {
                let leftover = stream
                    .partial_char
                    .lock()
                    .map(|mut b| std::mem::take(&mut *b))
                    .unwrap_or_default();
                if !leftover.is_empty() {
                    set_error(
                        &stream,
                        format!(
                            "响应体结尾有 {} 个字节凑不成完整字符（不是合法 UTF-8）",
                            leftover.len()
                        ),
                    );
                }
            }
            return empty_cstr();
        }
    };

    let mut pending = match stream.partial_char.lock() {
        Ok(mut held) => {
            let mut v = std::mem::take(&mut *held);
            v.extend_from_slice(&new_bytes);
            v
        }
        Err(_) => new_bytes,
    };

    // 切到最后一个完整字符处；余下的字节留给下一块。
    let valid_len = match std::str::from_utf8(&pending) {
        Ok(_) => pending.len(),
        Err(e) => {
            if e.error_len().is_some() {
                // 真的非法（不是「还没读全」），这条流的字节流本身坏了。
                set_error(
                    &stream,
                    format!("响应体含非法 UTF-8 字节（偏移 {}）", e.valid_up_to()),
                );
                return empty_cstr();
            }
            e.valid_up_to()
        }
    };
    let tail = pending.split_off(valid_len);
    if let Ok(mut held) = stream.partial_char.lock() {
        *held = tail;
    }

    set_state(&stream, STATE_DATA);
    match String::from_utf8(pending) {
        Ok(s) => rc_cstr_from_string(s),
        // 上面已经切到合法边界，走不到这里；真走到也不能 panic。
        Err(e) => rc_cstr_from_string(String::from_utf8_lossy(e.as_bytes()).into_owned()),
    }
}

/// 从通道recv_chunk。顺带把 last_state 记好（超时 / 结束 / 出错）。
fn recv_chunk(stream: &stream, timeout_ms: i64) -> Option<Vec<u8>> {
    // 已经判定结束的流不要再去 recv —— 通道断开后 recv_timeout 立刻返回
    // Disconnected，会被当成又一次「结束」没问题，但白等一轮没意义，
    // 更重要的是保证反复问的答案稳定。
    if stream.finished.lock().map(|v| *v).unwrap_or(false) {
        set_state(stream, STATE_EOF);
        return None;
    }
    let wait = Duration::from_millis(timeout_ms.max(0) as u64);
    match stream.receiver.recv_timeout(wait) {
        Ok(Chunk::Data(b)) => Some(b),
        Ok(Chunk::Err_(e)) => {
            set_error(stream, e);
            None
        }
        Err(crossbeam::channel::RecvTimeoutError::Timeout) => {
            set_state(stream, STATE_TIMEOUT);
            None
        }
        Err(crossbeam::channel::RecvTimeoutError::Disconnected) => {
            if let Ok(mut done) = stream.finished.lock() {
                *done = true;
            }
            set_state(stream, STATE_EOF);
            None
        }
    }
}

/// 上一次读取的结果：0 有数据 / 1 超时 / 2 结束 / 3 出错 / 4 无此流。
#[no_mangle]
pub extern "C" fn qi_http_stream_state(handle: i64) -> i64 {
    match get_stream(handle) {
        Some(s) => s.last_state.lock().map(|v| *v).unwrap_or(STATE_ERROR),
        None => STATE_NO_STREAM,
    }
}

/// 出错时的说明；没出错返回empty_cstr。
#[no_mangle]
pub extern "C" fn qi_http_stream_error(handle: i64) -> *mut c_char {
    match get_stream(handle) {
        Some(s) => rc_cstr_from_string(s.error_msg.lock().map(|v| v.clone()).unwrap_or_default()),
        None => empty_cstr(),
    }
}

/// 关闭流，回收句柄。重复关闭无副作用。
///
/// 立即返回 —— 读线程可能还阻塞在 read() 上（见模块头「取消的老实话」），
/// 但 qi 侧不等它。
#[no_mangle]
pub extern "C" fn qi_http_stream_close(handle: i64) -> i64 {
    let removed = pool().lock().ok().and_then(|mut p| p.remove(&handle));
    match removed {
        Some(stream) => {
            stream.cancelled.store(true, Ordering::Relaxed);
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

    fn to_string_owned(p: *mut c_char) -> String {
        if p.is_null() {
            return String::new();
        }
        unsafe { std::ffi::CStr::from_ptr(p) }
            .to_string_lossy()
            .into_owned()
    }

    /// 起一个只服务一次的 HTTP 服务，按 chunks 逐块写出、每块之间停 gap。
    /// 返回endpoint地址。
    fn serve_once(chunks: Vec<Vec<u8>>, header: &'static str, gap: Duration) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Ok((mut conn, _)) = listener.accept() {
                let mut scratch = [0u8; 4096];
                let _ = conn.read(&mut scratch);
                let _ = conn.write_all(header.as_bytes());
                let _ = conn.flush();
                for chunk in chunks {
                    if !gap.is_zero() {
                        std::thread::sleep(gap);
                    }
                    if conn.write_all(&chunk).is_err() {
                        return;
                    }
                    let _ = conn.flush();
                }
            }
        });
        format!("http://127.0.0.1:{}/", port)
    }

    const FIXED_HEADER: &str =
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n";

    fn open_stream(endpoint: &str) -> i64 {
        qi_http_stream_open(
            c("GET").as_ptr(),
            c(endpoint).as_ptr(),
            c("").as_ptr(),
            c("").as_ptr(),
            5_000,
            0,
        )
    }

    /// 读到流结束，返回拼起来的全文。
    fn read_all(stream: i64) -> String {
        let mut all = String::new();
        for _ in 0..500 {
            all.push_str(&to_string_owned(qi_http_stream_read(stream, 2_000)));
            let st = qi_http_stream_state(stream);
            if st == STATE_EOF || st == STATE_ERROR {
                break;
            }
        }
        all
    }

    #[test]
    fn chunks_arrive_incrementally() {
        let endpoint = serve_once(
            vec![
                b"hello ".to_vec(),
                b"streaming ".to_vec(),
                b"world".to_vec(),
            ],
            FIXED_HEADER,
            Duration::from_millis(30),
        );
        let stream = open_stream(&endpoint);
        assert!(stream > 0, "开流失败: {}", stream);
        assert_eq!(qi_http_stream_status(stream), 200);
        assert_eq!(read_all(stream), "hello streaming world");
        assert_eq!(qi_http_stream_state(stream), STATE_EOF);
        qi_http_stream_close(stream);
    }

    /// 这条是整个模块存在的理由：一个汉字被拆在两个网络块里，
    /// 不能变成乱码，也不能变成 U+FFFD。
    #[test]
    fn cjk_split_across_chunks_is_intact() {
        let text = "流式读取中文测试".as_bytes().to_vec();
        // 在第 4 个字节处切开 —— 正好落在第二个汉字中间
        let (head, rest) = text.split_at(4);
        let endpoint = serve_once(
            vec![head.to_vec(), rest.to_vec()],
            FIXED_HEADER,
            Duration::from_millis(30),
        );
        let stream = open_stream(&endpoint);
        assert!(stream > 0);
        let all = read_all(stream);
        assert_eq!(all, "流式读取中文测试");
        assert!(
            !all.contains('\u{fffd}'),
            "出现替换字符，说明半个字符被交出去了"
        );
        qi_http_stream_close(stream);
    }

    /// 每个字节单独一块 —— 最坏情况，每个汉字都被拆成三块。
    #[test]
    fn byte_at_a_time_is_intact() {
        let text = "汉字逐字节abc混排".as_bytes().to_vec();
        let chunks: Vec<Vec<u8>> = text.iter().map(|b| vec![*b]).collect();
        let endpoint = serve_once(chunks, FIXED_HEADER, Duration::from_millis(1));
        let stream = open_stream(&endpoint);
        assert!(stream > 0);
        assert_eq!(read_all(stream), "汉字逐字节abc混排");
        qi_http_stream_close(stream);
    }

    /// 服务端半天不说话时，读要能按时返回「超时」而不是挂住，且流还活着。
    #[test]
    fn stream_survives_read_timeout() {
        let endpoint = serve_once(
            vec![b"late".to_vec()],
            FIXED_HEADER,
            Duration::from_millis(600),
        );
        let stream = open_stream(&endpoint);
        assert!(stream > 0);

        let started = std::time::Instant::now();
        let first = to_string_owned(qi_http_stream_read(stream, 150));
        assert_eq!(first, "");
        assert_eq!(qi_http_stream_state(stream), STATE_TIMEOUT);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "没有按时返回"
        );

        // 流没被超时弄死，继续等就能拿到
        let mut all = String::new();
        for _ in 0..20 {
            all.push_str(&to_string_owned(qi_http_stream_read(stream, 300)));
            if all.contains("late") {
                break;
            }
        }
        assert_eq!(all, "late");
        qi_http_stream_close(stream);
    }

    #[test]
    fn read_bytes_does_no_decoding() {
        // 故意不是合法 UTF-8
        let raw = vec![0xff, 0xfe, 0x00, 0x41, 0x42];
        let endpoint = serve_once(vec![raw.clone()], FIXED_HEADER, Duration::ZERO);
        let stream = open_stream(&endpoint);
        assert!(stream > 0);
        let mut got = Vec::new();
        for _ in 0..50 {
            let h = qi_http_stream_read_bytes(stream, 1_000);
            if h != 0 {
                got.extend(crate::stdlib::bytes_ffi::clone_bytes(h).unwrap_or_default());
            }
            let st = qi_http_stream_state(stream);
            if st == STATE_EOF || st == STATE_ERROR {
                break;
            }
        }
        assert_eq!(got, raw);
        qi_http_stream_close(stream);
    }

    /// 非法 UTF-8 走文本读取时必须报错，不能悄悄替换成 U+FFFD。
    #[test]
    fn invalid_utf8_reports_error() {
        let endpoint = serve_once(
            vec![vec![b'a', 0xff, 0xfe, b'b']],
            FIXED_HEADER,
            Duration::ZERO,
        );
        let stream = open_stream(&endpoint);
        assert!(stream > 0);
        let mut saw_error = false;
        for _ in 0..50 {
            let _ = to_string_owned(qi_http_stream_read(stream, 1_000));
            let st = qi_http_stream_state(stream);
            if st == STATE_ERROR {
                saw_error = true;
                assert!(
                    to_string_owned(qi_http_stream_error(stream)).contains("非法 UTF-8"),
                    "错误信息没说清楚"
                );
                break;
            }
            if st == STATE_EOF {
                break;
            }
        }
        assert!(saw_error, "非法 UTF-8 被静默吞掉了");
        qi_http_stream_close(stream);
    }

    #[test]
    fn status_and_headers_readable() {
        let endpoint = serve_once(
            vec![b"nope".to_vec()],
            "HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\nX-Qi-Test: abc-123\r\nX-Qi-Bad: 值\r\nConnection: close\r\n\r\n",
            Duration::ZERO,
        );
        let stream = open_stream(&endpoint);
        assert!(stream > 0);
        assert_eq!(qi_http_stream_status(stream), 404);
        let header: serde_json::Value =
            serde_json::from_str(&to_string_owned(qi_http_stream_headers(stream))).unwrap();
        assert_eq!(header["content-type"], serde_json::json!("text/plain"));
        assert_eq!(header["x-qi-test"], serde_json::json!("abc-123"));
        // 非 ASCII 头值不合 HTTP 规范，但不能因此让这一条**消失** ——
        // 消失了 qi 侧会以为服务端没发这个头，查错方向。
        assert_eq!(
            header["x-qi-bad"],
            serde_json::json!("值"),
            "不规范的头值被丢掉了"
        );
        qi_http_stream_close(stream);
    }

    #[test]
    fn close_invalidates_handle_and_is_idempotent() {
        let endpoint = serve_once(vec![b"x".to_vec()], FIXED_HEADER, Duration::ZERO);
        let stream = open_stream(&endpoint);
        assert!(stream > 0);
        assert_eq!(qi_http_stream_close(stream), 1);
        assert_eq!(qi_http_stream_close(stream), 0, "重复关闭应无副作用");
        assert_eq!(qi_http_stream_state(stream), STATE_NO_STREAM);
        assert_eq!(qi_http_stream_status(stream), -1);
        assert_eq!(to_string_owned(qi_http_stream_read(stream, 10)), "");
    }

    /// handles_are_never_reused：关掉一条再开一条，新句柄不能等于旧的。
    #[test]
    fn handles_are_never_reused() {
        let endpoint = serve_once(vec![b"a".to_vec()], FIXED_HEADER, Duration::ZERO);
        let head = open_stream(&endpoint);
        qi_http_stream_close(head);
        let endpoint2 = serve_once(vec![b"b".to_vec()], FIXED_HEADER, Duration::ZERO);
        let rest = open_stream(&endpoint2);
        assert_ne!(head, rest, "句柄被复用了");
        qi_http_stream_close(rest);
    }

    #[test]
    fn bad_args_do_not_crash() {
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
