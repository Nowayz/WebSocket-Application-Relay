use super::*;
use std::io::Read;

#[test]
fn repeated_listener_readiness_keeps_one_bounded_accept_continuation() {
    let relay = Relay::new(crate::Config {
        max_connections: 1,
        workers: Some(1),
        ping_interval_secs: 0,
        ..crate::Config::default()
    })
    .unwrap();
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    socket
        .bind(
            &"127.0.0.1:0"
                .parse::<std::net::SocketAddr>()
                .unwrap()
                .into(),
        )
        .unwrap();
    socket.listen(256).unwrap();
    socket.set_nonblocking(true).unwrap();
    let listener: std::net::TcpListener = socket.into();
    let address = listener.local_addr().unwrap();
    let mut worker = Worker::new(listener, relay.clone()).unwrap();
    // This real descriptor occupies the global slot throughout the refused wave.
    let mut first = std::net::TcpStream::connect(address).unwrap();
    first
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    worker.accept().unwrap();
    assert_eq!(worker.connections.len(), 1);
    let mut refused: Vec<_> = (0..ACCEPT_QUANTUM * 2)
        .map(|_| {
            let stream = std::net::TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream
        })
        .collect();
    // A second readiness batch may arrive before the queued continuation runs.
    worker.accept().unwrap();
    worker.accept().unwrap();
    let mut byte = [0];
    for stream in &mut refused {
        assert_eq!(stream.read(&mut byte).unwrap(), 0);
    }
    assert_eq!(
        relay.metrics.accepted.load(Relaxed),
        1 + (ACCEPT_QUANTUM * 2) as u64
    );
    assert_eq!(
        relay.metrics.rejected.load(Relaxed),
        (ACCEPT_QUANTUM * 2) as u64
    );
    assert_eq!(relay.metrics.connections.load(Relaxed), 1);
    assert_eq!(
        worker
            .ready
            .iter()
            .filter(|&&token| token == LISTENER)
            .count(),
        1,
        "listener readiness must not accumulate duplicate accept continuations"
    );
    drop(worker);
    assert_eq!(first.read(&mut byte).unwrap(), 0);
    assert_eq!(relay.metrics.connections.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}
