//! 网络模块 FFI 接口
//!
//! 为 Qi 语言提供 C 接口的网络操作函数（TCP、UDP 等）

#![allow(non_snake_case)]

use super::http::{NetworkInterface, TcpConnection, TcpConnectionConfig};
use dashmap::DashMap;
use std::collections::HashMap;
use std::ffi::{c_void, CStr};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

// 取 listener 的原始套接字句柄（i32），用于登记进信号关停名单。
// Unix 下是 RawFd；Windows 下 SOCKET 句柄被官方保证可用 32 位表示
// （见 MSDN：socket handles fit in 32 bits for interop），故 `as i32` 安全。
#[cfg(unix)]
fn 原始套接字<T: std::os::fd::AsRawFd>(s: &T) -> i32 {
    s.as_raw_fd()
}
#[cfg(windows)]
fn 原始套接字<T: std::os::windows::io::AsRawSocket>(s: &T) -> i32 {
    s.as_raw_socket() as i32
}

// 全局网络接口实例
use std::sync::OnceLock;
static 全局网络接口: OnceLock<NetworkInterface> = OnceLock::new();

// 用 DashMap 替换 Mutex<HashMap>：连接查找走分片锁，**不同句柄的并发操作不再
// 互相阻塞**。每个 TcpConnection 还是裹一层 Mutex —— 是为了 read/write 需要
// &mut self；因为一条连接同一时刻只有一个 goroutine 在用，这个内层 Mutex
// 几乎永远不会真的竞争。
//
// 值类型是 **Arc**<Mutex<TcpConnection>>，与 TCP服务器池 的 Arc<TcpListener>
// 同一课教训：任何 IO（尤其阻塞 read）之前必须先克隆 Arc、立刻放掉 DashMap
// 的 shard guard。曾经 read/write 在持 shard 读 guard 的状态下阻塞在 socket
// 上（keep-alive 连接常态：goroutine 停在 read 里等下一条请求）——此时任何
// 落在同一 shard 的 insert（accept 新连接）/ remove（关连接）都要等写锁，
// 而挂起的写锁又挡住该 shard 后续所有读者：一次 accept 就能把整个 shard 上的
// 活跃连接冻住几百毫秒，超时→重连→更多 accept，雪崩式尾延迟
// （wrk 实测 99% 从 µs 级尖到 323ms+ / 一轮 20 个超时都源于此）。
static TCP连接池: OnceLock<DashMap<i64, std::sync::Arc<Mutex<TcpConnection>>>> = OnceLock::new();
static 连接句柄计数器: AtomicI64 = AtomicI64::new(0);

fn 获取网络接口() -> Option<&'static NetworkInterface> {
    全局网络接口.get()
}

fn 初始化网络接口() {
    全局网络接口.get_or_init(|| {
        NetworkInterface::new().unwrap_or_else(|_| panic!("Failed to initialize network interface"))
    });
}

fn 获取连接池() -> &'static DashMap<i64, std::sync::Arc<Mutex<TcpConnection>>> {
    TCP连接池.get_or_init(DashMap::new)
}

/// 按句柄取连接的 Arc 克隆 —— shard guard 在本函数内立即释放，
/// 调用方随后在 guard 之外做（可能阻塞的）IO。
#[inline]
fn 取连接(handle: i64) -> Option<std::sync::Arc<Mutex<TcpConnection>>> {
    获取连接池().get(&handle).map(|e| e.value().clone())
}

fn 下一个句柄() -> i64 {
    连接句柄计数器.fetch_add(1, Ordering::Relaxed) + 1
}

/// 从TCP连接池中取出连接并返回TcpStream（用于WebSocket升级）
/// 这将从池中移除连接，调用者获得TcpStream的所有权
pub(crate) fn 取出TCP流(handle: i64) -> Option<std::net::TcpStream> {
    let (_, arc) = 获取连接池().remove(&handle)?;
    // 常态：本连接只有当前 goroutine 在用，remove 后 Arc 唯一，直接拆箱。
    // 兜底：极端并发下别的线程还持着克隆（瞬态 IO 中），退回 dup fd
    // （try_clone_stream），残余 Arc 随其使用结束自然释放。
    match std::sync::Arc::try_unwrap(arc) {
        Ok(mu) => Some(
            mu.into_inner()
                .unwrap_or_else(|e| e.into_inner())
                .into_stream(),
        ),
        Err(arc) => {
            let conn = arc.lock().unwrap_or_else(|e| e.into_inner());
            conn.try_clone_stream().ok()
        }
    }
}

/// 克隆TCP连接的流（保留原连接在池中）
pub(crate) fn 克隆TCP流(handle: i64) -> Option<std::net::TcpStream> {
    let arc = 取连接(handle)?;
    let conn = arc.lock().unwrap_or_else(|e| e.into_inner());
    conn.try_clone_stream().ok()
}

/// 初始化网络模块
#[no_mangle]
pub extern "C" fn qi_network_init() {
    初始化网络接口();
}

// ===== Listener 模式控制（详细实现在文件后部，访问 TCP服务器池）=====

/// TCP 连接到指定地址和端口
/// 返回连接句柄（>0 成功，<0 失败）
#[no_mangle]
pub extern "C" fn qi_network_tcp_connect(host: *const c_char, port: u16, timeout_ms: i64) -> i64 {
    if host.is_null() {
        return -1;
    }

    // 确保网络接口已初始化
    if 获取网络接口().is_none() {
        初始化网络接口();
    }

    unsafe {
        let 主机 = CStr::from_ptr(host).to_string_lossy().to_string();
        let mut 配置 = TcpConnectionConfig::new(主机.clone(), port);

        if timeout_ms > 0 {
            配置 = 配置.with_timeout(Duration::from_millis(timeout_ms as u64));
        }

        match TcpConnection::connect(配置) {
            Ok(连接) => {
                let 句柄 = 下一个句柄();
                获取连接池().insert(句柄, std::sync::Arc::new(Mutex::new(连接)));
                句柄
            }
            Err(_) => -1,
        }
    }
}

/// 从 TCP 连接读取数据
/// 返回实际读取的字节数（<0 表示错误）
#[no_mangle]
pub extern "C" fn qi_network_tcp_read(handle: i64, buffer: *mut u8, buffer_size: i64) -> i64 {
    if buffer.is_null() || buffer_size <= 0 {
        return -1;
    }

    // 先克隆 Arc 放掉 shard guard，再做阻塞 read —— 不挡池上的 insert/remove
    if let Some(连接arc) = 取连接(handle) {
        let mut 连接 = 连接arc.lock().unwrap();
        let 缓冲区 = unsafe { std::slice::from_raw_parts_mut(buffer, buffer_size as usize) };
        match 连接.read(缓冲区) {
            Ok(字节数) => 字节数 as i64,
            Err(_) => -1,
        }
    } else {
        -1
    }
}

/// 向 TCP 连接写入数据
/// 返回实际写入的字节数（<0 表示错误）
#[no_mangle]
pub extern "C" fn qi_network_tcp_write(handle: i64, data: *const u8, data_size: i64) -> i64 {
    if data.is_null() || data_size <= 0 {
        return -1;
    }

    if let Some(连接arc) = 取连接(handle) {
        let mut 连接 = 连接arc.lock().unwrap();
        let 数据 = unsafe { std::slice::from_raw_parts(data, data_size as usize) };
        match 连接.write(数据) {
            Ok(字节数) => 字节数 as i64,
            Err(_) => -1,
        }
    } else {
        -1
    }
}

/// 从 TCP 连接读取数据并返回为字符串（高级版本）
/// 返回接收到的数据字符串，失败返回空字符串
#[no_mangle]
pub extern "C" fn qi_network_tcp_read_string(handle: i64, buffer_size: i64) -> *mut c_char {
    if buffer_size <= 0 {
        return crate::stdlib::qi_str::rc_cstr_from_str("");
    }

    let mut 缓冲区 = vec![0u8; buffer_size as usize];

    if let Some(连接arc) = 取连接(handle) {
        let mut 连接 = 连接arc.lock().unwrap();
        if let Ok(size) = 连接.read(&mut 缓冲区) {
            if size > 0 {
                if let Ok(字符串) = String::from_utf8(缓冲区[..size].to_vec()) {
                    return crate::stdlib::qi_str::rc_cstr_from_string(字符串);
                }
                let 字符串 = String::from_utf8_lossy(&缓冲区[..size]).to_string();
                return crate::stdlib::qi_str::rc_cstr_from_string(字符串);
            }
        }
    }

    crate::stdlib::qi_str::rc_cstr_from_str("")
}

/// 向 TCP 连接写入字符串数据（高级版本）
/// 返回写入的字节数（<0 表示错误）
#[no_mangle]
pub extern "C" fn qi_network_tcp_write_string(handle: i64, data: *const c_char) -> i64 {
    if data.is_null() {
        return -1;
    }

    unsafe {
        let 数据字符串 = CStr::from_ptr(data).to_string_lossy();
        let 数据字节 = 数据字符串.as_bytes();

        if let Some(连接arc) = 取连接(handle) {
            let mut 连接 = 连接arc.lock().unwrap();
            match 连接.write(数据字节) {
                Ok(字节数) => {
                    let _ = 连接.flush();
                    字节数 as i64
                }
                Err(_) => -1,
            }
        } else {
            -1
        }
    }
}

/// 关闭 TCP 连接
/// 返回 1 成功，0 失败
#[no_mangle]
pub extern "C" fn qi_network_tcp_close(handle: i64) -> i64 {
    if 获取连接池().remove(&handle).is_some() {
        1
    } else {
        0
    }
}

/// TCP 刷新缓冲区
/// 返回 1 成功，0 失败
#[no_mangle]
pub extern "C" fn qi_network_tcp_flush(handle: i64) -> i64 {
    if let Some(连接arc) = 取连接(handle) {
        let mut 连接 = 连接arc.lock().unwrap();
        match 连接.flush() {
            Ok(_) => 1,
            Err(_) => 0,
        }
    } else {
        0
    }
}

/// 获取 TCP 连接已读取的字节数
#[no_mangle]
pub extern "C" fn qi_network_tcp_bytes_read(handle: i64) -> i64 {
    if let Some(连接arc) = 取连接(handle) {
        let 连接 = 连接arc.lock().unwrap();
        连接.bytes_read() as i64
    } else {
        -1
    }
}

/// 获取 TCP 连接已写入的字节数
#[no_mangle]
pub extern "C" fn qi_network_tcp_bytes_written(handle: i64) -> i64 {
    if let Some(连接arc) = 取连接(handle) {
        let 连接 = 连接arc.lock().unwrap();
        连接.bytes_written() as i64
    } else {
        -1
    }
}

/// 解析域名到 IP 地址
/// 返回 IP 地址字符串（需要调用 qi_network_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_network_resolve_host(host: *const c_char) -> *mut c_char {
    if host.is_null() {
        return std::ptr::null_mut();
    }

    unsafe {
        let 主机名 = CStr::from_ptr(host).to_string_lossy().to_string();

        // 尝试解析为 socket 地址
        use std::net::ToSocketAddrs;
        let 地址字符串 = format!("{}:0", 主机名);

        match 地址字符串.to_socket_addrs() {
            Ok(mut 地址列表) => {
                if let Some(地址) = 地址列表.next() {
                    let ip字符串 = 地址.ip().to_string();
                    crate::stdlib::qi_str::rc_cstr_from_string(ip字符串)
                } else {
                    std::ptr::null_mut()
                }
            }
            Err(_) => std::ptr::null_mut(),
        }
    }
}

/// 检查端口是否可用
/// 返回 1 可用，0 不可用
#[no_mangle]
pub extern "C" fn qi_network_port_available(port: u16) -> i64 {
    use std::net::TcpListener;

    match TcpListener::bind(("127.0.0.1", port)) {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

/// 获取本机 IP 地址
/// 返回 IP 地址字符串（需要调用 qi_network_free_string 释放）
#[no_mangle]
pub extern "C" fn qi_network_get_local_ip() -> *mut c_char {
    use std::net::UdpSocket;

    // 使用 UDP 连接到外部地址获取本机 IP
    match UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => match socket.connect("8.8.8.8:80") {
            Ok(_) => match socket.local_addr() {
                Ok(addr) => {
                    let ip = addr.ip().to_string();
                    crate::stdlib::qi_str::rc_cstr_from_string(ip)
                }
                Err(_) => crate::stdlib::qi_str::rc_cstr_from_str("127.0.0.1"),
            },
            Err(_) => crate::stdlib::qi_str::rc_cstr_from_str("127.0.0.1"),
        },
        Err(_) => crate::stdlib::qi_str::rc_cstr_from_str("127.0.0.1"),
    }
}

/// 释放网络模块分配的字符串内存
/// （委托 rc_cstr_release：非 RC 指针一次性警告后静默泄漏，不崩溃）
#[no_mangle]
pub extern "C" fn qi_network_free_string(s: *mut c_char) {
    crate::stdlib::qi_str::rc_cstr_release(s);
}

// ============================================================================
// TCP 服务器功能
// ============================================================================

use std::net::TcpListener;
use std::sync::Arc;

// TCP 服务器监听器池：用 DashMap，listener.accept(&self) 不需要内层锁
static TCP服务器池: OnceLock<DashMap<i64, Arc<TcpListener>>> = OnceLock::new();

fn 获取服务器池() -> &'static DashMap<i64, Arc<TcpListener>> {
    TCP服务器池.get_or_init(DashMap::new)
}

/// 创建 TCP 服务器监听指定端口
/// 返回服务器句柄（>0 成功，<0 失败）
#[no_mangle]
pub extern "C" fn qi_network_tcp_listen(host: *const c_char, port: u16, _backlog: i32) -> i64 {
    if host.is_null() {
        return -1;
    }

    unsafe {
        let 主机 = CStr::from_ptr(host).to_string_lossy().to_string();
        let 地址 = format!("{}:{}", 主机, port);

        match TcpListener::bind(&地址) {
            Ok(listener) => {
                // 登记 listener 的 raw socket —— SIGINT 时 handler 会 shutdown 它，
                // 让任何阻塞在 accept() 上的线程立即返回 -1。
                let raw_fd = 原始套接字(&listener);
                crate::stdlib::signal_ffi::qi_signal_register_listener_fd(raw_fd);

                let 句柄 = 下一个句柄();
                获取服务器池().insert(句柄, Arc::new(listener));
                句柄
            }
            Err(_) => -1,
        }
    }
}

/// 接受 TCP 客户端连接（阻塞）
/// 返回客户端连接句柄（>0 成功，<0 失败）
#[no_mangle]
pub extern "C" fn qi_network_tcp_accept(server_handle: i64) -> i64 {
    // 关键：先把 Arc 克隆出来，立刻 drop dashmap 的 shard guard。
    // 否则 accept() 可能阻塞数小时，期间这个 shard 上其他 listener 操作全卡死。
    let listener = match 获取服务器池().get(&server_handle) {
        Some(entry) => entry.clone(),
        None => return -1,
    };

    match listener.accept() {
        Ok((stream, _addr)) => match TcpConnection::from_stream(stream) {
            Ok(连接) => {
                let 句柄 = 下一个句柄();
                获取连接池().insert(句柄, std::sync::Arc::new(Mutex::new(连接)));
                句柄
            }
            Err(_) => -1,
        },
        Err(_) => -1,
    }
}

/// 关闭 TCP 服务器
/// 返回 1 成功，0 失败
#[no_mangle]
pub extern "C" fn qi_network_tcp_server_close(server_handle: i64) -> i64 {
    match 获取服务器池().remove(&server_handle) {
        Some((_, listener)) => {
            crate::stdlib::signal_ffi::qi_signal_unregister_listener_fd(原始套接字(
                &*listener,
            ));
            1
        }
        None => 0,
    }
}

/// 二进制安全读：把读到的字节直接放进 字节切片 池
/// 返回字节切片句柄；连接关闭返回 0；错误返回 -1
#[no_mangle]
pub extern "C" fn qi_network_tcp_read_bytes(handle: i64, buffer_size: i64) -> i64 {
    let size = if buffer_size <= 0 {
        4096
    } else {
        buffer_size as usize
    };
    let mut buf = vec![0u8; size];

    // 关键路径：keep-alive 连接的 goroutine 大部分时间阻塞在这个 read 里等
    // 下一条请求 —— 必须先克隆 Arc 放掉 shard guard 再 read，绝不能持 guard 阻塞。
    let n = match 取连接(handle) {
        Some(连接arc) => {
            let mut conn = 连接arc.lock().unwrap();
            match conn.read(&mut buf) {
                Ok(n) => n,
                Err(_) => return -1,
            }
        }
        None => return -1,
    };
    if n == 0 {
        return 0;
    }
    buf.truncate(n);
    crate::stdlib::bytes_ffi::register_bytes(buf)
}

/// 二进制安全写：从 字节切片 句柄读出字节写入连接
/// 返回写入字节数；错误返回 -1
/// 负句柄 = 持久字节（预构建缓存响应）：克隆 Arc 借用直写，零拷贝。
#[no_mangle]
pub extern "C" fn qi_network_tcp_write_bytes(handle: i64, bytes_handle: i64) -> i64 {
    // 两种来源统一成 &[u8]：持久句柄零拷贝（Arc），普通句柄保持原 clone 语义
    let 持久;
    let 普通;
    let data: &[u8] = if bytes_handle < 0 {
        match crate::stdlib::bytes_ffi::persistent_arc(bytes_handle) {
            Some(a) => {
                持久 = a;
                &持久
            }
            None => return -1,
        }
    } else {
        match crate::stdlib::bytes_ffi::clone_bytes(bytes_handle) {
            Some(v) => {
                普通 = v;
                &普通
            }
            None => return -1,
        }
    };

    if let Some(连接arc) = 取连接(handle) {
        let mut c = 连接arc.lock().unwrap();
        let mut written = 0usize;
        while written < data.len() {
            match c.write(&data[written..]) {
                Ok(0) => return -1,
                Ok(n) => written += n,
                Err(_) => return -1,
            }
        }
        let _ = c.flush();
        written as i64
    } else {
        -1
    }
}

/// 把指定 listener 设置为非阻塞或阻塞模式
/// 现在的服务器主循环用阻塞 accept + 信号 shutdown listener 的方式优雅关闭，
/// 这个开关保留主要是为了兼容老代码或别的用例。
#[no_mangle]
pub extern "C" fn qi_network_tcp_listener_set_nonblocking(
    server_handle: i64,
    nonblocking: i64,
) -> i64 {
    if let Some(entry) = 获取服务器池().get(&server_handle) {
        match entry.set_nonblocking(nonblocking != 0) {
            Ok(_) => 0,
            Err(_) => -1,
        }
    } else {
        -1
    }
}

// ============================================================================
// UDP 功能
// ============================================================================

use std::net::UdpSocket;

// UDP Socket 池。值是 Arc<UdpSocket>：UdpSocket 的 send_to/recv_from/set_* 全是
// &self，无需内层 Mutex——取用时克隆 Arc、立刻释放池锁，阻塞 IO 全在锁外做。
// 否则（旧实现）一个套接字阻塞在 recv_from 会攥着全局锁，冻结所有 UDP 操作，
// 与 TCP 连接池「持 shard guard 做阻塞 IO」是同一类病（TCP 版已修，见上）。
static UDP套接字池: OnceLock<Mutex<HashMap<i64, Arc<UdpSocket>>>> = OnceLock::new();

#[allow(non_snake_case)]
fn 获取UDP池() -> &'static Mutex<HashMap<i64, Arc<UdpSocket>>> {
    UDP套接字池.get_or_init(|| Mutex::new(HashMap::new()))
}

/// 取出句柄对应的套接字克隆（Arc），池锁只握住查表这一瞬间。
fn 取UDP(handle: i64) -> Option<Arc<UdpSocket>> {
    获取UDP池()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .get(&handle)
        .cloned()
}

/// 创建 UDP Socket 并绑定到指定地址和端口
/// 返回 Socket 句柄（>0 成功，<0 失败）
#[no_mangle]
pub extern "C" fn qi_network_udp_bind(host: *const c_char, port: u16) -> i64 {
    if host.is_null() {
        return -1;
    }

    unsafe {
        let 主机 = CStr::from_ptr(host).to_string_lossy().to_string();
        let 地址 = format!("{}:{}", 主机, port);

        match UdpSocket::bind(&地址) {
            Ok(socket) => {
                let 句柄 = 下一个句柄();
                获取UDP池()
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .insert(句柄, Arc::new(socket));
                句柄
            }
            Err(_) => -1,
        }
    }
}

/// UDP 发送字符串到指定地址（简化版本）
/// 返回发送的字节数（<0 表示错误）
#[no_mangle]
pub extern "C" fn qi_network_udp_send_string(
    handle: i64,
    message: *const c_char,
    host: *const c_char,
    port: u16,
) -> i64 {
    if message.is_null() || host.is_null() {
        return -1;
    }

    unsafe {
        let 消息 = CStr::from_ptr(message).to_string_lossy();
        let 目标主机 = CStr::from_ptr(host).to_string_lossy().to_string();
        let 目标地址 = format!("{}:{}", 目标主机, port);

        // 克隆 Arc 即放池锁，send_to 在锁外做
        let Some(socket) = 取UDP(handle) else {
            return -1;
        };
        match socket.send_to(消息.as_bytes(), &目标地址) {
            Ok(字节数) => 字节数 as i64,
            Err(_) => -1,
        }
    }
}

/// UDP 发送数据到指定地址
/// 返回发送的字节数（<0 表示错误）
#[no_mangle]
pub extern "C" fn qi_network_udp_send_to(
    handle: i64,
    data: *const u8,
    data_size: i64,
    host: *const c_char,
    port: u16,
) -> i64 {
    if data.is_null() || data_size <= 0 || host.is_null() {
        return -1;
    }

    unsafe {
        let 目标主机 = CStr::from_ptr(host).to_string_lossy().to_string();
        let 目标地址 = format!("{}:{}", 目标主机, port);

        // 克隆 Arc 即放池锁，send_to 在锁外做
        let Some(socket) = 取UDP(handle) else {
            return -1;
        };
        let 数据 = std::slice::from_raw_parts(data, data_size as usize);
        match socket.send_to(数据, &目标地址) {
            Ok(字节数) => 字节数 as i64,
            Err(_) => -1,
        }
    }
}

/// UDP 接收数据（阻塞）
/// 返回接收的字节数（<0 表示错误）
/// sender_host 和 sender_port 用于返回发送方地址（可选）
#[no_mangle]
pub extern "C" fn qi_network_udp_recv_from(
    handle: i64,
    buffer: *mut u8,
    buffer_size: i64,
    sender_host: *mut *mut c_char,
    sender_port: *mut u16,
) -> i64 {
    if buffer.is_null() || buffer_size <= 0 {
        return -1;
    }

    // 关键路径：recv_from 是阻塞调用，必须在池锁外做——克隆 Arc 即放锁。
    // 旧实现持全局锁阻塞等包，会把所有 UDP 操作（含 close/set_timeout）一起冻住。
    let Some(socket) = 取UDP(handle) else {
        return -1;
    };
    let 缓冲区 = unsafe { std::slice::from_raw_parts_mut(buffer, buffer_size as usize) };

    match socket.recv_from(缓冲区) {
        Ok((字节数, 地址)) => {
            // 如果提供了发送方信息指针，填充它们
            if !sender_host.is_null() {
                let ip字符串 = 地址.ip().to_string();
                unsafe {
                    *sender_host = crate::stdlib::qi_str::rc_cstr_from_string(ip字符串);
                }
            }
            if !sender_port.is_null() {
                unsafe {
                    *sender_port = 地址.port();
                }
            }
            字节数 as i64
        }
        Err(_) => -1,
    }
}

/// UDP 接收数据并返回为字符串（简化版本）
/// 返回接收到的数据字符串，失败返回空字符串
#[no_mangle]
pub extern "C" fn qi_network_udp_recv_string(handle: i64, buffer_size: i64) -> *mut c_char {
    if buffer_size <= 0 {
        return crate::stdlib::qi_str::rc_cstr_from_str("");
    }

    let mut 缓冲区 = vec![0u8; buffer_size as usize];
    // 阻塞 recv 在池锁外做（克隆 Arc 即放锁）
    if let Some(socket) = 取UDP(handle) {
        match socket.recv_from(&mut 缓冲区) {
            Ok((size, _sender_addr)) => {
                if size > 0 {
                    if let Ok(字符串) = String::from_utf8(缓冲区[..size].to_vec()) {
                        return crate::stdlib::qi_str::rc_cstr_from_string(字符串);
                    }
                }
            }
            Err(_) => {}
        }
    }

    crate::stdlib::qi_str::rc_cstr_from_str("")
}

/// 关闭 UDP Socket
/// 返回 1 成功，0 失败
#[no_mangle]
pub extern "C" fn qi_network_udp_close(handle: i64) -> i64 {
    // 从池里摘掉即认为关闭；若别的线程正阻塞在 recv（持有 Arc 克隆），
    // 套接字随其调用返回后自然释放——语义与 TCP 池修复一致。
    let 有 = 获取UDP池()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .remove(&handle)
        .is_some();
    if 有 {
        1
    } else {
        0
    }
}

/// 设置 UDP Socket 超时时间（毫秒）
/// 返回 1 成功，0 失败
#[no_mangle]
pub extern "C" fn qi_network_udp_set_timeout(handle: i64, timeout_ms: i64) -> i64 {
    let Some(socket) = 取UDP(handle) else {
        return 0;
    };
    let 超时 = if timeout_ms > 0 {
        Some(Duration::from_millis(timeout_ms as u64))
    } else {
        None
    };

    match socket.set_read_timeout(超时) {
        Ok(_) => match socket.set_write_timeout(超时) {
            Ok(_) => 1,
            Err(_) => 0,
        },
        Err(_) => 0,
    }
}

/// 设置 UDP 广播模式
/// 返回 1 成功，0 失败
#[no_mangle]
pub extern "C" fn qi_network_udp_set_broadcast(handle: i64, enable: i32) -> i64 {
    let Some(socket) = 取UDP(handle) else {
        return 0;
    };
    match socket.set_broadcast(enable != 0) {
        Ok(_) => 1,
        Err(_) => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn bind_udp_ephemeral(host: &CString) -> (i64, u16) {
        let handle = qi_network_udp_bind(host.as_ptr(), 0);
        assert!(handle > 0, "failed to bind OS-assigned UDP port");

        let port = 取UDP(handle)
            .and_then(|socket| socket.local_addr().ok())
            .map(|address| address.port())
            .unwrap_or_else(|| {
                qi_network_udp_close(handle);
                panic!("failed to read OS-assigned UDP port");
            });
        (handle, port)
    }

    #[test]
    fn test_network_init() {
        qi_network_init();
        unsafe {
            assert!(全局网络接口.get().is_some());
        }
    }

    #[test]
    fn test_port_available() {
        // 测试一个不太可能被占用的端口
        let result = qi_network_port_available(54321);
        assert!(result == 1 || result == 0); // 可能可用或不可用
    }

    #[test]
    fn test_get_local_ip() {
        let ip_ptr = qi_network_get_local_ip();
        assert!(!ip_ptr.is_null());

        let ip_str = unsafe { CStr::from_ptr(ip_ptr).to_string_lossy() };
        assert!(!ip_str.is_empty());

        qi_network_free_string(ip_ptr);
    }

    #[test]
    fn test_resolve_host() {
        let host = CString::new("localhost").unwrap();
        let ip_ptr = qi_network_resolve_host(host.as_ptr());

        if !ip_ptr.is_null() {
            let ip_str = unsafe { CStr::from_ptr(ip_ptr).to_string_lossy() };
            println!("Resolved localhost to: {}", ip_str);
            qi_network_free_string(ip_ptr);
        }
    }

    /// 回归测试：一个套接字阻塞在 recv_from 时，其它 UDP 操作不得被冻结。
    /// 旧实现（Mutex<HashMap<i64, UdpSocket>> 持全局锁做阻塞 IO）下本测试会
    /// 卡死在 B 的 send/recv 上直到 A 收到包；Arc 化修复后应毫秒级完成。
    #[test]
    fn test_udp_blocked_recv_does_not_freeze_pool() {
        let 主机 = CString::new("127.0.0.1").unwrap();
        let (a, 端口a) = bind_udp_ephemeral(&主机);
        let (b, 端口b) = bind_udp_ephemeral(&主机);

        // 线程 1：A 无超时阻塞等包（旧实现会在此攥住全局池锁）
        let 收线程 = std::thread::spawn(move || {
            let mut 缓冲 = [0u8; 64];
            qi_network_udp_recv_from(
                a,
                缓冲.as_mut_ptr(),
                缓冲.len() as i64,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        });
        // 确保线程 1 已进入阻塞 recv
        std::thread::sleep(std::time::Duration::from_millis(150));

        // 主线程：B 自发自收一个包——必须在 A 仍阻塞期间完成
        let 开始 = std::time::Instant::now();
        assert_eq!(qi_network_udp_set_timeout(b, 2000), 1);
        let 消息 = CString::new("ping").unwrap();
        let 发 = qi_network_udp_send_string(b, 消息.as_ptr(), 主机.as_ptr(), 端口b);
        assert_eq!(发, 4, "B send 失败");
        let mut 缓冲 = [0u8; 64];
        let 收 = qi_network_udp_recv_from(
            b,
            缓冲.as_mut_ptr(),
            缓冲.len() as i64,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        assert_eq!(收, 4, "B recv 失败");
        assert!(
            开始.elapsed() < std::time::Duration::from_secs(1),
            "B 的收发被 A 的阻塞 recv 冻结了 {:?} —— 池锁又被持锁 IO 攥住了",
            开始.elapsed()
        );

        // 给 A 发一个包解除阻塞，回收线程与句柄
        let 解 = CString::new("bye").unwrap();
        assert_eq!(
            qi_network_udp_send_string(b, 解.as_ptr(), 主机.as_ptr(), 端口a),
            3
        );
        assert_eq!(收线程.join().unwrap(), 3);
        assert_eq!(qi_network_udp_close(a), 1);
        assert_eq!(qi_network_udp_close(b), 1);
    }
}

// ============================================================================
// 真 M:N 异步服务器
// ============================================================================
//
// 设计：用 tokio multi-threaded runtime + tokio::net 接管整个 listener +
// per-connection 的生命周期。每条连接是一个 tokio 任务，read/write 全 async
// —— IO 等待时让出 worker，跟 Go 的 net/http + netpoller 一个路子。
// qi handler 仍是同步函数，每个请求处理时短暂占用 tokio worker（μs 级），
// 不会显著影响调度。
//
// **不支持**的场景（这版先不动）：
//   - 流式响应（handler 返回 0 表示自己写完）
//   - TLS / HTTP/2 / WebSocket 升级
//   - keep-alive 之外的复杂连接管理
//
// 这版的目的：证明 M:N 在简单 HTTP 上能拿到大幅提升。

// 复用 async_runtime 模块里的全局 tokio runtime
use crate::async_runtime::ffi::全局异步运行时 as 异步运行时;

// Qi 把函数值统一包成 closure 对象传过来：
//   offset 0..8  : trampoline 函数指针
//   offset 8..   : 捕获槽（这里都是 0 个捕获，所以无所谓）
// trampoline 的签名是 extern "C" fn(env, ...args) — 第一个参数是 closure 对象本身。
// 所以调用时要先读 fn_ptr，再传 env+args 调它。

/// 处理函数 trampoline: (env, app, req_bytes_handle, client_handle) → resp_bytes_handle
type HandlerTrampoline = extern "C" fn(*const c_void, *const c_void, i64, i64) -> i64;

/// 从 closure 对象的 offset 0 读出 trampoline 函数指针
unsafe fn closure_trampoline<T>(closure_obj: *const c_void) -> T
where
    T: Copy,
{
    debug_assert_eq!(
        std::mem::size_of::<T>(),
        std::mem::size_of::<*const c_void>()
    );
    let fn_ptr = *(closure_obj as *const *const c_void);
    *(&fn_ptr as *const *const c_void as *const T)
}

#[inline]
fn invoke_handler(closure_obj: usize, app: *const c_void, req: i64, client: i64) -> i64 {
    let env = closure_obj as *const c_void;
    unsafe {
        let trampoline: HandlerTrampoline = closure_trampoline(env);
        trampoline(env, app, req, client)
    }
}

/// 启动异步服务器：takes ownership of the listener at server_handle，
/// 用 tokio 接管 accept 循环 + 每条连接的 IO，调 qi 侧的 handler_fn 处理请求。
///
/// HTTP 请求完整性（headers + Content-Length）在 Rust 侧直接检测 —— 比每个
/// chunk 调一次 Qi closure trampoline 便宜得多，对小请求基本是零开销路径。
///
/// 注意：handler_fn 是 Qi 的 *closure 对象* 指针，不是裸函数指针；
/// 调用它要走 trampoline（见 invoke_handler）。
/// 收到 SIGINT 时返回 0；其他错误返回 -1。
#[no_mangle]
pub extern "C" fn qi_runtime_async_serve(
    server_handle: i64,
    handler_fn: *const c_void,
    app_ptr: *const c_void,
) -> i64 {
    // 把 listener 从同步池里取出来 —— tokio 要 owning std listener。
    let listener_arc = match 获取服务器池().remove(&server_handle) {
        Some((_, arc)) => arc,
        None => return -1,
    };

    // 由于 Arc 可能还有别的引用（理论上不该，因为我们刚 remove 出来唯一持有方），
    // 用 try_unwrap，失败就 try_clone 一份（跨平台，内部走 dup/WSADuplicateSocket）。
    let std_listener = match std::sync::Arc::try_unwrap(listener_arc) {
        Ok(l) => l,
        Err(arc) => match arc.try_clone() {
            Ok(l) => l,
            Err(_) => return -1,
        },
    };

    if std_listener.set_nonblocking(true).is_err() {
        return -1;
    }

    // 指针跨线程 —— 包成 usize 走 Send。
    let app_addr = app_ptr as usize;
    let handler_addr = handler_fn as usize;

    异步运行时().block_on(async move {
        let tokio_listener = match tokio::net::TcpListener::from_std(std_listener) {
            Ok(l) => l,
            Err(_) => return,
        };
        loop {
            tokio::select! {
                accept_result = tokio_listener.accept() => {
                    match accept_result {
                        Ok((stream, _addr)) => {
                            let _ = stream.set_nodelay(true);
                            tokio::spawn(handle_conn_async(stream, handler_addr, app_addr));
                        }
                        Err(_) => break,
                    }
                }
                _ = 关闭信号_watcher() => break,
            }
        }
    });

    0
}

/// 异步关闭信号 watcher：100ms 周期轮询关闭标志。
async fn 关闭信号_watcher() {
    loop {
        if crate::stdlib::signal_ffi::qi_signal_should_shutdown() != 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn handle_conn_async(
    mut stream: tokio::net::TcpStream,
    handler_addr: usize,
    app_addr: usize,
) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // PROBE: 是否启用纯 Rust 硬编码响应（跳过 qi handler）。
    // 用于诊断"瓶颈是 qi handler 还是 IO 层"。
    let probe_rust_only = std::env::var("QI_BENCH_RUST_ONLY").is_ok();
    const HARDCODED_RESPONSE: &[u8] =
        b"HTTP/1.1 200 OK\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: keep-alive\r\nContent-Length: 2\r\n\r\nok";

    let mut read_buf = vec![0u8; 16384];
    let mut accumulated: Vec<u8> = Vec::with_capacity(16384);

    loop {
        // 读到完整请求为止 —— 完整性检测在 Rust 内联，不走 Qi 回调
        loop {
            match stream.read(&mut read_buf).await {
                Ok(0) => return, // peer close
                Ok(n) => accumulated.extend_from_slice(&read_buf[..n]),
                Err(_) => return,
            }
            if http_request_complete(&accumulated) {
                break;
            }
        }

        // 决定 keep-alive（在 move 之前 borrow 一下）
        let keep_alive = !request_has_connection_close(&accumulated);

        if probe_rust_only {
            // 纯 Rust 路径：跳过 qi handler，直接写硬编码响应
            accumulated.clear();
            if stream.write_all(HARDCODED_RESPONSE).await.is_err() {
                return;
            }
            if !keep_alive {
                return;
            }
            continue;
        }

        // 整个 buffer move 进字节池，不 clone。下一轮请求重新 alloc。
        let req_bytes = std::mem::take(&mut accumulated);
        let req_handle = crate::stdlib::bytes_ffi::register_bytes(req_bytes);

        // 同步调 qi handler — 这是 μs 级 CPU 工作，短暂占用 tokio worker 没事
        let resp_handle = invoke_handler(handler_addr, app_addr as *const c_void, req_handle, 0);
        // handler 可能已经释放了；no-op 再来一次
        crate::stdlib::bytes_ffi::free_bytes(req_handle);

        // 0 = handler 已自行流式写完；负数是**合法的持久句柄**（预构建缓存响应），不是错误
        if resp_handle == 0 {
            return;
        }

        // 取响应字节，async 写回。负句柄 = 预构建持久响应：Arc 借用零拷贝直写。
        if resp_handle < 0 {
            let arc = match crate::stdlib::bytes_ffi::persistent_arc(resp_handle) {
                Some(a) => a,
                None => return,
            };
            if stream.write_all(&arc).await.is_err() {
                return;
            }
        } else {
            let resp = match crate::stdlib::bytes_ffi::take_bytes(resp_handle) {
                Some(v) => v,
                None => return,
            };
            if stream.write_all(&resp).await.is_err() {
                return;
            }
        }
        let _ = stream.flush().await;

        if !keep_alive {
            return;
        }
        // accumulated 已经被 take 走，下轮循环重新积累
        accumulated.reserve(16384);
    }
}

/// 判断 HTTP/1.1 请求是否完整：headers 找到 \r\n\r\n + body 字节数 ≥ Content-Length。
/// 没 Content-Length 头视为无 body 请求（GET/HEAD）。
fn http_request_complete(bytes: &[u8]) -> bool {
    let header_end = match bytes.windows(4).position(|w| w == b"\r\n\r\n") {
        Some(p) => p,
        None => return false,
    };
    let headers = &bytes[..header_end];

    // 找 Content-Length（不区分大小写）
    let cl_needle = b"content-length:";
    let mut idx = 0;
    while idx + cl_needle.len() <= headers.len() {
        if headers[idx..idx + cl_needle.len()].eq_ignore_ascii_case(cl_needle) {
            // 行内取值
            let line_end = headers[idx..]
                .windows(2)
                .position(|w| w == b"\r\n")
                .map(|p| idx + p)
                .unwrap_or(headers.len());
            let value_bytes = &headers[idx + cl_needle.len()..line_end];
            // 解析 i64
            let value_str = match std::str::from_utf8(value_bytes) {
                Ok(s) => s.trim(),
                Err(_) => return true, // 解析失败保守判完整
            };
            let cl: usize = match value_str.parse() {
                Ok(v) => v,
                Err(_) => return true,
            };
            let body_received = bytes.len() - header_end - 4;
            return body_received >= cl;
        }
        idx += 1;
    }
    // 没 Content-Length —— GET/HEAD 等无 body 请求，complete
    true
}

fn request_has_connection_close(bytes: &[u8]) -> bool {
    let header_end = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .unwrap_or(bytes.len());
    let headers = &bytes[..header_end];

    // 大小写无关搜 "connection:" 然后看其行内是否带 "close"
    let needle = b"connection:";
    let mut i = 0;
    while i + needle.len() <= headers.len() {
        if headers[i..i + needle.len()].eq_ignore_ascii_case(needle) {
            // 找到 connection: header，往后到行尾扫 close
            let line_end = headers[i..]
                .windows(2)
                .position(|w| w == b"\r\n")
                .map(|p| i + p)
                .unwrap_or(headers.len());
            let value = &headers[i + needle.len()..line_end];
            return value.windows(5).any(|w| w.eq_ignore_ascii_case(b"close"));
        }
        i += 1;
    }
    false
}

// ============================================================================
// 异步 TCP IO — 每个操作返回 qi::Future，配 等待 关键字使用
// ============================================================================
//
// 跟旧 std-based TCP 池并存。这里的 TcpStream 是 tokio::net::TcpStream，
// 由 tokio runtime 用 epoll/kqueue 多路复用。N 个并发 read/write 不 pin N 个
// OS 线程（取决于 await 真不真，目前还是 sync wrapper 里 block_on）。

use std::sync::atomic::AtomicI64 as StdAtomicI64;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream as TokioTcpStream;
use tokio::sync::Mutex as TokioMutex;

static TOKIO_TCP_POOL: OnceLock<DashMap<i64, std::sync::Arc<TokioMutex<TokioTcpStream>>>> =
    OnceLock::new();
static TOKIO_TCP_NEXT: StdAtomicI64 = StdAtomicI64::new(1_000_000); // 跟 std 池的句柄段错开

fn tokio_tcp_pool() -> &'static DashMap<i64, std::sync::Arc<TokioMutex<TokioTcpStream>>> {
    TOKIO_TCP_POOL.get_or_init(DashMap::new)
}

fn next_tokio_handle() -> i64 {
    TOKIO_TCP_NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 创建一个新的 pending Future + 拿到它的字段 Arc 给 task 用
fn new_pending_future() -> (
    *mut crate::async_runtime::future::Future,
    std::sync::Arc<std::sync::Mutex<crate::async_runtime::future::FutureState>>,
    std::sync::Arc<std::sync::Mutex<Option<crate::async_runtime::future::FutureValue>>>,
    std::sync::Arc<std::sync::Mutex<Option<String>>>,
    std::sync::Arc<tokio::sync::Notify>,
) {
    use crate::async_runtime::future::Future;
    let f = Box::new(Future::pending());
    let f_ptr = Box::into_raw(f);
    unsafe {
        (
            f_ptr,
            (*f_ptr).state.clone(),
            (*f_ptr).value.clone(),
            (*f_ptr).error.clone(),
            (*f_ptr).notify.clone(),
        )
    }
}

fn fail_immediately(
    f_ptr: *mut crate::async_runtime::future::Future,
    msg: String,
) -> *mut crate::async_runtime::future::Future {
    use crate::async_runtime::future::FutureState;
    unsafe {
        *(*f_ptr).error.lock().unwrap() = Some(msg);
        *(*f_ptr).state.lock().unwrap() = FutureState::Failed;
        (*f_ptr).notify.notify_waiters();
    }
    f_ptr
}

/// 异步 TCP 连接 — 返回 未来<整数>。整数是连接句柄（>0），失败 -1。
#[no_mangle]
pub extern "C" fn qi_network_async_tcp_connect(
    host: *const c_char,
    port: u16,
) -> *mut crate::async_runtime::future::Future {
    use crate::async_runtime::future::{FutureState, FutureValue};

    let (f_ptr, state, value, error, notify) = new_pending_future();

    let host_str = if host.is_null() {
        return fail_immediately(f_ptr, "null host".to_string());
    } else {
        unsafe { CStr::from_ptr(host).to_string_lossy().to_string() }
    };

    异步运行时().spawn(async move {
        let addr = format!("{}:{}", host_str, port);
        match TokioTcpStream::connect(&addr).await {
            Ok(stream) => {
                let _ = stream.set_nodelay(true);
                let handle = next_tokio_handle();
                tokio_tcp_pool().insert(handle, std::sync::Arc::new(TokioMutex::new(stream)));
                *value.lock().unwrap() = Some(FutureValue::Integer(handle));
                *state.lock().unwrap() = FutureState::Completed;
                notify.notify_waiters();
            }
            Err(e) => {
                *error.lock().unwrap() = Some(format!("connect {} failed: {}", addr, e));
                *state.lock().unwrap() = FutureState::Failed;
                notify.notify_waiters();
            }
        }
    });

    f_ptr
}

/// 异步读字节 — 返回 未来<整数>。整数是字节切片句柄（>0），EOF=0，错误<0。
#[no_mangle]
pub extern "C" fn qi_network_async_tcp_read_bytes(
    handle: i64,
    buffer_size: i64,
) -> *mut crate::async_runtime::future::Future {
    use crate::async_runtime::future::{FutureState, FutureValue};

    let (f_ptr, state, value, error, notify) = new_pending_future();

    let stream = match tokio_tcp_pool().get(&handle) {
        Some(e) => e.clone(),
        None => return fail_immediately(f_ptr, format!("invalid tcp handle {}", handle)),
    };

    let buf_size = if buffer_size <= 0 {
        4096
    } else {
        buffer_size as usize
    };

    异步运行时().spawn(async move {
        let mut buf = vec![0u8; buf_size];
        let mut s = stream.lock().await;
        match s.read(&mut buf).await {
            Ok(0) => {
                *value.lock().unwrap() = Some(FutureValue::Integer(0));
                *state.lock().unwrap() = FutureState::Completed;
                notify.notify_waiters();
            }
            Ok(n) => {
                buf.truncate(n);
                let bytes_handle = crate::stdlib::bytes_ffi::register_bytes(buf);
                *value.lock().unwrap() = Some(FutureValue::Integer(bytes_handle));
                *state.lock().unwrap() = FutureState::Completed;
                notify.notify_waiters();
            }
            Err(e) => {
                *error.lock().unwrap() = Some(format!("read failed: {}", e));
                *state.lock().unwrap() = FutureState::Failed;
                notify.notify_waiters();
            }
        }
    });

    f_ptr
}

/// 异步写字节 — 返回 未来<整数>。整数是写入字节数（≥0），错误 < 0。
#[no_mangle]
pub extern "C" fn qi_network_async_tcp_write_bytes(
    handle: i64,
    bytes_handle: i64,
) -> *mut crate::async_runtime::future::Future {
    use crate::async_runtime::future::{FutureState, FutureValue};

    let (f_ptr, state, value, error, notify) = new_pending_future();

    let stream = match tokio_tcp_pool().get(&handle) {
        Some(e) => e.clone(),
        None => return fail_immediately(f_ptr, format!("invalid tcp handle {}", handle)),
    };

    let data = match crate::stdlib::bytes_ffi::clone_bytes(bytes_handle) {
        Some(d) => d,
        None => return fail_immediately(f_ptr, format!("invalid bytes handle {}", bytes_handle)),
    };

    异步运行时().spawn(async move {
        let mut s = stream.lock().await;
        match s.write_all(&data).await {
            Ok(()) => {
                let _ = s.flush().await;
                *value.lock().unwrap() = Some(FutureValue::Integer(data.len() as i64));
                *state.lock().unwrap() = FutureState::Completed;
                notify.notify_waiters();
            }
            Err(e) => {
                *error.lock().unwrap() = Some(format!("write failed: {}", e));
                *state.lock().unwrap() = FutureState::Failed;
                notify.notify_waiters();
            }
        }
    });

    f_ptr
}

/// 关闭异步 TCP 连接，释放池中条目
#[no_mangle]
pub extern "C" fn qi_network_async_tcp_close(handle: i64) -> i64 {
    if tokio_tcp_pool().remove(&handle).is_some() {
        1
    } else {
        0
    }
}

// ============================================================================
// 异步 TCP listener
// ============================================================================

static TOKIO_LISTENER_POOL: OnceLock<DashMap<i64, std::sync::Arc<tokio::net::TcpListener>>> =
    OnceLock::new();
static TOKIO_LISTENER_NEXT: StdAtomicI64 = StdAtomicI64::new(2_000_000); // 跟其他句柄段错开

fn tokio_listener_pool() -> &'static DashMap<i64, std::sync::Arc<tokio::net::TcpListener>> {
    TOKIO_LISTENER_POOL.get_or_init(DashMap::new)
}

fn next_listener_handle() -> i64 {
    TOKIO_LISTENER_NEXT.fetch_add(1, Ordering::Relaxed)
}

/// 异步 TCP 监听 — 返回 未来<整数 server_handle>
#[no_mangle]
pub extern "C" fn qi_network_async_tcp_listen(
    host: *const c_char,
    port: u16,
) -> *mut crate::async_runtime::future::Future {
    use crate::async_runtime::future::{FutureState, FutureValue};
    let (f_ptr, state, value, error, notify) = new_pending_future();

    let host_str = if host.is_null() {
        return fail_immediately(f_ptr, "null host".to_string());
    } else {
        unsafe { CStr::from_ptr(host).to_string_lossy().to_string() }
    };

    异步运行时().spawn(async move {
        let addr = format!("{}:{}", host_str, port);
        match tokio::net::TcpListener::bind(&addr).await {
            Ok(listener) => {
                // 登记 raw socket 到信号关停名单（跟 std listener 一样的 SIGINT 处理）
                let raw_fd = 原始套接字(&listener);
                crate::stdlib::signal_ffi::qi_signal_register_listener_fd(raw_fd);
                let handle = next_listener_handle();
                tokio_listener_pool().insert(handle, std::sync::Arc::new(listener));
                *value.lock().unwrap() = Some(FutureValue::Integer(handle));
                *state.lock().unwrap() = FutureState::Completed;
                notify.notify_waiters();
            }
            Err(e) => {
                *error.lock().unwrap() = Some(format!("listen {} failed: {}", addr, e));
                *state.lock().unwrap() = FutureState::Failed;
                notify.notify_waiters();
            }
        }
    });

    f_ptr
}

/// 异步 TCP 接受连接 — 返回 未来<整数 client_handle>
#[no_mangle]
pub extern "C" fn qi_network_async_tcp_accept(
    server_handle: i64,
) -> *mut crate::async_runtime::future::Future {
    use crate::async_runtime::future::{FutureState, FutureValue};
    let (f_ptr, state, value, error, notify) = new_pending_future();

    let listener = match tokio_listener_pool().get(&server_handle) {
        Some(e) => e.clone(),
        None => {
            return fail_immediately(f_ptr, format!("invalid listener {}", server_handle));
        }
    };

    异步运行时().spawn(async move {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let _ = stream.set_nodelay(true);
                let handle = next_tokio_handle();
                tokio_tcp_pool().insert(handle, std::sync::Arc::new(TokioMutex::new(stream)));
                *value.lock().unwrap() = Some(FutureValue::Integer(handle));
                *state.lock().unwrap() = FutureState::Completed;
                notify.notify_waiters();
            }
            Err(e) => {
                // accept 错误（比如 listener 被信号 shutdown）-- 返回 -1 表示 EOF
                *value.lock().unwrap() = Some(FutureValue::Integer(-1));
                *error.lock().unwrap() = Some(format!("accept failed: {}", e));
                *state.lock().unwrap() = FutureState::Completed;
                notify.notify_waiters();
            }
        }
    });

    f_ptr
}

/// 关闭异步 TCP listener
#[no_mangle]
pub extern "C" fn qi_network_async_tcp_listener_close(server_handle: i64) -> i64 {
    match tokio_listener_pool().remove(&server_handle) {
        Some((_, listener)) => {
            crate::stdlib::signal_ffi::qi_signal_unregister_listener_fd(原始套接字(
                &*listener,
            ));
            1
        }
        None => 0,
    }
}
