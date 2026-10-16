use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acknowledged_listener_changes_and_new_id_owners_preserve_wire_fifo() {
    let server = Server::new(Config {
        workers: Some(4),
        ..config()
    })
    .await;
    let (mut source, source_id) = server.join("source").await;
    let (mut target, target_id) = server.join("target").await;
    let (mut observer, mut observer_id) = server.join("re_globl").await;
    authenticate(&mut observer).await;
    let barrier = b"\x82\x01\xf6";
    let expected_barrier = protocol::envelope(source_id, barrier);
    for (sequence, flags) in [7u8, 0, 2, 2, 4, 1, 7, 0, 255, 8, 7]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            command(&mut observer, 1, &protocol::uint(u64::from(flags))).await,
            protocol::reply(1, &protocol::uint(u64::from(flags & 7)))
        );
        let value = protocol::uint(sequence as u64);
        let expected = protocol::envelope(source_id, &value);
        send(&mut source, target_id, &value).await;
        assert_eq!(data(&mut target).await, expected);
        // A direct message after the routed value is an exact FIFO barrier:
        // observers get the value only with bit 2, and get this barrier once.
        send(&mut source, observer_id, barrier).await;
        if flags & 2 != 0 {
            assert_eq!(data(&mut observer).await, expected);
        }
        assert_eq!(data(&mut observer).await, expected_barrier);
        let next_id = 1000 + sequence as u64;
        assert_eq!(
            command(&mut observer, 3, &protocol::uint(next_id)).await,
            protocol::reply(3, &protocol::uint(next_id))
        );
        observer_id = next_id;
    }
    observer.close(None).await.unwrap();
    assert_eq!(
        timeout(Duration::from_secs(3), observer.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Close(None)
    );
    assert!(
        timeout(Duration::from_secs(3), observer.next())
            .await
            .unwrap()
            .is_none()
    );
    wait_counter(&server.relay.metrics.disconnected, 1).await;
    let (mut replacement, _) = server.join("re_globl").await;
    authenticate(&mut replacement).await;
    assert_eq!(
        command(&mut replacement, 3, &protocol::uint(observer_id)).await,
        protocol::reply(3, &protocol::uint(observer_id))
    );
    send(&mut source, target_id, &[0xf5]).await;
    assert_eq!(
        data(&mut target).await,
        protocol::envelope(source_id, &[0xf5])
    );
    send(&mut source, observer_id, barrier).await;
    assert_eq!(data(&mut replacement).await, expected_barrier);
    // The new session has no inherited listener permission despite owning the
    // former listener's ID. Enabling permission takes effect after its reply.
    assert_eq!(
        command(&mut replacement, 1, &[2]).await,
        protocol::reply(1, &[2])
    );
    send(&mut source, target_id, &[0xf4]).await;
    let expected = protocol::envelope(source_id, &[0xf4]);
    assert_eq!(data(&mut target).await, expected);
    assert_eq!(data(&mut replacement).await, expected);
    server.stop().await;
}
