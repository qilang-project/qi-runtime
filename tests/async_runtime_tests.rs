//! Integration tests for the Qi async runtime

use std::time::Duration;

use qi_runtime::async_runtime::{TaskPriority, TaskStatus};
use qi_runtime::{AsyncRuntime, AsyncRuntimeConfig};

#[tokio::test]
async fn test_async_runtime_task_execution() {
    let runtime =
        AsyncRuntime::new(AsyncRuntimeConfig::default()).expect("runtime should be created");

    let task = runtime.spawn(async {
        tokio::time::sleep(Duration::from_millis(50)).await;
    });

    assert_eq!(task.status(), TaskStatus::Pending);

    task.join().await.expect("task should complete");

    // Task has completed - verify it completes successfully
    assert_eq!(task.status(), TaskStatus::Completed);

    runtime.shutdown().expect("runtime should shutdown");
}

#[tokio::test]
async fn test_async_runtime_priority_spawns() {
    let runtime =
        AsyncRuntime::new(AsyncRuntimeConfig::default()).expect("runtime should be created");

    let high = runtime.spawn_with_priority(
        async {
            tokio::time::sleep(Duration::from_millis(10)).await;
        },
        TaskPriority::High,
    );
    let normal = runtime.spawn_with_priority(
        async {
            tokio::time::sleep(Duration::from_millis(20)).await;
        },
        TaskPriority::Normal,
    );
    let low = runtime.spawn_with_priority(
        async {
            tokio::time::sleep(Duration::from_millis(30)).await;
        },
        TaskPriority::Low,
    );

    high.join()
        .await
        .expect("high priority task should complete");
    normal
        .join()
        .await
        .expect("normal priority task should complete");
    low.join().await.expect("low priority task should complete");

    runtime.shutdown().expect("runtime should shutdown");
}

#[tokio::test]
async fn test_async_runtime_task_cancellation() {
    let runtime =
        AsyncRuntime::new(AsyncRuntimeConfig::default()).expect("runtime should be created");

    let task = runtime.spawn(async {
        tokio::time::sleep(Duration::from_secs(2)).await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;
    task.cancel().expect("task should cancel");

    runtime.shutdown().expect("runtime should shutdown");
}
