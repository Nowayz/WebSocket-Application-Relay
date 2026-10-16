//! Direct-peer admission includes unfinished HTTP and both public protocols.
use super::*;
use std::net::{Ipv4Addr, SocketAddr};
use tokio::net::TcpSocket;
use tokio_tungstenite::client_async;

async fn connect_from(url: &str, address: u8) -> TcpStream {
    let destination: SocketAddr = url
        .strip_prefix("ws://")
        .unwrap()
        .split('/')
        .next()
        .unwrap()
        .parse()
        .unwrap();
    let socket = TcpSocket::new_v4().unwrap();
    socket
        .bind(SocketAddr::from((Ipv4Addr::new(127, 0, 0, address), 0)))
        .unwrap();
    socket.connect(destination).await.unwrap()
}

async fn upgrade_from(
    url: &str,
    address: u8,
    cbor: bool,
) -> Result<Socket, tokio_tungstenite::tungstenite::Error> {
    let stream = connect_from(url, address).await;
    let mut request = url.into_client_request().unwrap();
    if cbor {
        request.headers_mut().insert(
            "sec-websocket-protocol",
            protocol::SUBPROTOCOL.parse().unwrap(),
        );
    }
    client_async(request, MaybeTlsStream::Plain(stream))
        .await
        .map(|(socket, _)| socket)
}

async fn join_from(url: &str, address: u8) -> (Socket, u64) {
    let mut socket = upgrade_from(url, address, true).await.unwrap();
    socket
        .send(Message::Binary(protocol::text(&format!("ip{address}"))))
        .await
        .unwrap();
    let id = Decoder::new(&data(&mut socket).await).u64().unwrap();
    (socket, id)
}

async fn denied(mut socket: TcpStream) {
    let result = timeout(Duration::from_secs(3), socket.read(&mut [0]))
        .await
        .unwrap();
    assert!(
        matches!(result, Ok(0))
            || matches!(result,Err(ref error) if error.kind()==std::io::ErrorKind::ConnectionReset)
    );
}

async fn connections(server: &Server, expected: usize) {
    timeout(Duration::from_secs(3), async {
        while server.relay.metrics.connections.load(Relaxed) != expected {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

async fn clean_close(socket: &mut Socket) {
    socket.close(None).await.unwrap();
    assert_eq!(
        timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Close(None)
    );
    assert!(
        timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_ip_admission_spans_workers_and_keeps_other_addresses_usable() {
    let server = Server::new(Config {
        workers: Some(4),
        max_connections: 4,
        max_connections_per_ip: 1,
        handshake_timeout_secs: 30,
        ..config()
    })
    .await;
    let half = connect_from(&server.url, 2).await;
    connections(&server, 1).await;
    timeout(Duration::from_secs(3), async {
        while server
            .relay
            .metrics
            .read_budget
            .as_ref()
            .unwrap()
            .reserved()
            == 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    futures_util::future::join_all((0..32).map(|_| async {
        denied(connect_from(&server.url, 2).await).await;
    }))
    .await;
    assert_eq!(server.relay.metrics.ip_rejected.load(Relaxed), 32);
    assert_eq!(server.relay.metrics.rejected.load(Relaxed), 32);
    let mut spoof = connect_from(&server.url, 2).await;
    let _ = spoof.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\nX-Forwarded-For: 127.0.0.9\r\nForwarded: for=127.0.0.9\r\n\r\n").await;
    denied(spoof).await;
    assert_eq!(server.relay.metrics.ip_rejected.load(Relaxed), 33);
    let (mut healthy, id) = join_from(&server.url, 3).await;
    let value = protocol::byte_string(&vec![0x7b; 32 * 1024]);
    send(&mut healthy, id, &value).await;
    assert_eq!(data(&mut healthy).await, protocol::envelope(id, &value));
    let (mut fourth, _) = join_from(&server.url, 4).await;
    let (mut fifth, _) = join_from(&server.url, 5).await;
    connections(&server, 4).await;
    denied(connect_from(&server.url, 6).await).await;
    assert_eq!(server.relay.metrics.rejected.load(Relaxed), 34);
    assert_eq!(server.relay.metrics.ip_rejected.load(Relaxed), 33);
    clean_close(&mut fourth).await;
    clean_close(&mut fifth).await;
    drop(half);
    connections(&server, 1).await;
    let (mut replacement, _) = join_from(&server.url, 2).await;
    clean_close(&mut replacement).await;
    clean_close(&mut healthy).await;
    connections(&server, 0).await;
    assert_eq!(server.relay.metrics.join_rejected.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn handshake_timeout_releases_the_address_lease() {
    let server = Server::new(Config {
        workers: Some(1),
        max_connections_per_ip: 1,
        handshake_timeout_secs: 1,
        ..config()
    })
    .await;
    let _half = connect_from(&server.url, 2).await;
    connections(&server, 1).await;
    denied(connect_from(&server.url, 2).await).await;
    wait_counter(&server.relay.metrics.handshake_errors, 1).await;
    connections(&server, 0).await;
    let (mut replacement, _) = join_from(&server.url, 2).await;
    clean_close(&mut replacement).await;
    server.stop().await;
}

#[tokio::test]
async fn legacy_ingress_enforces_its_own_direct_peer_limit_before_upgrade() {
    let server = Server::new(config()).await;
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}/", listener.local_addr().unwrap());
    let shutdown = CancellationToken::new();
    let task = tokio::spawn(war_relay::legacy_bridge::serve_with_limits(
        listener,
        server.url.clone(),
        4,
        1,
        war_relay::origin::OriginPolicy::default(),
        shutdown.clone(),
    ));
    let half = connect_from(&url, 2).await;
    // A same-address connection can only be accepted after the first accept;
    // the single bridge accept loop serializes their address leases.
    denied(connect_from(&url, 2).await).await;
    let mut healthy = upgrade_from(&url, 3, false).await.unwrap();
    healthy
        .send(Message::Text("ip-legacy3".into()))
        .await
        .unwrap();
    let welcome = data(&mut healthy).await;
    let id = u64::from_le_bytes(welcome.as_ref().try_into().unwrap());
    let mut packet = id.to_le_bytes().to_vec();
    packet.extend_from_slice(&[0, 1, 2, 255]);
    healthy
        .send(Message::Binary(packet.clone().into()))
        .await
        .unwrap();
    assert_eq!(data(&mut healthy).await, packet);
    drop(half);
    let mut replacement = timeout(Duration::from_secs(3), async {
        loop {
            if let Ok(socket) = upgrade_from(&url, 2, false).await {
                break socket;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    replacement
        .send(Message::Text("ip-legacy2".into()))
        .await
        .unwrap();
    assert_eq!(data(&mut replacement).await.len(), 8);
    clean_close(&mut replacement).await;
    clean_close(&mut healthy).await;
    let pending = connect_from(&url, 2).await;
    shutdown.cancel();
    timeout(Duration::from_secs(3), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    denied(pending).await;
    connections(&server, 0).await;
    server.stop().await;
}
