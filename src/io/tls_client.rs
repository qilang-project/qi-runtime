//! 客户端 TLS 流（给 wss:// 和将来的数据库/其它 TCP 客户端用）
//!
//! ── 为什么不能直接用 rustls::StreamOwned ──────────────────────────
//!
//! WebSocket 连接的并发模型是「一个协程阻塞在 recv，别的协程随时能 send」——
//! qi-web 的广播全靠它（`实时广播.qi`：任意 goroutine 向正在 recv 的连接推帧）。
//! 明文路径能做到，是因为 `&TcpStream` 自己实现了 Read/Write，收发各借一个
//! 不可变引用，谁也不挡谁。
//!
//! TLS 不行：它是个状态机，收发都要 `&mut ClientConnection`。最直觉的写法
//! ——「整个流套一个 Mutex」——会让阻塞的 recv 一直攥着锁，send 全部卡死。
//! 这正是运行时 16-1 版 WS 池的那个老 bug（recv 抱池锁，第二条连接永久卡住）。
//!
//! ── 这里的纪律：阻塞的 socket 读，绝不持有 conn 锁 ────────────────
//!
//!   收：先不加锁地阻塞读 socket 原始字节 → 再上锁把密文喂给状态机解出明文
//!   发：上锁写明文进状态机、把密文吐到 socket（短暂持锁，不阻塞在读上）
//!
//! 锁只在「喂字节/取字节」这几微秒内被持有，阻塞等待发生在锁外，
//! 于是 recv 阻塞期间 send 照常。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use rustls::pki_types::ServerName;
use rustls::{ClientConfig, ClientConnection, RootCertStore};

/// 进程级共享的客户端配置（根证书解析一次就够，每连接重建是纯浪费）
static 客户配置: OnceLock<Arc<ClientConfig>> = OnceLock::new();

fn 装默认加密提供者() {
    static 装过: OnceLock<()> = OnceLock::new();
    装过.get_or_init(|| {
        // 装不上多半是别处已经装过（reqwest 也用 rustls），忽略即可
        let _ = rustls::crypto::ring::default_provider().install_default();
    });
}

fn 取客户配置() -> Arc<ClientConfig> {
    客户配置
        .get_or_init(|| {
            装默认加密提供者();
            // 用编进二进制的 Mozilla 根证书，不读系统钥匙串：
            // 交叉编译出来的 Linux 二进制要能在任何机器上直接跑，
            // 依赖系统证书库会在最小化容器里悄悄失败。
            let mut 根 = RootCertStore::empty();
            根.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            add_extra_roots(&mut 根);
            Arc::new(
                ClientConfig::builder()
                    .with_root_certificates(根)
                    .with_no_client_auth(),
            )
        })
        .clone()
}

/// 额外信任的根证书（PEM）：`QI_TLS_CA_FILE`，没设就看 `SSL_CERT_FILE`。
///
/// 内置的 Mozilla 根证书认不了内网 CA 和自签证书 —— 公司内网的 wss 网关、
/// 本地起的 TLS 测试服务都连不上，而且报错只是「握手失败」。设了这个变量就
/// 在内置根之外**追加**这些证书（不替换：公网端点照样能连）。读不出来或
/// 解析失败只打一行警告，不让整个进程连公网也连不了。
fn add_extra_roots(roots: &mut RootCertStore) {
    let path = std::env::var_os("QI_TLS_CA_FILE").or_else(|| std::env::var_os("SSL_CERT_FILE"));
    let Some(path) = path else {
        return;
    };
    let file = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[qi TLS] 读不了额外根证书 {}: {e}", path.to_string_lossy());
            return;
        }
    };
    let mut reader = std::io::BufReader::new(file);
    let mut added = 0usize;
    for cert in rustls_pemfile::certs(&mut reader) {
        match cert {
            Ok(c) => {
                if roots.add(c).is_ok() {
                    added += 1;
                }
            }
            Err(e) => {
                eprintln!("[qi TLS] 解析 {} 失败: {e}", path.to_string_lossy());
                return;
            }
        }
    }
    if added == 0 {
        eprintln!("[qi TLS] {} 里没有可用的证书", path.to_string_lossy());
    }
}

pub struct TLS客户流 {
    socket: TcpStream,
    conn: Mutex<ClientConnection>,
}

impl TLS客户流 {
    /// 在已连上的 TCP 上完成 TLS 握手。域名用于 SNI 与证书校验。
    pub fn 握手(socket: TcpStream, 域名: &str) -> Result<Self, std::io::Error> {
        Self::handshake_with(socket, 域名, 取客户配置())
    }

    fn handshake_with(
        mut socket: TcpStream,
        域名: &str,
        config: Arc<ClientConfig>,
    ) -> Result<Self, std::io::Error> {
        let 名 = ServerName::try_from(域名.to_string())
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "域名无效"))?;
        let mut conn = ClientConnection::new(config, 名)
            .map_err(|e| std::io::Error::other(format!("TLS 初始化失败: {e}")))?;

        // 握手期是纯同步的、还没有别的协程碰这条连接，可以放心用 complete_io
        conn.complete_io(&mut socket)
            .map_err(|e| std::io::Error::other(format!("TLS 握手失败: {e}")))?;

        Ok(Self {
            socket,
            conn: Mutex::new(conn),
        })
    }

    /// 阻塞读一批密文并解密，把明文追加进 出。返回追加了多少字节。
    ///
    /// 超时 只作用在 socket 读上。
    ///
    /// **返回 Ok(0) 不代表「没数据」** —— 一条 TLS 记录可能跨多次 socket 读，
    /// 只收到半条时解不出任何明文。调用方必须继续泵（在自己的截止时间内），
    /// 把 Ok(0) 当成「超时/无数据」会让大帧永远读不出来：小帧一次读完记录
    /// 所以看着正常，几十 KB 的帧必然中途「读断」。
    pub fn 泵(
        &self, 出: &mut Vec<u8>, 超时: Option<Duration>
    ) -> Result<usize, std::io::Error> {
        let mut 密文 = [0u8; 16384];
        // ★ 阻塞发生在这里，**没有持有 conn 锁**
        self.socket.set_read_timeout(超时)?;
        let n = (&self.socket).read(&mut 密文);
        if 超时.is_some() {
            let _ = self.socket.set_read_timeout(None);
        }
        let n = match n {
            Ok(0) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "对端已关闭",
                ))
            }
            Ok(n) => n,
            Err(e) => return Err(e),
        };

        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        let 原长 = 出.len();

        // ⚠ read_tls **不保证吃完整个游标**：它有内部缓冲上限，一次只收下一部分，
        // 返回值才是真正收下的字节数。喂一次就把 密文 丢掉的话，剩下的字节永远
        // 消失、TLS 流从此错位 —— 小帧看不出来（一个 read 正好装得下），
        // 大帧必炸。实测症状是 wss 上收得到小的文本帧、一遇到几十 KB 的音频帧
        // 就「读断」，而同一个端点用别的客户端一切正常。
        //
        // 所以要循环喂，并且**每喂一轮就把明文排空** —— 不排空的话 rustls 的
        // 缓冲一直是满的，read_tls 会一直收下 0 字节，死循环。
        let mut 已喂 = 0usize;
        while 已喂 < n {
            let mut 游标 = std::io::Cursor::new(&密文[已喂..n]);
            let 本轮 = conn.read_tls(&mut 游标)?;
            if 本轮 == 0 {
                // 缓冲满且排不空（对端还没发完一条完整记录）—— 留着下次再喂，
                // 但这批剩下的字节已经读出 socket 了，只能先解出来再说
                break;
            }
            已喂 += 本轮;
            conn.process_new_packets()
                .map_err(|e| std::io::Error::other(format!("TLS 解密失败: {e}")))?;
            // read_to_end 在没有明文时返回 WouldBlock（数据已在内存里，不阻塞），
            // 错误无所谓：已读到的字节 read_to_end 会保留在 出 里
            let _ = conn.reader().read_to_end(出);
        }

        // 解包可能让状态机想回话（告警、密钥更新），顺手吐出去
        while conn.wants_write() {
            conn.write_tls(&mut &self.socket)?;
        }
        Ok(出.len() - 原长)
    }

    /// 加密并发送。持锁很短，且不阻塞在读上。
    pub fn 发送(&self, 数据: &[u8]) -> Result<(), std::io::Error> {
        let mut conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());
        // ⚠ 不能一把 write_all：rustls 的明文发送缓冲有上限（默认 64 KiB），
        // 满了 write 就收下 0 字节，write_all 随即报 WriteZero —— 小帧没事，
        // 一过 64 KiB 的帧（大段文本、音频）就整帧发不出去，连接被判坏。
        // 所以每塞一批就把密文吐到 socket，腾出缓冲再塞下一批。
        let mut sent = 0usize;
        while sent < 数据.len() {
            let n = conn.writer().write(&数据[sent..])?;
            sent += n;
            let mut flushed = false;
            while conn.wants_write() {
                conn.write_tls(&mut &self.socket)?;
                flushed = true;
            }
            if n == 0 && !flushed {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "TLS 发送缓冲满且无法排空",
                ));
            }
        }
        Ok(())
    }

    /// 关闭：先发 close_notify 让对端知道是正常收尾，再断 TCP。
    pub fn 关闭(&self) {
        if let Ok(mut conn) = self.conn.lock() {
            conn.send_close_notify();
            while conn.wants_write() {
                if conn.write_tls(&mut &self.socket).is_err() {
                    break;
                }
            }
        }
        let _ = self.socket.shutdown(std::net::Shutdown::Both);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    const CA: &str = include_str!("../../tests/fixtures/tls/ca.pem");
    const LEAF: &str = include_str!("../../tests/fixtures/tls/leaf.pem");
    const LEAF_KEY: &str = include_str!("../../tests/fixtures/tls/leaf.pk8");

    /// 超过 rustls 64 KiB 明文发送缓冲的一次发送：以前 write_all 撞上 WriteZero，
    /// wss 上大帧整帧发不出去（小帧一直正常，所以只有大文本/音频才炸）。
    #[test]
    fn send_larger_than_plaintext_buffer() {
        装默认加密提供者();
        let certs: Vec<_> = rustls_pemfile::certs(&mut LEAF.as_bytes())
            .collect::<Result<_, _>>()
            .unwrap();
        let key = rustls_pemfile::private_key(&mut LEAF_KEY.as_bytes())
            .unwrap()
            .unwrap();
        let server_config = Arc::new(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_single_cert(certs, key)
                .unwrap(),
        );
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        const N: usize = 300 * 1024;

        // 服务端：收满 N 字节原样回送
        let server = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let conn = rustls::ServerConnection::new(server_config).unwrap();
            let mut tls = rustls::StreamOwned::new(conn, sock);
            let mut buf = vec![0u8; N];
            tls.read_exact(&mut buf).unwrap();
            tls.write_all(&buf).unwrap();
            tls.flush().unwrap();
        });

        let mut roots = RootCertStore::empty();
        for c in rustls_pemfile::certs(&mut CA.as_bytes()) {
            roots.add(c.unwrap()).unwrap();
        }
        let config = Arc::new(
            ClientConfig::builder()
                .with_root_certificates(roots)
                .with_no_client_auth(),
        );
        let sock = TcpStream::connect(addr).unwrap();
        let client = TLS客户流::handshake_with(sock, "localhost", config).unwrap();
        let data: Vec<u8> = (0..N).map(|i| (i % 251) as u8).collect();
        client.发送(&data).expect("大于 64 KiB 的一次发送不该失败");

        let mut got = Vec::new();
        while got.len() < N {
            client
                .泵(&mut got, Some(Duration::from_secs(10)))
                .expect("回读失败");
        }
        assert_eq!(got, data);
        server.join().unwrap();
    }
}
