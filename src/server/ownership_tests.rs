use super::*;
use std::io::Read;
use tungstenite::protocol::Role;

#[test]
fn a_retained_routing_target_keeps_transport_capacity_until_the_descriptor_closes() {
    let relay = Relay::new(crate::Config {
        max_connections: 1,
        max_connections_per_ip: 1,
        ping_interval_secs: 0,
        ..crate::Config::default()
    })
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let client = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (stream, _) = listener.accept().unwrap();
    stream.set_nonblocking(true).unwrap();
    listener.set_nonblocking(true).unwrap();
    let worker = Worker::new(listener, relay.clone()).unwrap();
    relay.metrics.connections.fetch_add(1, Relaxed);
    let limit = relay.ip_limit.as_ref().unwrap();
    let address = client.local_addr().unwrap().ip();
    let socket = Socket::new(
        mio::net::TcpStream::from_std(stream),
        Token(2),
        worker.registry.clone(),
        worker.signal.clone(),
        ConnectionGuard(relay.metrics.clone(), Some(limit.acquire(address).unwrap())),
        &relay,
    )
    .unwrap();
    let mut connection = Connection::new(socket.clone(), Token(2), None, &relay);
    let registration = relay
        .subscribe_with_output(b"retained", socket.clone())
        .unwrap();
    let retained = registration.peer.clone();
    connection.registration = Some(registration);
    let mut client = WebSocket::from_raw_socket(client, Role::Client, None);
    assert!(matches!(client.read().unwrap(), Message::Binary(_)));
    drop(socket);
    drop(connection);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(
        relay.metrics.connections.load(Relaxed),
        1,
        "retiring membership cannot release capacity while a captured target owns TCP"
    );
    assert!(limit.acquire(address).is_none());
    client.get_mut().set_nonblocking(true).unwrap();
    let mut byte = [0];
    assert_eq!(
        client.get_mut().read(&mut byte).unwrap_err().kind(),
        io::ErrorKind::WouldBlock
    );
    drop(retained);
    assert_eq!(relay.metrics.connections.load(Relaxed), 0);
    assert_eq!(limit.address_count(), 0);
    assert!(limit.acquire(address).is_some());
    client.get_mut().set_nonblocking(false).unwrap();
    assert_eq!(client.get_mut().read(&mut byte).unwrap(), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}
