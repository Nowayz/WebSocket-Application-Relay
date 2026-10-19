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

#[tokio::test]
async fn shutdown_releases_blocked_output_and_pending_upgrades_with_live_clients() {
    use futures_util::{SinkExt, StreamExt};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn joined(
        url: &str,
        address: std::net::SocketAddr,
        channel: &[u8],
        small_receive_buffer: bool,
    ) -> (tokio_tungstenite::WebSocketStream<TcpStream>, u64) {
        let stream = TcpStream::connect(address).await.unwrap();
        if small_receive_buffer {
            socket2::SockRef::from(&stream)
                .set_recv_buffer_size(4096)
                .unwrap();
        }
        let (mut ws, _) = tokio_tungstenite::client_async(url, stream).await.unwrap();
        ws.send(Message::Binary(protocol::byte_string(channel)))
            .await
            .unwrap();
        let Some(Ok(Message::Binary(welcome))) = ws.next().await else {
            panic!("missing welcome");
        };
        let id = minicbor::Decoder::new(&welcome).u64().unwrap();
        (ws, id)
    }

    let relay = Relay::new(crate::Config {
        workers: Some(4),
        ping_interval_secs: 0,
        tcp_send_buffer_bytes: 4096,
        shutdown_timeout_secs: 1,
        write_timeout_secs: 5,
        ..crate::Config::default()
    })
    .unwrap();
    let listener = AsyncListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let url = format!("ws://{address}");
    let stop = CancellationToken::new();
    let _cancel_on_exit = stop.clone().drop_guard();
    let mut services = JoinSet::new();
    services.spawn(serve(listener, None, relay.clone(), stop.clone()));
    let (slow, slow_id) = joined(&url, address, b"shutdown-slow", true).await;
    let (mut idle, _) = joined(&url, address, b"shutdown-idle", false).await;
    let (mut responsive, responsive_id) = joined(&url, address, b"shutdown-good", false).await;
    let mut upgrading = TcpStream::connect(address).await.unwrap();
    upgrading.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
    let value = protocol::byte_string(&vec![0x5a; 64 * 1024]);
    let packet = protocol::envelope(slow_id, &value);
    tokio::time::timeout(Duration::from_secs(3), async {
        for _ in 0..32 {
            responsive
                .send(Message::Binary(packet.clone()))
                .await
                .unwrap();
        }
        // The self reply is a FIFO barrier: cancel only after all submitted
        // private packets have been parsed, so unread input cannot reset Close.
        let barrier = protocol::envelope(responsive_id, &protocol::uint(42));
        responsive
            .send(Message::Binary(barrier.clone()))
            .await
            .unwrap();
        assert_eq!(
            responsive.next().await.unwrap().unwrap(),
            Message::Binary(barrier)
        );
        while relay.metrics.queued_bytes.load(Relaxed) == 0
            || relay.metrics.connections.load(Relaxed) != 4
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("fixture must establish actual deferred output and pending upgrade");
    let queued = relay.metrics.queued_bytes.load(Relaxed);
    assert!(queued > 0 && queued <= 32 * packet.len());
    assert!(queued <= relay.config.total_queue_bytes);
    stop.cancel();
    tokio::time::timeout(Duration::from_secs(3), async {
        let Some(Ok(Message::Close(Some(frame)))) = responsive.next().await else {
            panic!("responsive client did not receive shutdown Close");
        };
        assert_eq!(frame.code, CloseCode::Away);
        assert_eq!(frame.reason, "Server Shutdown");
        responsive.flush().await.unwrap();
        let mut byte = [0];
        match upgrading.read(&mut byte).await {
            Ok(0) => {}
            Err(error) if error.kind() == io::ErrorKind::ConnectionReset => {}
            other => panic!("unfinished upgrade did not retire: {other:?}"),
        }
        services.join_next().await.unwrap().unwrap().unwrap();
    })
    .await
    .expect("unread clients must not extend the configured shutdown deadline");
    // Shutdown flushes Close and retires; it need not wait for peer ACKs.
    let Some(Ok(Message::Close(Some(frame)))) =
        tokio::time::timeout(Duration::from_secs(1), idle.next())
            .await
            .unwrap()
    else {
        panic!("unpolled idle client did not receive shutdown Close");
    };
    assert_eq!(frame.code, CloseCode::Away);
    // All client descriptors are still alive here. Process exit or client Drop
    // must not be needed to release the library's reservations.
    for gauge in [
        &relay.metrics.connections,
        &relay.metrics.queued_bytes,
        &relay.metrics.variable_bytes,
        &relay.metrics.output_queue_capacity_bytes,
    ] {
        assert_eq!(gauge.load(Relaxed), 0);
    }
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.read_budget.as_ref().unwrap().reserved(), 0);
    assert_eq!(relay.metrics.write_timeouts.load(Relaxed), 0);
    drop((slow, idle, responsive, upgrading));
}

#[tokio::test]
async fn impossible_worker_reservations_return_errors_and_release_listeners() {
    let listener = AsyncListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let relay = Relay::new(crate::Config {
        workers: Some(usize::MAX),
        ..crate::Config::default()
    })
    .unwrap();
    let error = serve(listener, None, relay, CancellationToken::new())
        .await
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("cannot reserve native worker storage")
    );
    drop(std::net::TcpListener::bind(address).unwrap());

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    assert!(worker_listeners(listener, usize::MAX, 128).is_err());
    drop(std::net::TcpListener::bind(address).unwrap());
}
