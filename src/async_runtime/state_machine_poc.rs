//! State machine PoC for compiler async/await milestone.
//!
//! 模拟 codegen 将来要生成的 LLVM IR 形态 —— 用纯 Rust 把状态机
//! 手动写一遍，证明设计可行（docs/编译器异步状态机里程碑.md §9）。
//!
//! 跑通的要点：
//! 1. 异步函数入口立即返回 pending Future（不阻塞调用方）
//! 2. `等待 F` 在 F pending 时不阻塞 worker，而是注册 waker 后 return
//! 3. F complete 时回调 poll，状态机继续执行
//! 4. 最终 return 通过 complete return_future 把值传出去
//!
//! 模拟的源码：
//! ```qi
//! 异步 函数 加一异步(): 未来<整数> {
//!     变量 a: 整数 = 等待 异步获取40();
//!     返回 a + 2;       // 期望 42
//! }
//! ```
//! 其中 `异步获取40()` 是一个延迟 5ms 后 complete(40) 的 Future。

#![allow(dead_code)]

use super::future::{Future, FutureValue, StateMachineWaker};
use std::sync::{Arc, Mutex};

// ============================================================================
// 模拟 codegen 生成的 frame struct + poll fn
// ============================================================================

/// 模拟 异步 函数 加一异步() 的 frame
#[repr(C)]
struct Frame加一异步 {
    state: i64,
    /// 当前正在 await 的 Future（poll 时取它的值）
    awaited_future: *mut Future,
    /// 调用方持有的返回 Future（最终值塞这里）
    return_future: *mut Future,
    /// 局部变量
    local_a: i64,
}

/// 模拟 异步 函数 入口 —— 编译器会生成等价代码
fn 加一异步() -> *mut Future {
    // alloc frame
    let frame = Box::new(Frame加一异步 {
        state: 0,
        awaited_future: std::ptr::null_mut(),
        return_future: std::ptr::null_mut(),
        local_a: 0,
    });
    let frame_ptr = Box::into_raw(frame) as *mut u8;

    // alloc return future
    let ret_fut = Box::new(Future::pending());
    let ret_fut_ptr = Box::into_raw(ret_fut);
    unsafe {
        (*(frame_ptr as *mut Frame加一异步)).return_future = ret_fut_ptr;
    }

    // 第一次 poll —— 在调用方线程里直接跑（编译器 codegen 会 spawn）
    poll_加一异步(frame_ptr);

    ret_fut_ptr
}

/// 模拟 codegen 生成的 _poll fn
extern "C" fn poll_加一异步(frame_ptr: *mut u8) {
    let frame = unsafe { &mut *(frame_ptr as *mut Frame加一异步) };

    loop {
        match frame.state {
            // ───────── state 0：initial ─────────
            0 => {
                // 等待 异步获取40() — 调子函数取 Future
                let fut = 异步获取40();
                frame.awaited_future = fut;

                // 检查是否已就绪（fast path 跳过 waker 注册）
                let is_ready = unsafe { (*fut).is_completed() };
                if is_ready {
                    // 立即 resume —— 走到 state 1
                    frame.state = 1;
                    continue;
                }

                // pending —— transition 到 state 1，注册 waker
                frame.state = 1;
                let waker = StateMachineWaker {
                    poll_fn: poll_加一异步,
                    frame: frame_ptr as usize,
                };
                unsafe {
                    (*fut).register_sm_waker(waker);
                }
                return; // 让 caller (tokio) 跑别的 task
            }
            // ───────── state 1：after await ─────────
            1 => {
                // 取出 awaited future 的值
                let fut = frame.awaited_future;
                let value = unsafe {
                    let v = (*fut).value.lock().unwrap().clone();
                    match v {
                        Some(FutureValue::Integer(i)) => i,
                        _ => 0,
                    }
                };
                frame.local_a = value;

                // 释放 awaited future
                unsafe {
                    drop(Box::from_raw(fut));
                }
                frame.awaited_future = std::ptr::null_mut();

                // 计算 a + 2
                let result = frame.local_a + 2;

                // complete return_future
                let ret_fut = frame.return_future;
                unsafe {
                    (*ret_fut).complete(FutureValue::Integer(result));
                }

                // 标记 done，free frame
                frame.state = -1;
                unsafe {
                    drop(Box::from_raw(frame as *mut Frame加一异步));
                }
                return;
            }
            // ───────── 已完成 ─────────
            _ => return,
        }
    }
}

/// 模拟 codegen 生成的 异步获取40() —— 返回延迟 5ms 后 complete(40) 的 Future
fn 异步获取40() -> *mut Future {
    let fut = Box::new(Future::pending());
    let fut_ptr = Box::into_raw(fut);
    // 拷一份 Arc 到后台 task
    let fut_arc = Arc::new(Mutex::new(fut_ptr as usize));

    let fut_addr = fut_ptr as usize;
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        unsafe {
            let f = &*(fut_addr as *const Future);
            f.complete(FutureValue::Integer(40));
        }
        let _ = fut_arc;
    });

    fut_ptr
}

// ============================================================================
// 测试入口
// ============================================================================

/// PoC 入口：跑 加一异步() 并 await 结果，验证 = 42
#[no_mangle]
pub extern "C" fn qi_async_state_machine_poc_run() -> i64 {
    // 必须在 tokio context 里跑（调用方需 ensure_runtime_initialized）
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();

    rt.block_on(async {
        let ret_fut = 加一异步();

        // 等 ret_fut 完成 —— 用 sync await 模拟 caller 调用
        // 注：现实中 caller 也会是状态机 / tokio task，这里用 sync 是 PoC 简化
        loop {
            let is_done = unsafe { (*ret_fut).is_completed() };
            if is_done {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        let value = unsafe {
            let v = (*ret_fut).value.lock().unwrap().clone();
            match v {
                Some(FutureValue::Integer(i)) => i,
                _ => -1,
            }
        };

        unsafe {
            drop(Box::from_raw(ret_fut));
        }

        value
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn poc_state_machine_returns_42() {
        let result = qi_async_state_machine_poc_run();
        assert_eq!(result, 42, "异步函数加一异步() 状态机应返回 42 (40 + 2)");
    }

    /// 验证 await 真的非阻塞 —— 主流程做别的事，等 sleep 完成后回来
    #[test]
    fn poc_state_machine_yields_during_await() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();

        let result = rt.block_on(async {
            let start = std::time::Instant::now();
            let ret_fut = 加一异步();

            // 在状态机暂停期间，这条 task 应该能跑
            let mut counter = 0i32;
            loop {
                let is_done = unsafe { (*ret_fut).is_completed() };
                if is_done {
                    break;
                }
                counter += 1;
                tokio::task::yield_now().await;
            }

            let elapsed = start.elapsed();
            let value = unsafe {
                let v = (*ret_fut).value.lock().unwrap().clone();
                match v {
                    Some(FutureValue::Integer(i)) => i,
                    _ => -1,
                }
            };
            unsafe {
                drop(Box::from_raw(ret_fut));
            }

            // counter > 0 表示主 task 不是被 block 的；elapsed >= 5ms 是 sleep 实际等了
            (value, counter, elapsed)
        });

        assert_eq!(result.0, 42);
        assert!(
            result.1 > 0,
            "主 task 在等 await 时应该有机会运行（实际 yield_now 次数 = {}）",
            result.1
        );
        assert!(
            result.2.as_millis() >= 4,
            "实际等待时间应该 >= 5ms (sleep 时长)，实测 {}ms",
            result.2.as_millis()
        );
    }
}
