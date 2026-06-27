//! 同步原语 FFI — 互斥锁 + 原子整数
//!
//! 设计：句柄基础（handle-based），全局注册表，和 subprocess_ffi.rs 同一惯例。
//!
//! **互斥锁**
//! 要在两次 FFI 调用之间保持 MutexGuard，需要 `'static` lifetime 的 guard。
//! 方案：`Box::leak` 把 `Mutex<()>` 提升为 `&'static Mutex<()>`，这样 guard 的
//! lifetime 也是 `'static`，可以存入 `LOCK_GUARDS` 注册表。
//! 加锁时把 guard 塞进注册表；解锁时把 guard 从注册表取出并 drop。
//!
//! **原子整数**
//! `AtomicI64` 没有 guard 问题，直接 `Arc<AtomicI64>` 即可。

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

// ─── Send wrapper for MutexGuard<'static, ()> ────────────────────────────────
//
// MutexGuard 本身不是 Send，但我们保证：
//   1. guard 在同一线程创建并存入注册表
//   2. unlock 也是通过同一注册表访问
//   3. 整个 GuardRegistry 本身用 Mutex 保护（单一临界区访问）
//
// 从 Rust 的形式安全性角度：MutexGuard<!Send> 放进 Mutex<HashMap<_, Guard>> 意味着
// Mutex<HashMap> 也无法 Send/Sync，而 OnceLock<Mutex<...>> 要求内容 Sync。
// 我们使用 raw pointer 包装绕过此限制，并自行保证正确性（唯一所有权，Drop 在注册表移除时发生）。

struct SendableGuard(*mut ());
// SAFETY: guard 实际上是 Box<MutexGuard<'static, ()>> 的指针；我们保证
// 同一时刻只有一个线程通过 lock/unlock 访问，且 GuardRegistry 的 Mutex 确保互斥。
unsafe impl Send for SendableGuard {}
unsafe impl Sync for SendableGuard {}

impl SendableGuard {
    /// 从 `Box<MutexGuard<'static, ()>>` 创建（消耗 Box，拿走所有权）
    fn new(guard: MutexGuard<'static, ()>) -> Self {
        // 把 guard 放进 Box，然后将 Box 的原始指针存起来
        let boxed = Box::new(guard);
        SendableGuard(Box::into_raw(boxed) as *mut ())
    }

    /// 解锁：消耗 self，drop Box（进而 drop guard，释放锁）。
    /// 调用后 self 被 consumed，不会再触发 Drop。
    unsafe fn unlock(mut self) {
        let ptr = self.0 as *mut MutexGuard<'static, ()>;
        self.0 = std::ptr::null_mut(); // 防止 Drop 再次释放
        drop(Box::from_raw(ptr));
    }
}

impl Drop for SendableGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // 异常路径（destroy 未 unlock 就销毁）：安全释放。
            unsafe {
                let ptr = self.0 as *mut MutexGuard<'static, ()>;
                drop(Box::from_raw(ptr));
            }
            self.0 = std::ptr::null_mut();
        }
    }
}

// ─── 内部存储类型 ─────────────────────────────────────────────────────────────

struct MutexEntry {
    /// 实际的 Mutex（`'static` 引用，通过 Box::leak 获得）
    m: &'static Mutex<()>,
}

type MutexRegistry = Mutex<HashMap<i64, MutexEntry>>;
type GuardRegistry = Mutex<HashMap<i64, SendableGuard>>;
type AtomicRegistry = Mutex<HashMap<i64, Arc<AtomicI64>>>;

fn mutex_registry() -> &'static MutexRegistry {
    static REG: OnceLock<MutexRegistry> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn guard_registry() -> &'static GuardRegistry {
    static REG: OnceLock<GuardRegistry> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

fn atomic_registry() -> &'static AtomicRegistry {
    static REG: OnceLock<AtomicRegistry> = OnceLock::new();
    REG.get_or_init(|| Mutex::new(HashMap::new()))
}

static MUTEX_COUNTER: AtomicI64 = AtomicI64::new(1);
static ATOMIC_COUNTER: AtomicI64 = AtomicI64::new(1);

// ─── 互斥锁 API ───────────────────────────────────────────────────────────────

/// 创建一把新互斥锁，返回句柄（>0）；失败返回 -1。
#[no_mangle]
pub extern "C" fn qi_sync_mutex_create() -> i64 {
    // Box::leak → 'static 引用，guard 可安全存储
    let m: &'static Mutex<()> = Box::leak(Box::new(Mutex::new(())));
    let handle = MUTEX_COUNTER.fetch_add(1, Ordering::SeqCst);
    if let Ok(mut reg) = mutex_registry().lock() {
        reg.insert(handle, MutexEntry { m });
        handle
    } else {
        -1
    }
}

/// 阻塞直到获得锁。成功 1，失败 -1。
#[no_mangle]
pub extern "C" fn qi_sync_mutex_lock(handle: i64) -> i32 {
    // 先拿到 &'static Mutex<()>
    let m = {
        let reg = match mutex_registry().lock() {
            Ok(g) => g,
            Err(_) => return -1,
        };
        match reg.get(&handle) {
            Some(e) => e.m,
            None => return -1,
        }
    };

    // 阻塞加锁
    let guard = match m.lock() {
        Ok(g) => g,
        Err(_) => return -1, // 中毒锁
    };

    // 存储 guard（保持锁）
    match guard_registry().lock() {
        Ok(mut gr) => {
            gr.insert(handle, SendableGuard::new(guard));
            1
        }
        Err(_) => -1,
    }
}

/// 释放之前通过 qi_sync_mutex_lock 获得的锁。成功 1，失败 -1。
#[no_mangle]
pub extern "C" fn qi_sync_mutex_unlock(handle: i64) -> i32 {
    match guard_registry().lock() {
        Ok(mut gr) => {
            if let Some(sg) = gr.remove(&handle) {
                // SAFETY: 我们是唯一持有这个 guard 的，现在 drop 它
                unsafe {
                    sg.unlock();
                }
                1
            } else {
                -1 // 没有对应 guard（未加锁？）
            }
        }
        Err(_) => -1,
    }
}

/// 尝试加锁（非阻塞）。1=成功获得锁，0=锁被占用，-1=错误。
#[no_mangle]
pub extern "C" fn qi_sync_mutex_trylock(handle: i64) -> i32 {
    let m = {
        let reg = match mutex_registry().lock() {
            Ok(g) => g,
            Err(_) => return -1,
        };
        match reg.get(&handle) {
            Some(e) => e.m,
            None => return -1,
        }
    };

    match m.try_lock() {
        Ok(guard) => match guard_registry().lock() {
            Ok(mut gr) => {
                gr.insert(handle, SendableGuard::new(guard));
                1
            }
            Err(_) => -1,
        },
        Err(std::sync::TryLockError::WouldBlock) => 0,
        Err(_) => -1,
    }
}

/// 销毁互斥锁（同时释放可能持有的 guard）。成功 1，失败 -1。
#[no_mangle]
pub extern "C" fn qi_sync_mutex_destroy(handle: i64) -> i32 {
    // 先释放 guard（如果有）— SendableGuard::drop 会自动 unlock
    let _ = guard_registry().lock().map(|mut gr| gr.remove(&handle));
    // 从注册表移除（注意：Box::leak 的内存不回收——生命周期与进程同）
    match mutex_registry().lock() {
        Ok(mut reg) => {
            if reg.remove(&handle).is_some() {
                1
            } else {
                -1
            }
        }
        Err(_) => -1,
    }
}

// ─── 原子整数 API ─────────────────────────────────────────────────────────────

/// 创建原子整数，初始值为 `initial`，返回句柄（>0）；失败返回 -1。
#[no_mangle]
pub extern "C" fn qi_sync_atomic_create(initial: i64) -> i64 {
    let atom = Arc::new(AtomicI64::new(initial));
    let handle = ATOMIC_COUNTER.fetch_add(1, Ordering::SeqCst);
    match atomic_registry().lock() {
        Ok(mut reg) => {
            reg.insert(handle, atom);
            handle
        }
        Err(_) => -1,
    }
}

/// 原子读取当前值。句柄无效时返回 i64::MIN。
#[no_mangle]
pub extern "C" fn qi_sync_atomic_load(handle: i64) -> i64 {
    match atomic_registry().lock() {
        Ok(reg) => match reg.get(&handle) {
            Some(a) => a.load(Ordering::SeqCst),
            None => i64::MIN,
        },
        Err(_) => i64::MIN,
    }
}

/// 原子写入值。成功 1，失败 -1。
#[no_mangle]
pub extern "C" fn qi_sync_atomic_store(handle: i64, val: i64) -> i32 {
    match atomic_registry().lock() {
        Ok(reg) => match reg.get(&handle) {
            Some(a) => {
                a.store(val, Ordering::SeqCst);
                1
            }
            None => -1,
        },
        Err(_) => -1,
    }
}

/// 原子加法（fetch_add），返回加法后的新值。句柄无效时返回 i64::MIN。
#[no_mangle]
pub extern "C" fn qi_sync_atomic_add(handle: i64, delta: i64) -> i64 {
    match atomic_registry().lock() {
        Ok(reg) => match reg.get(&handle) {
            Some(a) => a.fetch_add(delta, Ordering::SeqCst) + delta,
            None => i64::MIN,
        },
        Err(_) => i64::MIN,
    }
}

/// 原子比较交换（CAS）。expected==当前值时写入 new，返回 1；否则返回 0；出错 -1。
#[no_mangle]
pub extern "C" fn qi_sync_atomic_cas(handle: i64, expected: i64, new: i64) -> i32 {
    match atomic_registry().lock() {
        Ok(reg) => match reg.get(&handle) {
            Some(a) => {
                match a.compare_exchange(expected, new, Ordering::SeqCst, Ordering::SeqCst) {
                    Ok(_) => 1,
                    Err(_) => 0,
                }
            }
            None => -1,
        },
        Err(_) => -1,
    }
}

/// 销毁原子整数，释放 Arc。成功 1，失败 -1。
#[no_mangle]
pub extern "C" fn qi_sync_atomic_destroy(handle: i64) -> i32 {
    match atomic_registry().lock() {
        Ok(mut reg) => {
            if reg.remove(&handle).is_some() {
                1
            } else {
                -1
            }
        }
        Err(_) => -1,
    }
}

// ─── 单元测试 ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mutex_lock_unlock_roundtrip() {
        let h = qi_sync_mutex_create();
        assert!(h > 0);
        assert_eq!(qi_sync_mutex_lock(h), 1);
        assert_eq!(qi_sync_mutex_unlock(h), 1);
        // 解锁后可以再次加锁
        assert_eq!(qi_sync_mutex_lock(h), 1);
        assert_eq!(qi_sync_mutex_unlock(h), 1);
        assert_eq!(qi_sync_mutex_destroy(h), 1);
    }

    #[test]
    fn test_mutex_trylock() {
        let h = qi_sync_mutex_create();
        assert!(h > 0);
        // 未加锁时尝试加锁应成功
        assert_eq!(qi_sync_mutex_trylock(h), 1);
        // 已加锁时 trylock 应返回 0（WouldBlock）
        // 注意：guard 存在于注册表，再次 trylock 同一 handle 实际上是同一线程，
        // std::sync::Mutex 是非递归锁，再次 try_lock 会 WouldBlock。
        assert_eq!(qi_sync_mutex_trylock(h), 0);
        assert_eq!(qi_sync_mutex_unlock(h), 1);
        assert_eq!(qi_sync_mutex_destroy(h), 1);
    }

    #[test]
    fn test_atomic_add_roundtrip() {
        let h = qi_sync_atomic_create(0);
        assert!(h > 0);
        assert_eq!(qi_sync_atomic_load(h), 0);

        let after = qi_sync_atomic_add(h, 5);
        assert_eq!(after, 5);
        assert_eq!(qi_sync_atomic_load(h), 5);

        let after2 = qi_sync_atomic_add(h, 3);
        assert_eq!(after2, 8);

        assert_eq!(qi_sync_atomic_store(h, 100), 1);
        assert_eq!(qi_sync_atomic_load(h), 100);

        // CAS：期望 100，交换为 200
        assert_eq!(qi_sync_atomic_cas(h, 100, 200), 1);
        assert_eq!(qi_sync_atomic_load(h), 200);

        // CAS：期望 100（已不是 100），应返回 0
        assert_eq!(qi_sync_atomic_cas(h, 100, 999), 0);
        assert_eq!(qi_sync_atomic_load(h), 200); // 未变

        assert_eq!(qi_sync_atomic_destroy(h), 1);
    }

    #[test]
    fn test_atomic_concurrent_add() {
        use std::thread;
        let h = qi_sync_atomic_create(0);
        assert!(h > 0);

        let n_threads = 8usize;
        let n_iters = 1000usize;

        let handles: Vec<_> = (0..n_threads)
            .map(|_| {
                thread::spawn(move || {
                    for _ in 0..n_iters {
                        qi_sync_atomic_add(h, 1);
                    }
                })
            })
            .collect();

        for th in handles {
            th.join().unwrap();
        }

        let total = qi_sync_atomic_load(h);
        assert_eq!(total, (n_threads * n_iters) as i64, "原子计数应无竞争丢失");
        assert_eq!(qi_sync_atomic_destroy(h), 1);
    }
}
