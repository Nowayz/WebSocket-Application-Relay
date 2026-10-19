use super::*;
use futures_util::poll;
use std::sync::atomic::AtomicBool;

#[tokio::test]
async fn cleanup_joins_remaining_workers_after_error_or_panic() {
    for panic in [false, true] {
        let mut tasks = JoinSet::new();
        let failed = tasks.spawn(async move {
            assert!(!panic, "controlled service panic");
            anyhow::bail!("first service failure")
        });
        tokio::time::timeout(Duration::from_secs(1), async {
            while !failed.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let (release, wait) = tokio::sync::oneshot::channel();
        let retired = Arc::new(AtomicBool::new(false));
        let observed = retired.clone();
        tasks.spawn(async move {
            wait.await.unwrap();
            observed.store(true, Relaxed);
            anyhow::bail!("later service failure")
        });
        let mut cleanup = Box::pin(drain_service_workers(&mut tasks, Ok(())));
        assert!(
            poll!(cleanup.as_mut()).is_pending(),
            "an earlier failure must not abandon a still-running worker"
        );
        release.send(()).unwrap();
        let error = tokio::time::timeout(Duration::from_secs(1), cleanup)
            .await
            .unwrap()
            .unwrap_err();
        assert!(retired.load(Relaxed));
        assert!(tasks.is_empty());
        assert_eq!(
            error.to_string(),
            if panic {
                "service worker panicked"
            } else {
                "first service failure"
            }
        );
    }
}

#[tokio::test]
async fn cleanup_still_obeys_the_outer_deadline_after_an_error() {
    let mut tasks = JoinSet::new();
    tasks.spawn(async { anyhow::bail!("service failure") });
    let pending = tasks.spawn(std::future::pending::<Result<()>>());
    assert!(
        tokio::time::timeout(
            Duration::from_millis(1),
            drain_service_workers(&mut tasks, Ok(()))
        )
        .await
        .is_err(),
        "a failed worker must not bypass the wait for the outer deadline"
    );
    assert!(!pending.is_finished());
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
}

#[tokio::test]
async fn cleanup_preserves_the_failure_that_triggered_shutdown() {
    let mut tasks = JoinSet::new();
    tasks.spawn(async { anyhow::bail!("later cleanup failure") });
    let error = drain_service_workers(&mut tasks, Err(anyhow::anyhow!("triggering failure")))
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "triggering failure");
    assert!(tasks.is_empty());
}
