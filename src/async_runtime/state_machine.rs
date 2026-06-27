//! 编译器异步状态机 runtime helpers
//!
//! 实现 docs/编译器异步状态机里程碑.md §3：codegen 把 异步函数 编译成
//! frame struct + entry fn + poll fn 后，由这些 FFI 助手做 alloc / 调度 /
//! waker 注册 / Future 完成等粘合工作。
//!
//! ## 设计要点
//!
//! - **Frame** 是 codegen 决定布局的不透明 byte buffer，runtime 只管 alloc/free
//! - **poll_fn** 是 codegen 生成的 `extern "C" fn(*mut u8)`，对每个异步函数
//!   都不同；runtime 通过函数指针调度
//! - **spawn_poll**：把首次 poll 投到 tokio runtime 上跑（不阻塞调用方）
//! - **register_waker**：Future pending 时给它挂个 waker，complete 时回调 poll
//!
//! ## 跟现有 Future 对接
//!
//! `Future::register_sm_waker` 已实现（见 future.rs）：pending 时 push waker
//! 到 sm_wakers 列表，complete/fail 时全部触发。本文件只负责 wrap FFI。

#![allow(non_snake_case)]

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::sync::atomic::{AtomicI64, Ordering};

use super::future::{Future, FutureState, FutureValue, StateMachineWaker};

// ============================================================================
// Frame 分配 / 释放
// ============================================================================

/// 分配一个 size 字节的 zero-init 堆 frame。
/// codegen 在异步函数入口调一次，frame 跨 await 持续存在。
/// 调用方负责最终调 qi_async_free_frame 释放（通常在 poll fn 走到终态时）。
#[no_mangle]
pub extern "C" fn qi_async_alloc_frame(size: i64) -> *mut u8 {
    if size <= 0 {
        return std::ptr::null_mut();
    }
    let layout =
        std::alloc::Layout::from_size_align(size as usize, 8).expect("invalid frame layout");
    unsafe {
        let ptr = std::alloc::alloc_zeroed(layout);
        if ptr.is_null() {
            std::alloc::handle_alloc_error(layout);
        }
        ptr
    }
}

/// 释放 frame。size 必须跟 alloc 时传的一致 — codegen 会把 size 编进 poll fn
/// 的终态分支里。如果 size==0 或 ptr==null，no-op。
#[no_mangle]
pub extern "C" fn qi_async_free_frame(ptr: *mut u8, size: i64) {
    if ptr.is_null() || size <= 0 {
        return;
    }
    let layout =
        std::alloc::Layout::from_size_align(size as usize, 8).expect("invalid frame layout");
    unsafe {
        std::alloc::dealloc(ptr, layout);
    }
}

// ============================================================================
// 首次 poll 调度
// ============================================================================

/// 在 tokio runtime 上 spawn 一个 task，立刻调用 poll_fn(frame)。
/// 用于异步函数 entry：alloc frame + spawn → 立即返回 pending Future 给调用方。
///
/// poll_fn 是 codegen 生成的 fn(*mut u8) — 不同异步函数有不同的 poll_fn。
/// 通过 usize 跨 Send 边界（裸函数指针 / 裸 frame ptr 都不是 Send，但 usize 是）。
#[no_mangle]
pub extern "C" fn qi_async_spawn_poll(poll_fn: extern "C" fn(*mut u8), frame: *mut u8) {
    let poll_addr = poll_fn as usize;
    let frame_addr = frame as usize;
    let rt = crate::async_runtime::ffi::全局异步运行时();
    rt.spawn(async move {
        unsafe {
            let pf: extern "C" fn(*mut u8) = std::mem::transmute(poll_addr);
            pf(frame_addr as *mut u8);
        }
    });
}

// ============================================================================
// Future waker 注册 + 状态查询
// ============================================================================

/// 把状态机 poll_fn 注册为 future 的 waker。future complete/fail 时
/// 自动回调 poll_fn(frame)。
/// 如果 future 已经 ready，立即同步调用 poll_fn — 这是 fast path。
#[no_mangle]
pub extern "C" fn qi_future_register_waker(
    fut: *mut Future,
    poll_fn: extern "C" fn(*mut u8),
    frame: *mut u8,
) {
    if fut.is_null() {
        return;
    }
    let waker = StateMachineWaker {
        poll_fn,
        frame: frame as usize,
    };
    unsafe {
        (*fut).register_sm_waker(waker);
    }
}

/// Future 是否已就绪（Completed 或 Failed）。
/// codegen 在 等待 翻译里用：fast-path skip waker 注册如果已经 ready。
#[no_mangle]
pub extern "C" fn qi_future_is_ready(fut: *mut Future) -> i32 {
    if fut.is_null() {
        return 0;
    }
    unsafe {
        let st = (*fut).state.lock().unwrap().clone();
        matches!(st, FutureState::Completed | FutureState::Failed) as i32
    }
}

// ============================================================================
// Future 值读取（codegen 在 await resume 块里调）
// ============================================================================

/// 读取已就绪 future 的 i64 值。未就绪/类型不对返回 0。
/// codegen 应该先 register_waker 拿到 ready 信号才调这个。
#[no_mangle]
pub extern "C" fn qi_future_value_i64(fut: *mut Future) -> i64 {
    if fut.is_null() {
        return 0;
    }
    unsafe {
        match (*fut).value.lock().unwrap().clone() {
            Some(FutureValue::Integer(v)) => v,
            _ => 0,
        }
    }
}

#[no_mangle]
pub extern "C" fn qi_future_value_f64(fut: *mut Future) -> f64 {
    if fut.is_null() {
        return 0.0;
    }
    unsafe {
        match (*fut).value.lock().unwrap().clone() {
            Some(FutureValue::Float(v)) => v,
            _ => 0.0,
        }
    }
}

#[no_mangle]
pub extern "C" fn qi_future_value_bool(fut: *mut Future) -> i32 {
    if fut.is_null() {
        return 0;
    }
    unsafe {
        match (*fut).value.lock().unwrap().clone() {
            Some(FutureValue::Boolean(v)) => v as i32,
            _ => 0,
        }
    }
}

/// 读取已就绪 future 的字符串值。返回新 alloc 的 *mut c_char，调用方
/// 应该用 qi_string_free 释放。
#[no_mangle]
pub extern "C" fn qi_future_value_string(fut: *mut Future) -> *mut c_char {
    if fut.is_null() {
        return CString::new("").unwrap().into_raw();
    }
    unsafe {
        match (*fut).value.lock().unwrap().clone() {
            Some(FutureValue::String(s)) => CString::new(s).unwrap_or_default().into_raw(),
            _ => CString::new("").unwrap().into_raw(),
        }
    }
}

#[no_mangle]
pub extern "C" fn qi_future_value_ptr(fut: *mut Future) -> *mut u8 {
    if fut.is_null() {
        return std::ptr::null_mut();
    }
    unsafe {
        match (*fut).value.lock().unwrap().clone() {
            Some(FutureValue::Pointer(p)) => p,
            _ => std::ptr::null_mut(),
        }
    }
}

// ============================================================================
// Future 完成（codegen 在 异步函数 返回 翻译里调）
// ============================================================================

/// 把 future 标记为 Completed，值为 i64。
/// 触发所有注册的 sm_wakers — 等价于 Future::complete(FutureValue::Integer)。
#[no_mangle]
pub extern "C" fn qi_future_complete_i64(fut: *mut Future, value: i64) {
    if fut.is_null() {
        return;
    }
    unsafe {
        (*fut).complete(FutureValue::Integer(value));
    }
}

#[no_mangle]
pub extern "C" fn qi_future_complete_f64(fut: *mut Future, value: f64) {
    if fut.is_null() {
        return;
    }
    unsafe {
        (*fut).complete(FutureValue::Float(value));
    }
}

#[no_mangle]
pub extern "C" fn qi_future_complete_bool(fut: *mut Future, value: i32) {
    if fut.is_null() {
        return;
    }
    unsafe {
        (*fut).complete(FutureValue::Boolean(value != 0));
    }
}

/// String value 来自 codegen — 是 *const c_char 指针。runtime 把它拷成 String
/// 存进 FutureValue（避免悬空指针）。
#[no_mangle]
pub extern "C" fn qi_future_complete_string(fut: *mut Future, s_ptr: *const c_char) {
    if fut.is_null() {
        return;
    }
    let s = if s_ptr.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(s_ptr).to_string_lossy().into_owned() }
    };
    unsafe {
        (*fut).complete(FutureValue::String(s));
    }
}

#[no_mangle]
pub extern "C" fn qi_future_complete_ptr(fut: *mut Future, ptr: *mut u8) {
    if fut.is_null() {
        return;
    }
    unsafe {
        (*fut).complete(FutureValue::Pointer(ptr));
    }
}

/// 把 future 标记为 Failed，error 文本来自 codegen *const c_char。
#[no_mangle]
pub extern "C" fn qi_future_fail(fut: *mut Future, err_ptr: *const c_char) {
    if fut.is_null() {
        return;
    }
    let err = if err_ptr.is_null() {
        "未知错误".to_string()
    } else {
        unsafe { CStr::from_ptr(err_ptr).to_string_lossy().into_owned() }
    };
    unsafe {
        (*fut).fail(err);
    }
}

// ============================================================================
// 创建 pending Future（异步函数 entry 用）
// ============================================================================

/// 创建一个 pending Future 返回给调用方，状态机 poll fn 完成时通过
/// qi_future_complete_* 写入值。
#[no_mangle]
pub extern "C" fn qi_future_pending() -> *mut Future {
    Box::into_raw(Box::new(Future::pending()))
}

// ============================================================================
// 调试统计（可选，方便 bench 时观察）
// ============================================================================

static FRAMES_ALLOCATED: AtomicI64 = AtomicI64::new(0);
static FRAMES_FREED: AtomicI64 = AtomicI64::new(0);
static SPAWN_POLL_CALLS: AtomicI64 = AtomicI64::new(0);

#[no_mangle]
pub extern "C" fn qi_async_state_machine_metrics() -> *mut c_char {
    let alloc = FRAMES_ALLOCATED.load(Ordering::Relaxed);
    let freed = FRAMES_FREED.load(Ordering::Relaxed);
    let spawn = SPAWN_POLL_CALLS.load(Ordering::Relaxed);
    let live = alloc - freed;
    let s = format!(
        "{{\"frames_allocated\":{},\"frames_freed\":{},\"frames_live\":{},\"spawn_poll_calls\":{}}}",
        alloc, freed, live, spawn
    );
    CString::new(s).unwrap_or_default().into_raw()
}

// ============================================================================
// 单元测试 — 仿照 state_machine_poc.rs 的等价测试，但走 FFI 接口
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// 用 FFI 拼出等价于 PoC 的状态机：
    ///   异步函数 加一异步(): 未来<整数> { 返回 等待 异步获取40() + 2 }
    /// 期望返回 42。
    #[repr(C)]
    struct TestFrame {
        state: i64,
        awaited_future: *mut Future,
        return_future: *mut Future,
        local_a: i64,
    }

    extern "C" fn test_poll_fn(frame_ptr: *mut u8) {
        let frame = unsafe { &mut *(frame_ptr as *mut TestFrame) };
        loop {
            match frame.state {
                0 => {
                    // 等待 异步获取40()
                    let fut = make_test_future_with_delay(40, 5);
                    frame.awaited_future = fut;
                    if qi_future_is_ready(fut) != 0 {
                        frame.state = 1;
                        continue;
                    }
                    frame.state = 1;
                    qi_future_register_waker(fut, test_poll_fn, frame_ptr);
                    return;
                }
                1 => {
                    let v = qi_future_value_i64(frame.awaited_future);
                    frame.local_a = v;
                    unsafe {
                        drop(Box::from_raw(frame.awaited_future));
                    }
                    frame.awaited_future = std::ptr::null_mut();

                    let result = frame.local_a + 2;
                    qi_future_complete_i64(frame.return_future, result);
                    frame.state = -1;
                    qi_async_free_frame(frame_ptr, std::mem::size_of::<TestFrame>() as i64);
                    return;
                }
                _ => return,
            }
        }
    }

    fn make_test_future_with_delay(value: i64, delay_ms: u64) -> *mut Future {
        let fut_ptr = qi_future_pending();
        let addr = fut_ptr as usize;
        let _ = Arc::new(()); // keep clippy quiet
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            unsafe {
                let f = &*(addr as *const Future);
                f.complete(FutureValue::Integer(value));
            }
        });
        fut_ptr
    }

    #[test]
    fn ffi_state_machine_returns_42() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();

        let result = rt.block_on(async {
            // 模拟 codegen 生成的 entry fn
            let frame_ptr = qi_async_alloc_frame(std::mem::size_of::<TestFrame>() as i64);
            assert!(!frame_ptr.is_null());

            let return_fut = qi_future_pending();
            unsafe {
                let frame = &mut *(frame_ptr as *mut TestFrame);
                frame.state = 0;
                frame.return_future = return_fut;
                frame.awaited_future = std::ptr::null_mut();
                frame.local_a = 0;
            }

            // qi_async_spawn_poll 在当前 tokio runtime 上 spawn 首次 poll
            // 用 usize 跨 Send 边界（裸 *mut u8 不是 Send）
            let frame_addr = frame_ptr as usize;
            tokio::spawn(async move {
                test_poll_fn(frame_addr as *mut u8);
            });

            // 等 return_fut ready
            loop {
                if qi_future_is_ready(return_fut) != 0 {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(1)).await;
            }

            let value = qi_future_value_i64(return_fut);
            unsafe {
                drop(Box::from_raw(return_fut));
            }
            value
        });

        assert_eq!(result, 42, "状态机 FFI 路径应返回 42");
    }

    #[test]
    fn alloc_free_roundtrip() {
        let p = qi_async_alloc_frame(64);
        assert!(!p.is_null());
        // zero-init 检查
        unsafe {
            for i in 0..64 {
                assert_eq!(*p.add(i), 0);
            }
        }
        qi_async_free_frame(p, 64);
        // double-free 不该 panic（null path）
        qi_async_free_frame(std::ptr::null_mut(), 64);
        qi_async_free_frame(p, 0); // size 0 no-op
    }

    #[test]
    fn future_complete_value_roundtrip() {
        let fut = qi_future_pending();
        assert_eq!(qi_future_is_ready(fut), 0);
        qi_future_complete_i64(fut, 12345);
        assert_eq!(qi_future_is_ready(fut), 1);
        assert_eq!(qi_future_value_i64(fut), 12345);
        unsafe {
            drop(Box::from_raw(fut));
        }
    }
}
