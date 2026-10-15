use std::{
    sync::{Arc, atomic::Ordering::Relaxed},
    time::Duration,
};

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use minicbor::{Decoder, Encoder};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::JoinHandle,
    time::timeout,
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{Message, client::IntoClientRequest},
};
use tokio_util::sync::CancellationToken;
use war_relay::{
    Config, Relay,
    protocol::{self, BROADCAST},
};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

struct Server {
    relay: Arc<Relay>,
    url: String,
    admin: std::net::SocketAddr,
    shutdown: CancellationToken,
    task: Option<JoinHandle<anyhow::Result<()>>>,
}

impl Server {
    async fn new(config: Config) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let admin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/any-path", listener.local_addr().unwrap());
        let address = admin.local_addr().unwrap();
        let relay = Relay::new(config).unwrap();
        let shutdown = CancellationToken::new();
        let task = tokio::spawn(war_relay::serve(
            listener,
            Some(admin),
            relay.clone(),
            shutdown.clone(),
        ));
        Self {
            relay,
            url,
            admin: address,
            shutdown,
            task: Some(task),
        }
    }

    async fn join(&self, channel: &str) -> (Socket, u64) {
        let mut request = self.url.as_str().into_client_request().unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            protocol::SUBPROTOCOL.parse().unwrap(),
        );
        let (mut ws, response) = connect_async(request).await.unwrap();
        assert_eq!(
            response.headers()["sec-websocket-protocol"],
            protocol::SUBPROTOCOL
        );
        ws.send(Message::Binary(protocol::text(channel)))
            .await
            .unwrap();
        let welcome = data(&mut ws).await;
        let id = Decoder::new(&welcome).u64().unwrap();
        assert!(id != 0 && id != BROADCAST);
        (ws, id)
    }

    async fn http(&self, method: &str, path: &str, token: Option<&str>) -> String {
        let mut socket = TcpStream::connect(self.admin).await.unwrap();
        let auth = token.map_or(String::new(), |s| format!("Authorization: Bearer {s}\r\n"));
        socket.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        let mut bytes = Vec::new();
        timeout(Duration::from_secs(3), socket.read_to_end(&mut bytes))
            .await
            .unwrap()
            .unwrap();
        String::from_utf8(bytes).unwrap()
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        timeout(Duration::from_secs(3), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(self.relay.counts(), (0, 1));
        assert_eq!(self.relay.metrics.connections.load(Relaxed), 0);
        assert_eq!(self.relay.metrics.queued_bytes.load(Relaxed), 0);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

fn config() -> Config {
    Config {
        admin_token: Some("admin-test".into()),
        auth_password: Some("host-test".into()),
        ping_interval_secs: 0,
        ..Config::default()
    }
}

async fn data(ws: &mut Socket) -> Bytes {
    timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await.unwrap().unwrap() {
                Message::Binary(bytes) => return bytes,
                Message::Ping(_) => ws.flush().await.unwrap(),
                other => panic!("unexpected message: {other:?}"),
            }
        }
    })
    .await
    .unwrap()
}

async fn send(ws: &mut Socket, target: u64, value: &[u8]) {
    ws.send(Message::Binary(protocol::envelope(target, value)))
        .await
        .unwrap();
}

async fn command(ws: &mut Socket, op: u8, value: &[u8]) -> Bytes {
    ws.send(Message::Binary(protocol::request(op, value)))
        .await
        .unwrap();
    data(ws).await
}

async fn authenticate(ws: &mut Socket) {
    assert_eq!(
        command(ws, 0, &protocol::text("host-test")).await,
        protocol::reply(0, &[0xf5])
    );
}

async fn close_code(ws: &mut Socket) -> u16 {
    timeout(Duration::from_secs(3), async {
        loop {
            match ws.next().await.unwrap().unwrap() {
                Message::Close(Some(frame)) => {
                    ws.flush().await.unwrap();
                    return u16::from(frame.code);
                }
                Message::Ping(_) => {
                    let _ = ws.flush().await;
                }
                _ => {}
            }
        }
    })
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn routes_typed_cbor_private_and_channel_messages_without_changing_payload() {
    let server = Server::new(config()).await;
    let (mut a, aid) = server.join("room").await;
    let (mut b, _) = server.join("room").await;
    let (mut outsider, oid) = server.join("elsewhere").await;
    // {"text": "π", "bytes": h'0001', "items": [true, null, -1]} encoded independently.
    let value = [
        0xa3, 0x64, b't', b'e', b'x', b't', 0x62, 0xcf, 0x80, 0x65, b'b', b'y', b't', b'e', b's',
        0x42, 0, 1, 0x65, b'i', b't', b'e', b'm', b's', 0x83, 0xf5, 0xf6, 0x20,
    ];
    send(&mut a, BROADCAST, &value).await;
    assert_eq!(data(&mut b).await, protocol::envelope(aid, &value));
    assert!(
        timeout(Duration::from_millis(30), outsider.next())
            .await
            .is_err()
    );
    assert!(timeout(Duration::from_millis(30), a.next()).await.is_err());
    send(&mut a, oid, &protocol::text("across channels")).await;
    assert_eq!(
        data(&mut outsider).await,
        protocol::envelope(aid, &protocol::text("across channels"))
    );
    send(&mut a, aid, &[0xf5]).await;
    assert_eq!(data(&mut a).await, protocol::envelope(aid, &[0xf5]));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fragmented_input_and_interleaved_control_leave_direct_output_ordered() {
    use tokio_tungstenite::tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };

    let server = Server::new(config()).await;
    let (mut sender, sender_id) = server.join("fragmented").await;
    let (mut recipient, recipient_id) = server.join("fragmented").await;
    let value = protocol::byte_string(&vec![0x5a; 70_000]);
    let encoded = protocol::envelope(recipient_id, &value);
    // Split the CBOR routing header itself, then the application value. The
    // decoder must retain both while this same socket receives other output.
    sender
        .send(Message::Frame(Frame::message(
            encoded.slice(..1),
            OpCode::Data(Data::Binary),
            false,
        )))
        .await
        .unwrap();
    sender
        .send(Message::Ping(Bytes::from_static(b"between")))
        .await
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(3), sender.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Pong(Bytes::from_static(b"between"))
    );
    let during = protocol::text("while input is fragmented");
    send(&mut recipient, sender_id, &during).await;
    assert_eq!(
        data(&mut sender).await,
        protocol::envelope(recipient_id, &during)
    );
    sender
        .send(Message::Frame(Frame::message(
            encoded.slice(1..17),
            OpCode::Data(Data::Continue),
            false,
        )))
        .await
        .unwrap();
    sender
        .send(Message::Frame(Frame::message(
            encoded.slice(17..),
            OpCode::Data(Data::Continue),
            true,
        )))
        .await
        .unwrap();
    assert_eq!(
        data(&mut recipient).await,
        protocol::envelope(sender_id, &value)
    );
    let after = protocol::text("after fragments");
    send(&mut sender, recipient_id, &after).await;
    assert_eq!(
        data(&mut recipient).await,
        protocol::envelope(sender_id, &after)
    );
    drop(sender);
    drop(recipient);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shared_network_payloads_are_reused_and_small_views_after_large_input_are_compacted() {
    let server = Server::new(config()).await;
    let (mut sender, sender_id) = server.join("storage").await;
    let (mut recipient, recipient_id) = server.join("storage").await;
    // Ordinary tiny messages share the normal read slab too; compaction is
    // reserved for connections whose incoming storage has grown beyond it.
    send(&mut sender, recipient_id, &[0xf5]).await;
    assert_eq!(
        data(&mut recipient).await,
        protocol::envelope(sender_id, &[0xf5])
    );
    assert_eq!(server.relay.metrics.routing_reused_bytes.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    let large = protocol::byte_string(&vec![0x5a; 65536]);
    send(&mut sender, recipient_id, &large).await;
    assert_eq!(
        data(&mut recipient).await,
        protocol::envelope(sender_id, &large)
    );
    assert_eq!(
        server.relay.metrics.routing_reused_bytes.load(Relaxed),
        large.len() as u64 + 1
    );
    assert_eq!(server.relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    send(&mut sender, recipient_id, &[0xf6]).await;
    assert_eq!(
        data(&mut recipient).await,
        protocol::envelope(sender_id, &[0xf6])
    );
    assert_eq!(server.relay.metrics.routing_copied_bytes.load(Relaxed), 1);
    drop(sender);
    drop(recipient);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_listeners_and_global_broadcast_preserve_legacy_scope() {
    let server = Server::new(config()).await;
    let (mut global, gid) = server.join("re_globl").await;
    let (mut a, aid) = server.join("alpha").await;
    let (mut b, bid) = server.join("beta").await;
    authenticate(&mut global).await;
    assert_eq!(
        command(&mut global, 1, &[7]).await,
        protocol::reply(1, &[7])
    );
    send(&mut a, BROADCAST, &[1]).await;
    assert_eq!(data(&mut global).await, protocol::envelope(aid, &[1]));
    send(&mut a, bid, &[2]).await;
    assert_eq!(data(&mut b).await, protocol::envelope(aid, &[2]));
    assert_eq!(data(&mut global).await, protocol::envelope(aid, &[2]));
    send(&mut global, BROADCAST, &[3]).await;
    assert_eq!(data(&mut a).await, protocol::envelope(gid, &[3]));
    assert_eq!(data(&mut b).await, protocol::envelope(gid, &[3]));
    a.close(None).await.unwrap();
    assert_eq!(
        data(&mut global).await,
        protocol::envelope(BROADCAST, &protocol::uint(aid))
    );
    assert!(timeout(Duration::from_millis(30), b.next()).await.is_err());
    server.stop().await;
}

#[tokio::test]
async fn variables_are_typed_scoped_and_deleted_when_last_member_leaves() {
    let server = Server::new(config()).await;
    let (mut a, _) = server.join("vars").await;
    let (mut other, _) = server.join("other").await;
    let mut set = Encoder::new(Vec::new());
    set.array(2)
        .unwrap()
        .str("key")
        .unwrap()
        .map(1)
        .unwrap()
        .str("x")
        .unwrap()
        .u64(42)
        .unwrap();
    assert_eq!(
        command(&mut a, 4, &set.into_writer()).await,
        protocol::reply(4, &[0xf5])
    );
    let reply = command(&mut a, 5, &protocol::text("key")).await;
    assert_eq!(
        reply,
        protocol::reply(200, &[0x82, 0xf5, 0xa1, 0x61, b'x', 0x18, 42])
    );
    assert_eq!(
        command(&mut other, 5, &protocol::text("key")).await,
        protocol::reply(200, &[0x82, 0xf4, 0xf6])
    );
    a.close(None).await.unwrap();
    timeout(Duration::from_secs(2), async {
        while server
            .relay
            .channels()
            .iter()
            .any(|c| c.name.as_deref() == Some("vars"))
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (mut new, _) = server.join("vars").await;
    assert_eq!(
        command(&mut new, 5, &protocol::text("key")).await,
        protocol::reply(200, &[0x82, 0xf4, 0xf6])
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fixed_id_takeover_disconnects_old_session_and_keeps_new_routing() {
    let server = Server::new(config()).await;
    let (mut first, _) = server.join("one").await;
    let (mut next, _) = server.join("two").await;
    let (mut sender, sid) = server.join("three").await;
    authenticate(&mut first).await;
    authenticate(&mut next).await;
    assert_eq!(
        command(&mut first, 3, &protocol::uint(42)).await,
        protocol::reply(3, &protocol::uint(42))
    );
    assert_eq!(
        command(&mut next, 3, &protocol::uint(42)).await,
        protocol::reply(3, &protocol::uint(42))
    );
    assert_eq!(close_code(&mut first).await, 4001);
    send(&mut sender, 42, &[0xf5]).await;
    assert_eq!(data(&mut next).await, protocol::envelope(sid, &[0xf5]));
    next.send(Message::Binary(protocol::request(3, &protocol::uint(0))))
        .await
        .unwrap();
    assert_eq!(close_code(&mut next).await, 1008);
    server.stop().await;
}

#[tokio::test]
async fn authentication_channel_queries_and_admin_drain_are_enforced() {
    let server = Server::new(config()).await;
    let (mut ws, _) = server.join("room").await;
    assert_eq!(
        command(&mut ws, 0, &protocol::text("wrong")).await,
        protocol::reply(0, &[0xf4])
    );
    authenticate(&mut ws).await;
    let reply = command(&mut ws, 2, &[0xf6]).await;
    let (_, value) = protocol::parse(&reply).unwrap();
    let mut d = Decoder::new(value);
    assert_eq!(d.array().unwrap(), Some(2));
    assert_eq!(d.u8().unwrap(), 2);
    assert_eq!(d.array().unwrap(), Some(2));
    assert_eq!(d.array().unwrap(), Some(2));
    assert_eq!(d.bytes().unwrap(), b"re_globl");
    assert_eq!(d.u64().unwrap(), 0);
    assert_eq!(d.array().unwrap(), Some(2));
    assert_eq!(d.bytes().unwrap(), b"room");
    assert_eq!(d.u64().unwrap(), 1);
    assert!(
        server
            .http("POST", "/v1/drain", None)
            .await
            .starts_with("HTTP/1.1 401")
    );
    assert!(
        server
            .http("GET", "/metrics", None)
            .await
            .starts_with("HTTP/1.1 401")
    );
    assert!(
        server
            .http("GET", "/metrics", Some("admin-test"))
            .await
            .contains("war_sessions 1")
    );
    assert!(
        server
            .http("POST", "/v1/drain", Some("admin-test"))
            .await
            .starts_with("HTTP/1.1 204")
    );
    assert!(
        server
            .http("GET", "/readyz", None)
            .await
            .starts_with("HTTP/1.1 503")
    );
    assert!(connect_async(&server.url).await.is_err());
    // An existing session still routes while admission is drained.
    let id = Decoder::new(&command(&mut ws, 3, &protocol::uint(123)).await)
        .array()
        .unwrap();
    assert_eq!(id, Some(2));
    send(&mut ws, 123, &[1]).await;
    assert_eq!(data(&mut ws).await, protocol::envelope(123, &[1]));
    assert!(
        server
            .http("POST", "/v1/resume", Some("admin-test"))
            .await
            .starts_with("HTTP/1.1 204")
    );
    let (mut second, sid) = server.join("room").await;
    assert!(
        server
            .http(
                "POST",
                &format!("/v1/sessions/{sid}/disconnect"),
                Some("admin-test")
            )
            .await
            .starts_with("HTTP/1.1 204")
    );
    assert_eq!(close_code(&mut second).await, 4000);
    server.stop().await;
}

#[tokio::test]
async fn malformed_packets_text_and_unauthorized_commands_close_cleanly() {
    let server = Server::new(config()).await;
    for (message, expected) in [
        (Message::Binary(vec![0x82, 1, 0xff].into()), 1002),
        (Message::Text("not cbor".into()), 1003),
        (Message::Binary(protocol::request(2, &[0xf6])), 1008),
        (Message::Binary(protocol::request(255, &[0xf6])), 1002),
        (
            Message::Binary(protocol::request(4, &[0x82, 0x61, b'x'])),
            1002,
        ),
    ] {
        let (mut ws, _) = server.join("malformed").await;
        ws.send(message).await.unwrap();
        assert_eq!(close_code(&mut ws).await, expected);
    }
    let (mut ws, _) = connect_async(&server.url).await.unwrap();
    ws.send(Message::Binary(protocol::text("seventeen-bytes!!")))
        .await
        .unwrap();
    assert_eq!(close_code(&mut ws).await, 1002);
    server.stop().await;
}

#[tokio::test]
async fn token_bucket_limits_apply_immediately_after_join() {
    let server = Server::new(Config {
        messages_per_second: 1,
        ..config()
    })
    .await;
    let (mut ws, _) = server.join("rate").await;
    send(&mut ws, BROADCAST, &[1]).await;
    assert_eq!(close_code(&mut ws).await, 1008);
    assert_eq!(server.relay.metrics.rate_limited.load(Relaxed), 1);
    server.stop().await;
}

#[tokio::test]
async fn channel_admission_variable_and_message_size_limits_are_real_network_limits() {
    let server = Server::new(Config {
        max_channels: 2,
        max_channel_members: 1,
        max_message_bytes: 128,
        queue_bytes: 256,
        max_variable_bytes: 4,
        ..config()
    })
    .await;
    let (_ws, _) = server.join("limit").await;
    for channel in ["limit", "another"] {
        let (mut denied, _) = connect_async(&server.url).await.unwrap();
        denied
            .send(Message::Binary(protocol::text(channel)))
            .await
            .unwrap();
        assert_eq!(close_code(&mut denied).await, 1013);
    }
    let (mut global, _) = server.join("re_globl").await;
    let mut set = Encoder::new(Vec::new());
    set.array(2)
        .unwrap()
        .str("key")
        .unwrap()
        .str("too big")
        .unwrap();
    global
        .send(Message::Binary(protocol::request(4, &set.into_writer())))
        .await
        .unwrap();
    assert_eq!(close_code(&mut global).await, 1013);
    let (mut oversized, _) = server.join("re_globl").await;
    oversized
        .send(Message::Binary(protocol::envelope(
            BROADCAST,
            &protocol::byte_string(&[0; 200]),
        )))
        .await
        .unwrap();
    assert_eq!(close_code(&mut oversized).await, 1009);
    server.stop().await;
}

#[tokio::test]
async fn idle_half_handshakes_are_bounded_and_release_capacity() {
    let server = Server::new(Config {
        max_connections: 1,
        handshake_timeout_secs: 1,
        ..config()
    })
    .await;
    let addr = server
        .url
        .strip_prefix("ws://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let _half_open = TcpStream::connect(addr).await.unwrap();
    timeout(Duration::from_secs(1), async {
        while server.relay.metrics.connections.load(Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(connect_async(&server.url).await.is_err());
    tokio::time::sleep(Duration::from_millis(1100)).await;
    let (_ws, _) = server.join("admitted").await;
    assert_eq!(server.relay.metrics.handshake_errors.load(Relaxed), 1);
    server.stop().await;
}

#[tokio::test]
async fn heartbeat_disconnects_a_client_that_never_reads_or_pongs() {
    let server = Server::new(Config {
        ping_interval_secs: 1,
        pong_timeout_secs: 1,
        ..config()
    })
    .await;
    let (mut ws, _) = server.join("heartbeat").await;
    tokio::time::sleep(Duration::from_millis(2200)).await;
    assert_eq!(close_code(&mut ws).await, 1001);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_of_a_large_channel_does_not_generate_disconnect_fanout() {
    let server = Server::new(config()).await;
    let mut sockets = Vec::new();
    for _ in 0..100 {
        sockets.push(server.join("shutdown").await.0);
    }
    server.shutdown.cancel();
    for ws in &mut sockets {
        // close_code would ignore binary frames; explicitly forbid notifications.
        assert!(
            matches!(timeout(Duration::from_secs(3), ws.next()).await.unwrap().unwrap().unwrap(), Message::Close(Some(frame)) if u16::from(frame.code) == 1001)
        );
    }
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn channel_list_output_uses_output_budget_instead_of_incoming_message_limit() {
    let server = Server::new(Config {
        max_message_bytes: 128,
        queue_bytes: 16 * 1024,
        ..config()
    })
    .await;
    let mut sockets = Vec::new();
    for i in 0..300 {
        sockets.push(server.join(&format!("c{i:015}")).await.0);
    }
    let (mut host, _) = server.join("host").await;
    authenticate(&mut host).await;
    let reply = command(&mut host, 2, &[0xf6]).await;
    assert!(reply.len() > 128 + 4096);
    let (_, value) = protocol::parse(&reply).unwrap();
    let mut d = Decoder::new(value);
    assert_eq!(d.array().unwrap(), Some(2));
    assert_eq!(d.u8().unwrap(), 2);
    assert_eq!(d.array().unwrap(), Some(302));
    server.stop().await;
}

#[tokio::test]
async fn peer_initiated_close_is_acknowledged_before_the_transport_is_dropped() {
    let server = Server::new(config()).await;
    let (mut ws, _) = server.join("close-ack").await;
    ws.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
        code: 1000.into(),
        reason: "client finished".into(),
    }))
    .await
    .unwrap();
    let reply = timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(reply, Message::Close(Some(frame)) if u16::from(frame.code)==1000 && frame.reason=="client finished")
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_peer_closes_finish_handshakes_during_notification_pressure() {
    let server = Server::new(Config {
        queue_messages: 4,
        ..config()
    })
    .await;
    let mut clients = Vec::with_capacity(128);
    for _ in 0..128 {
        clients.push(server.join("close-burst").await.0);
    }
    let gate = Arc::new(tokio::sync::Barrier::new(clients.len() + 1));
    let mut closing = tokio::task::JoinSet::new();
    for mut ws in clients {
        let gate = gate.clone();
        closing.spawn(async move {
            gate.wait().await;
            ws.close(None).await.unwrap();
            timeout(Duration::from_secs(3), async {
                loop {
                    match ws.next().await.unwrap().unwrap() {
                        Message::Binary(bytes) => {
                            assert_eq!(protocol::parse(&bytes).unwrap().0, BROADCAST)
                        }
                        Message::Close(frame) => {
                            // The server may have sent its capacity Close before
                            // this peer's Close arrived. Either completes cleanly.
                            assert!(frame.is_none_or(|frame| u16::from(frame.code) == 1013));
                            ws.flush().await.unwrap();
                            break;
                        }
                        other => panic!("unexpected closing message: {other:?}"),
                    }
                }
            })
            .await
            .unwrap();
        });
    }
    gate.wait().await;
    while let Some(result) = closing.join_next().await {
        result.unwrap();
    }
    assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.write_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.transport_disconnects.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn close_before_join_preserves_the_peers_code_and_reason() {
    let server = Server::new(config()).await;
    let (mut ws, _) = connect_async(&server.url).await.unwrap();
    ws.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
        code: 4002.into(),
        reason: "no subscription".into(),
    }))
    .await
    .unwrap();
    assert!(
        matches!(timeout(Duration::from_secs(2),ws.next()).await.unwrap().unwrap().unwrap(), Message::Close(Some(frame)) if u16::from(frame.code)==4002 && frame.reason=="no subscription")
    );
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn exhausted_input_quota_still_acknowledges_close_before_and_after_join() {
    for joined in [false, true] {
        let server = Server::new(Config {
            messages_per_second: 1,
            bytes_per_second: 0,
            ..config()
        })
        .await;
        let mut ws = if joined {
            server.join("close-quota").await.0
        } else {
            let (mut ws, _) = connect_async(&server.url).await.unwrap();
            ws.send(Message::Ping(Bytes::from_static(b"quota")))
                .await
                .unwrap();
            assert!(matches!(
                timeout(Duration::from_secs(2), ws.next())
                    .await
                    .unwrap()
                    .unwrap()
                    .unwrap(),
                Message::Pong(_)
            ));
            ws
        };
        ws.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: 4003.into(),
            reason: "allow cleanup".into(),
        }))
        .await
        .unwrap();
        assert!(
            matches!(timeout(Duration::from_secs(2), ws.next()).await.unwrap().unwrap().unwrap(), Message::Close(Some(frame)) if u16::from(frame.code) == 4003 && frame.reason == "allow cleanup")
        );
        ws.flush().await.unwrap();
        assert_eq!(server.relay.metrics.rate_limited.load(Relaxed), 0);
        assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
        server.stop().await;
    }
}

#[tokio::test]
async fn an_abrupt_transport_drop_is_counted_separately_from_protocol_violations() {
    let server = Server::new(config()).await;
    let (ws, _) = server.join("abrupt").await;
    drop(ws);
    timeout(Duration::from_secs(2), async {
        while server.relay.counts().0 != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.transport_disconnects.load(Relaxed), 1);
    server.stop().await;
}
