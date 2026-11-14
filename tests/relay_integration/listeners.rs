use super::*;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acknowledged_listener_changes_and_new_id_owners_preserve_wire_fifo() {
    let server = Server::new(config()).await;
    let mut source = server.join("source").await;
    let mut target = server.join("target").await;
    let mut observer = server.join("re_globl").await;
    observer.authenticate().await;
    let barrier = b"\x82\x01\xf6";
    let expected_barrier = protocol::envelope(source.id, barrier);
    for (sequence, flags) in [7u8, 0, 2, 2, 4, 1, 7, 0, 255, 8, 7]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            observer.command(1, &protocol::uint(u64::from(flags))).await,
            protocol::reply(1, &protocol::uint(u64::from(flags & 7)))
        );
        let value = protocol::uint(sequence as u64);
        let expected = protocol::envelope(source.id, &value);
        source.send(target.id, &value).await;
        assert_eq!(target.read().await, expected);
        // A direct message after the routed value is an exact FIFO barrier:
        // observers get the value only with bit 2, and get this barrier once.
        source.send(observer.id, barrier).await;
        if flags & 2 != 0 {
            assert_eq!(observer.read().await, expected);
        }
        assert_eq!(observer.read().await, expected_barrier);
        let next_id = 1000 + sequence as u64;
        assert_eq!(
            observer.command(3, &protocol::uint(next_id)).await,
            protocol::reply(3, &protocol::uint(next_id))
        );
        observer.id = next_id;
    }
    let observer_id = observer.id;
    observer.close().await;
    eventually(|| server.relay.metrics.disconnected.load(Relaxed) == 1).await;
    let mut replacement = server.join("re_globl").await;
    replacement.authenticate().await;
    assert_eq!(
        replacement.command(3, &protocol::uint(observer_id)).await,
        protocol::reply(3, &protocol::uint(observer_id))
    );
    source.send(target.id, &[0xf5]).await;
    assert_eq!(target.read().await, protocol::envelope(source.id, &[0xf5]));
    source.send(observer_id, barrier).await;
    assert_eq!(replacement.read().await, expected_barrier);
    // The new session has no inherited listener permission despite owning the
    // former listener's ID. Enabling permission takes effect after its reply.
    assert_eq!(replacement.command(1, &[2]).await, protocol::reply(1, &[2]));
    source.send(target.id, &[0xf4]).await;
    let expected = protocol::envelope(source.id, &[0xf4]);
    assert_eq!(target.read().await, expected);
    assert_eq!(replacement.read().await, expected);
    server.stop().await;
}
