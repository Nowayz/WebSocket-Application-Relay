use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
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

#[path = "relay_integration/admission.rs"]
mod admission;
#[path = "relay_integration/channel_listing.rs"]
mod channel_listing;
#[path = "relay_integration/listeners.rs"]
mod listeners;
#[path = "relay_integration/origin_policy.rs"]
mod origin_policy;

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
        http_at(self.admin, method, path, token).await
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
        if let Some(budget) = &self.relay.metrics.read_budget {
            assert_eq!(
                budget.reserved(),
                0,
                "decoder reservations must retire with transports"
            );
        }
    }
}

async fn http_at(
    admin: std::net::SocketAddr,
    method: &str,
    path: &str,
    token: Option<&str>,
) -> String {
    let mut socket = TcpStream::connect(admin).await.unwrap();
    let auth = token.map_or(String::new(), |s| format!("Authorization: Bearer {s}\r\n"));
    socket.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(3), socket.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8(bytes).unwrap()
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

async fn wait_counter(counter: &AtomicU64, minimum: u64) {
    timeout(Duration::from_secs(3), async {
        while counter.load(Relaxed) < minimum {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
}

async fn prime_read_storage(source: &mut Socket, source_id: u64, receiver_id: u64) {
    let unknown = (1..)
        .find(|id| *id != source_id && *id != receiver_id)
        .unwrap();
    send(
        source,
        unknown,
        &protocol::byte_string(&vec![0x5a; 4 * 1024 * 1024]),
    )
    .await;
    send(source, source_id, &[0]).await;
    assert_eq!(data(source).await, protocol::envelope(source_id, &[0]));
}

async fn hold_fragmented_input(ws: &mut Socket) {
    // One incomplete binary message, spanning more than two parsing quotas.
    let mut wire = vec![0x02, 0x81, 0, 0, 0, 0, 0x82];
    for _ in 0..513 {
        wire.extend_from_slice(&[0, 0x80, 0, 0, 0, 0]);
    }
    ws.get_mut().write_all(&wire).await.unwrap();
    ws.send(Message::Ping(Bytes::from_static(b"held")))
        .await
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), ws.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Pong(Bytes::from_static(b"held"))
    );
}

#[tokio::test]
async fn fragmented_final_growth_fits_staging_at_and_below_the_message_limit() {
    use tokio_tungstenite::tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };
    const MESSAGE_BYTES: usize = 4 * 1024 * 1024;
    for max_message_bytes in [MESSAGE_BYTES, 4 * MESSAGE_BYTES] {
        let server = Server::new(Config {
            max_message_bytes,
            total_read_buffer_bytes: 16 * 1024 * 1024,
            messages_per_second: 0,
            bytes_per_second: 0,
            ..config()
        })
        .await;
        let (mut source, source_id) = server.join("fragment-limit").await;
        let (mut healthy, healthy_id) = server.join("fragment-health").await;
        // An explicit uint64 target makes the packet exactly 4 MiB,
        // including for the rare randomly assigned ID with a shorter preferred form.
        let mut packet = vec![0x82, 0x1b];
        packet.extend_from_slice(&source_id.to_be_bytes());
        packet.extend_from_slice(&protocol::byte_string(&vec![0x5a; MESSAGE_BYTES - 15]));
        assert_eq!(packet.len(), MESSAGE_BYTES);
        let packet = Bytes::from(packet);
        let split = MESSAGE_BYTES * 3 / 4;
        source
            .send(Message::Frame(Frame::message(
                packet.slice(..split),
                OpCode::Data(Data::Binary),
                false,
            )))
            .await
            .unwrap();
        source
            .send(Message::Ping(Bytes::from_static(b"assembled-first")))
            .await
            .unwrap();
        assert_eq!(
            timeout(Duration::from_secs(3), source.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            Message::Pong(Bytes::from_static(b"assembled-first"))
        );
        send(&mut healthy, healthy_id, &[0x18, 42]).await;
        assert_eq!(
            data(&mut healthy).await,
            protocol::envelope(healthy_id, &[0x18, 42])
        );
        source
            .send(Message::Frame(Frame::message(
                packet.slice(split..),
                OpCode::Data(Data::Continue),
                true,
            )))
            .await
            .unwrap();
        let reply = timeout(Duration::from_secs(3), source.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let budget = server.relay.metrics.read_budget.as_ref().unwrap();
        assert!(
            matches!(reply, Message::Binary(_)),
            "valid fragmented input was refused: {reply:?}; peak={}, rejected={}",
            budget.peak(),
            budget.rejected()
        );
        let Message::Binary(reply) = reply else {
            unreachable!()
        };
        let mut decoder = Decoder::new(&reply);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u64().unwrap(), source_id);
        assert_eq!(&reply[decoder.position()..], &packet[10..]);
        assert_eq!(budget.rejected(), 0);
        assert!(budget.peak() <= server.relay.config.total_read_buffer_bytes);
        assert_eq!(server.relay.metrics.routing_copied_bytes.load(Relaxed), 0);
        server.stop().await;
    }
}

#[tokio::test]
async fn input_admission_denies_incomplete_frames_before_and_after_join_and_keeps_routing() {
    let server = Server::new(Config {
        total_read_buffer_bytes: 100000,
        ..config()
    })
    .await;
    let (mut healthy, id) = server.join("budget-good").await;
    for joined in [false, true] {
        let mut offender = if joined {
            server.join("budget-bad").await.0
        } else {
            connect_async(&server.url).await.unwrap().0
        };
        let mut header = vec![0x82, 0xff];
        header.extend_from_slice(&65536u64.to_be_bytes());
        header.extend_from_slice(&[0; 4]);
        offender.get_mut().write_all(&header).await.unwrap();
        assert_eq!(close_code(&mut offender).await, 1013);
        send(&mut healthy, id, &[0x18, 42]).await;
        assert_eq!(
            data(&mut healthy).await,
            protocol::envelope(id, &[0x18, 42])
        );
    }
    let budget = server.relay.metrics.read_budget.as_ref().unwrap();
    assert_eq!(budget.rejected(), 2);
    assert!(budget.peak() <= 100000);
    let text = server.http("GET", "/metrics", Some("admin-test")).await;
    assert!(text.contains("war_read_buffer_reservations_rejected_total 2\n"));
    assert!(text.contains("war_read_buffer_limit_bytes 100000\n"));
    assert!(text.contains("war_join_rejected_total 1\n"));
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn handshake_reservations_reject_with_http_503_and_release_after_abandonment() {
    let server = Server::new(Config {
        total_read_buffer_bytes: 65536,
        ..config()
    })
    .await;
    let address = server
        .url
        .strip_prefix("ws://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let half_open = TcpStream::connect(address).await.unwrap();
    let budget = server.relay.metrics.read_budget.as_ref().unwrap();
    timeout(Duration::from_secs(3), async {
        while budget.reserved() != 40960 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    match connect_async(&server.url).await {
        Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
            assert_eq!(response.status(), 503);
        }
        // Windows discards buffered input on a reset, so when the request
        // bytes race the refusal's FIN the client may see a reset instead.
        #[cfg(windows)]
        Err(tokio_tungstenite::tungstenite::Error::Io(_)) => {}
        other => panic!("exhausted handshake admission must return HTTP 503: {other:?}"),
    }
    assert_eq!(server.relay.metrics.join_rejected.load(Relaxed), 0);
    assert_eq!(budget.reserved(), 40960);
    drop(half_open);
    timeout(Duration::from_secs(3), async {
        while budget.reserved() != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (mut healthy, id) = server.join("budget-after").await;
    send(&mut healthy, id, &[0]).await;
    assert_eq!(data(&mut healthy).await, protocol::envelope(id, &[0]));
    assert_eq!(budget.rejected(), 1);
    assert_eq!(server.relay.metrics.rejected.load(Relaxed), 1);
    server.stop().await;
}

#[tokio::test]
async fn upgrade_input_is_bounded_and_frame_reads_continue_after_the_handoff() {
    let server = Server::new(config()).await;
    let address = server
        .url
        .strip_prefix("ws://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let mut oversized = TcpStream::connect(address).await.unwrap();
    let mut request = b"GET / HTTP/1.1\r\nHost: localhost\r\nX-Large: ".to_vec();
    request.resize(16385, b'x');
    let _ = oversized.write_all(&request).await;
    wait_counter(&server.relay.metrics.handshake_errors, 1).await;
    let budget = server.relay.metrics.read_budget.as_ref().unwrap();
    assert_eq!(budget.reserved(), 0);

    let mut raw = TcpStream::connect(address).await.unwrap();
    let mut request = b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nX-Padding: ".to_vec();
    request.resize(16000, b'x');
    request.extend_from_slice(b"\r\n\r\n");
    raw.write_all(&request).await.unwrap();
    let mut response = Vec::new();
    timeout(Duration::from_secs(3), async {
        while !response.ends_with(b"\r\n\r\n") {
            response.push(raw.read_u8().await.unwrap());
        }
    })
    .await
    .unwrap();
    assert!(response.starts_with(b"HTTP/1.1 101 "));
    let mut ws = WebSocketStream::from_raw_socket(
        MaybeTlsStream::Plain(raw),
        tokio_tungstenite::tungstenite::protocol::Role::Client,
        None,
    )
    .await;
    ws.send(Message::Binary(protocol::text("handoff")))
        .await
        .unwrap();
    let id = Decoder::new(&data(&mut ws).await).u64().unwrap();
    let value = protocol::byte_string(&vec![0x39; 65536]);
    send(&mut ws, id, &value).await;
    assert_eq!(data(&mut ws).await, protocol::envelope(id, &value));
    server.stop().await;
}

#[tokio::test]
async fn enlarged_read_storage_retires_without_closing_sessions_or_copying_later_values() {
    let server = Server::new(Config {
        read_buffer_idle_secs: 1,
        messages_per_second: 0,
        bytes_per_second: 0,
        ..config()
    })
    .await;
    let (mut source, source_id) = server.join("read-retire").await;
    let (mut receiver, receiver_id) = server.join("read-retire").await;
    prime_read_storage(&mut source, source_id, receiver_id).await;
    wait_counter(&server.relay.metrics.read_buffer_reclaims, 1).await;
    assert_eq!(server.relay.counts(), (2, 2));
    assert_eq!(server.relay.metrics.connections.load(Relaxed), 2);
    let copied = server.relay.metrics.routing_copied_bytes.load(Relaxed);
    send(&mut source, receiver_id, &[0x18, 42]).await;
    assert_eq!(
        data(&mut receiver).await,
        protocol::envelope(source_id, &[0x18, 42])
    );
    assert_eq!(
        server.relay.metrics.routing_copied_bytes.load(Relaxed),
        copied
    );
    assert_eq!(
        server
            .relay
            .metrics
            .read_buffer_reclaim_header_bytes
            .load(Relaxed),
        0
    );
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn read_storage_reclamation_defers_an_incomplete_frame_and_preserves_its_bytes() {
    let server = Server::new(Config {
        read_buffer_idle_secs: 1,
        messages_per_second: 0,
        bytes_per_second: 0,
        ..config()
    })
    .await;
    let (mut source, source_id) = server.join("read-busy").await;
    let (mut receiver, receiver_id) = server.join("read-busy").await;
    prime_read_storage(&mut source, source_id, receiver_id).await;
    let value = protocol::byte_string(&vec![0x39; 128 * 1024]);
    let packet = protocol::envelope(receiver_id, &value);
    let mask = [0x17, 0x39, 0x5b, 0x7d];
    let mut wire = vec![0x82, 0xff];
    wire.extend_from_slice(&(packet.len() as u64).to_be_bytes());
    wire.extend_from_slice(&mask);
    wire.extend(
        packet
            .iter()
            .enumerate()
            .map(|(i, byte)| byte ^ mask[i & 3]),
    );
    // A complete header and only part of its body make the decoder busy when
    // the same pinned deadline cancels its pending read future.
    source
        .get_mut()
        .write_all(&wire[..14 + 4096])
        .await
        .unwrap();
    wait_counter(&server.relay.metrics.read_buffer_reclaims_deferred, 1).await;
    assert_eq!(server.relay.metrics.read_buffer_reclaims.load(Relaxed), 0);
    source
        .get_mut()
        .write_all(&wire[14 + 4096..])
        .await
        .unwrap();
    assert_eq!(
        data(&mut receiver).await,
        protocol::envelope(source_id, &value)
    );
    wait_counter(&server.relay.metrics.read_buffer_reclaims, 1).await;
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn reclaiming_between_fragments_preserves_message_assembly_and_ping_replies() {
    use tokio_tungstenite::tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };
    let server = Server::new(Config {
        read_buffer_idle_secs: 1,
        messages_per_second: 0,
        bytes_per_second: 0,
        ..config()
    })
    .await;
    let (mut source, source_id) = server.join("read-fragment").await;
    let (mut receiver, receiver_id) = server.join("read-fragment").await;
    prime_read_storage(&mut source, source_id, receiver_id).await;
    let value = protocol::byte_string(&vec![0x39; 128 * 1024]);
    let packet = protocol::envelope(receiver_id, &value);
    let split = packet.len() / 2;
    source
        .send(Message::Frame(Frame::message(
            packet.slice(..split),
            OpCode::Data(Data::Binary),
            false,
        )))
        .await
        .unwrap();
    wait_counter(&server.relay.metrics.read_buffer_reclaims, 1).await;
    let ping = Bytes::from_static(b"between fragments");
    source.send(Message::Ping(ping.clone())).await.unwrap();
    assert_eq!(
        timeout(Duration::from_secs(3), source.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        Message::Pong(ping)
    );
    source
        .send(Message::Frame(Frame::message(
            packet.slice(split..),
            OpCode::Data(Data::Continue),
            true,
        )))
        .await
        .unwrap();
    assert_eq!(
        data(&mut receiver).await,
        protocol::envelope(source_id, &value)
    );
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
    server.stop().await;
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
async fn shared_network_payloads_are_reused_without_copying_later_base_slab_values() {
    let server = Server::new(config()).await;
    let (mut sender, sender_id) = server.join("storage").await;
    let (mut recipient, recipient_id) = server.join("storage").await;
    // Ordinary tiny messages share the normal read slab too. A completed large
    // receive slab retires before the worker lends storage to another reader.
    send(&mut sender, recipient_id, &[0xf5]).await;
    assert_eq!(
        data(&mut recipient).await,
        protocol::envelope(sender_id, &[0xf5])
    );
    // Delivery can reach the client before its worker publishes batched input
    // totals. Await that publication, then retain the exact counter assertion.
    wait_counter(&server.relay.metrics.routing_reused_bytes, 1).await;
    assert_eq!(server.relay.metrics.routing_reused_bytes.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    let large = protocol::byte_string(&vec![0x5a; 65536]);
    send(&mut sender, recipient_id, &large).await;
    assert_eq!(
        data(&mut recipient).await,
        protocol::envelope(sender_id, &large)
    );
    wait_counter(
        &server.relay.metrics.routing_reused_bytes,
        large.len() as u64 + 1,
    )
    .await;
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
    wait_counter(
        &server.relay.metrics.routing_reused_bytes,
        large.len() as u64 + 2,
    )
    .await;
    assert_eq!(server.relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    assert_eq!(
        server.relay.metrics.routing_reused_bytes.load(Relaxed),
        large.len() as u64 + 2
    );
    drop(sender);
    drop(recipient);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_listeners_and_global_broadcast_preserve_global_scope() {
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
async fn decoder_rejections_before_join_send_policy_close_without_waiting_for_the_body() {
    let server = Server::new(Config {
        max_message_bytes: 128,
        ..config()
    })
    .await;
    let (mut healthy, healthy_id) = server.join("early-health").await;
    for (wire, expected) in [
        // Unmasked data, fragmented Ping, and an oversized masked frame header.
        (&[0x82, 1, 0xf6][..], 1002),
        (&[0x09, 0x80, 0, 0, 0, 0][..], 1002),
        (&[0x82, 0xfe, 0, 129, 0, 0, 0, 0][..], 1009),
    ] {
        let (mut pending, _) = connect_async(&server.url).await.unwrap();
        pending.get_mut().write_all(wire).await.unwrap();
        let reply = timeout(Duration::from_secs(2), pending.next())
            .await
            .unwrap();
        assert!(
            matches!(&reply, Some(Ok(Message::Close(Some(frame)))) if u16::from(frame.code) == expected),
            "before-join decoder rejection should send {expected}: {reply:?}"
        );
        // Failed decoders can only send a best-effort Close. Do not assert a
        // complete acknowledgement exchange on their terminal read path.
        drop(pending);
        send(&mut healthy, healthy_id, &[0x18, 42]).await;
        assert_eq!(
            data(&mut healthy).await,
            protocol::envelope(healthy_id, &[0x18, 42])
        );
    }
    assert_eq!(server.relay.metrics.join_rejected.load(Relaxed), 3);
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.transport_disconnects.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn prejoin_rate_rejection_counts_both_the_stage_and_rate_decision_once() {
    let server = Server::new(Config {
        messages_per_second: 1,
        bytes_per_second: 0,
        ..config()
    })
    .await;
    let (mut source, _) = connect_async(&server.url).await.unwrap();
    source
        .send(Message::Ping(Bytes::from_static(b"first")))
        .await
        .unwrap();
    assert_eq!(
        source.next().await.unwrap().unwrap(),
        Message::Pong(Bytes::from_static(b"first"))
    );
    source
        .send(Message::Ping(Bytes::from_static(b"second")))
        .await
        .unwrap();
    assert_eq!(close_code(&mut source).await, 1008);
    assert_eq!(server.relay.metrics.rate_limited.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.join_rejected.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn join_rejections_are_counted_once_and_exclude_normal_close_and_shutdown() {
    let server = Server::new(config()).await;
    for (message, expected) in [
        (Message::Binary(vec![0xff].into()), 1002),
        (Message::Binary(protocol::text("seventeen-bytes!!")), 1002),
        (Message::Text("wrong transport".into()), 1003),
    ] {
        let (mut ws, _) = connect_async(&server.url).await.unwrap();
        ws.send(message).await.unwrap();
        assert_eq!(close_code(&mut ws).await, expected);
    }
    assert_eq!(server.relay.metrics.join_rejected.load(Relaxed), 3);
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    let (mut normal, _) = connect_async(&server.url).await.unwrap();
    normal.close(None).await.unwrap();
    assert!(matches!(
        normal.next().await,
        Some(Ok(Message::Close(None)))
    ));
    normal.flush().await.unwrap();
    let (mut pending, _) = connect_async(&server.url).await.unwrap();
    hold_fragmented_input(&mut pending).await;
    let (mut joined, _) = server.join("shutdown-held").await;
    hold_fragmented_input(&mut joined).await;
    server.shutdown.cancel();
    assert_eq!(close_code(&mut pending).await, 1001);
    assert_eq!(close_code(&mut joined).await, 1001);
    assert_eq!(server.relay.metrics.join_rejected.load(Relaxed), 3);
    server.stop().await;
}

#[tokio::test]
async fn incomplete_join_still_times_out_and_releases_staging_while_healthy_routing_continues() {
    let server = Server::new(Config {
        join_timeout_secs: 1,
        write_timeout_secs: 1,
        ..config()
    })
    .await;
    let (mut healthy, healthy_id) = server.join("deadline-health").await;
    let (mut pending, _) = connect_async(&server.url).await.unwrap();
    hold_fragmented_input(&mut pending).await;
    let (closed, ()) = tokio::join!(close_code(&mut pending), async {
        for _ in 0..75 {
            send(&mut healthy, healthy_id, &[0x18, 42]).await;
            assert_eq!(
                data(&mut healthy).await,
                protocol::envelope(healthy_id, &[0x18, 42])
            );
            tokio::time::sleep(Duration::from_millis(15)).await;
        }
    });
    assert_eq!(closed, 1008);
    assert_eq!(server.relay.metrics.join_rejected.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
    // Empty decoders return their receive slabs to their worker. The remaining
    // allowance is bounded by worker scratch storage, rather than one slab per
    // live connection; the abandoned fragment collector must be gone.
    timeout(Duration::from_secs(3), async {
        while server.relay.metrics.connections.load(Relaxed) != 1 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let workers = server.relay.config.workers.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map_or(2, usize::from)
            .min(16)
    });
    assert!(
        server
            .relay
            .metrics
            .read_budget
            .as_ref()
            .unwrap()
            .reserved()
            <= workers * 8192
    );
    server.stop().await;
}

#[tokio::test]
async fn admin_cancellation_retires_membership_before_closing_incomplete_assembly() {
    let server = Server::new(config()).await;
    let (mut healthy, healthy_id) = server.join("cancel-health").await;
    let (mut source, source_id) = server.join("cancel-held").await;
    hold_fragmented_input(&mut source).await;
    assert!(
        server
            .http(
                "POST",
                &format!("/v1/sessions/{source_id}/disconnect"),
                Some("admin-test")
            )
            .await
            .starts_with("HTTP/1.1 204")
    );
    wait_counter(&server.relay.metrics.disconnected, 1).await;
    assert_eq!(server.relay.counts(), (1, 2));
    assert_eq!(server.relay.metrics.queued_bytes.load(Relaxed), 0);
    send(&mut healthy, healthy_id, &[0x18, 42]).await;
    assert_eq!(
        data(&mut healthy).await,
        protocol::envelope(healthy_id, &[0x18, 42])
    );
    assert_eq!(close_code(&mut source).await, 4000);
    assert_eq!(server.relay.metrics.join_rejected.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn cancelling_the_embedding_service_stops_workers_without_disconnect_fanout() {
    let mut server = Server::new(Config {
        workers: Some(2),
        ..config()
    })
    .await;
    let mut sockets = Vec::new();
    for _ in 0..64 {
        sockets.push(server.join("abort-group").await.0);
    }
    let address = server
        .url
        .strip_prefix("ws://")
        .unwrap()
        .split('/')
        .next()
        .unwrap();
    let mut pending = TcpStream::connect(address).await.unwrap();
    pending
        .write_all(b"GET / HTTP/1.1\r\nHost: pending")
        .await
        .unwrap();
    timeout(Duration::from_secs(3), async {
        while server.relay.metrics.connections.load(Relaxed) != 65
            || server
                .relay
                .metrics
                .read_budget
                .as_ref()
                .unwrap()
                .reserved()
                < 40960
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(
        server
            .relay
            .metrics
            .read_budget
            .as_ref()
            .unwrap()
            .reserved()
            >= 40960
    );
    let delivered = server.relay.metrics.deliveries.load(Relaxed);
    let task = server.task.take().unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    timeout(Duration::from_secs(3), async {
        while server.relay.metrics.connections.load(Relaxed) != 0
            || server
                .relay
                .metrics
                .read_budget
                .as_ref()
                .unwrap()
                .reserved()
                != 0
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert!(server.relay.is_draining());
    let mut byte = [0];
    let ended = timeout(Duration::from_secs(3), pending.read(&mut byte))
        .await
        .unwrap();
    assert!(match ended {
        Ok(0) => true,
        Err(error) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
        ),
        _ => false,
    });
    assert_eq!(server.relay.counts(), (0, 1));
    assert_eq!(server.relay.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.variable_bytes.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.disconnected.load(Relaxed), 64);
    assert_eq!(server.relay.metrics.handshake_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.deliveries.load(Relaxed), delivered);
}

#[test]
fn native_workers_do_not_depend_on_the_embedders_blocking_pool() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
        .block_on(async {
            let (release, held) = std::sync::mpsc::channel();
            let (started, active) = tokio::sync::oneshot::channel();
            let occupied = tokio::task::spawn_blocking(move || {
                let _ = started.send(());
                held.recv_timeout(Duration::from_secs(5)).unwrap();
            });
            active.await.unwrap();
            let server = Server::new(Config {
                workers: Some(4),
                ..config()
            })
            .await;
            let mut sockets = Vec::new();
            for _ in 0..16 {
                let (mut socket, id) = server.join("dedicated-io").await;
                send(&mut socket, id, &[0x18, 42]).await;
                assert_eq!(data(&mut socket).await, protocol::envelope(id, &[0x18, 42]));
                sockets.push(socket);
            }
            server.stop().await;
            release.send(()).unwrap();
            occupied.await.unwrap();
        });
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
async fn reserved_simple_encodings_never_reach_recipients_or_channel_variables() {
    use tokio_tungstenite::tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };

    let server = Server::new(Config {
        messages_per_second: 0,
        bytes_per_second: 0,
        ..config()
    })
    .await;
    let (mut healthy, healthy_id) = server.join("healthy-cbor").await;
    for value in 0..32u8 {
        for prefix in [&[][..], &[0x81], &[0xa1, 0], &[0xc0], &[0xa1]] {
            let (mut bad, _) = server.join("invalid-cbor").await;
            let mut encoded = prefix.to_vec();
            encoded.extend_from_slice(&[0xf8, value]);
            if prefix == [0xa1] {
                encoded.push(0xf6);
            }
            if value.is_multiple_of(2) {
                if let MaybeTlsStream::Plain(tcp) = bad.get_ref() {
                    tcp.set_nodelay(true).unwrap();
                }
                let packet = protocol::envelope(healthy_id, &encoded);
                let split = packet.len() - 1;
                bad.send(Message::Frame(Frame::message(
                    packet.slice(..split),
                    OpCode::Data(Data::Binary),
                    false,
                )))
                .await
                .unwrap();
                bad.send(Message::Ping(Bytes::from_static(b"split-cbor")))
                    .await
                    .unwrap();
                assert_eq!(
                    timeout(Duration::from_secs(2), bad.next())
                        .await
                        .unwrap()
                        .unwrap()
                        .unwrap(),
                    Message::Pong(Bytes::from_static(b"split-cbor"))
                );
                bad.send(Message::Frame(Frame::message(
                    packet.slice(split..),
                    OpCode::Data(Data::Continue),
                    true,
                )))
                .await
                .unwrap();
            } else {
                send(&mut bad, healthy_id, &encoded).await;
            }
            timeout(Duration::from_secs(2), async {
                tokio::select! {
                    incoming = bad.next() => assert!(matches!(incoming.unwrap().unwrap(), Message::Close(Some(frame)) if u16::from(frame.code) == 1002)),
                    incoming = healthy.next() => panic!("malformed value reached a healthy recipient: {incoming:?}"),
                }
            })
            .await
            .unwrap();
            bad.flush().await.unwrap();
            send(&mut healthy, healthy_id, &protocol::uint(u64::from(value))).await;
            assert_eq!(
                data(&mut healthy).await,
                protocol::envelope(healthy_id, &protocol::uint(u64::from(value)))
            );
        }
        let (mut bad, _) = server.join("healthy-cbor").await;
        // [0, [4, ["key", simple(value)]]]; a command must not store it either.
        let command = [0x82, 0, 0x82, 4, 0x82, 0x63, b'k', b'e', b'y', 0xf8, value];
        bad.send(Message::Binary(Bytes::copy_from_slice(&command)))
            .await
            .unwrap();
        assert_eq!(close_code(&mut bad).await, 1002);
        assert_eq!(server.relay.metrics.variable_bytes.load(Relaxed), 0);
        // The bad peer shared this channel, so its departure is observable.
        let departed = data(&mut healthy).await;
        assert_eq!(protocol::parse(&departed).unwrap().0, BROADCAST);
    }
    let (mut valid, valid_id) = server.join("valid-cbor").await;
    for value in (32..=255u8)
        .map(|value| vec![0xf8, value])
        .chain((0xe0..=0xf7).map(|value| vec![value]))
    {
        send(&mut valid, healthy_id, &value).await;
        assert_eq!(
            data(&mut healthy).await,
            protocol::envelope(valid_id, &value)
        );
    }
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 32 * 6);
    for ws in [&mut healthy, &mut valid] {
        ws.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: 1000.into(),
            reason: "".into(),
        }))
        .await
        .unwrap();
        assert_eq!(close_code(ws).await, 1000);
    }
    assert_eq!(server.relay.metrics.transport_disconnects.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn heartbeat_and_control_traffic_do_not_extend_application_idle_policy() {
    let server = Server::new(Config {
        idle_timeout_secs: 1,
        ping_interval_secs: 1,
        pong_timeout_secs: 3,
        max_message_bytes: 17,
        ..config()
    })
    .await;
    let (mut ws, _) = server.join("idle-control").await;
    let mut controls = tokio::time::interval(Duration::from_millis(200));
    timeout(Duration::from_secs(3), async {
        loop {
            tokio::select! {
                _ = controls.tick() => ws.send(Message::Ping(Bytes::from_static(b"keep transport"))).await.unwrap(),
                incoming = ws.next() => match incoming.unwrap().unwrap() {
                    Message::Close(Some(frame)) => {
                        assert_eq!(u16::from(frame.code), 1001);
                        assert_eq!(frame.reason, "Application Idle");
                        ws.flush().await.unwrap();
                        break;
                    }
                    Message::Ping(_) => ws.flush().await.unwrap(),
                    Message::Pong(_) => {},
                    other => panic!("unexpected idle-policy frame: {other:?}"),
                }
            }
        }
    }).await.unwrap();
    assert_eq!(server.relay.metrics.idle_timeouts.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.rate_limited.load(Relaxed), 0);
    // The 18-byte Close payload (status plus reason) must also be accepted as
    // an acknowledgement when the application-message budget is only 17.
    timeout(Duration::from_secs(2), async {
        while server.relay.metrics.connections.load(Relaxed) != 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
    server.stop().await;
}

async fn expect_application_idle_close(ws: &mut Socket) {
    let incoming = timeout(Duration::from_secs(3), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(
        matches!(incoming, Message::Close(Some(frame)) if u16::from(frame.code) == 1001 && frame.reason == "Application Idle")
    );
    ws.flush().await.unwrap();
}

#[tokio::test]
async fn application_input_and_receive_only_output_extend_idle_policy() {
    let server = Server::new(Config {
        idle_timeout_secs: 1,
        ..config()
    })
    .await;
    let (mut sender, sender_id) = server.join("idle-source").await;
    let (mut receiver, receiver_id) = server.join("idle-receiver").await;
    assert_eq!(
        command(&mut sender, 4, &[0x82, 0x61, b'x', 1]).await,
        protocol::reply(4, &[0xf5])
    );
    assert!(server.relay.metrics.variable_bytes.load(Relaxed) > 0);
    // The sender receives no application replies during this sequence. The
    // recipient sends no application input. Both stay active beyond one timeout.
    for sequence in 1..=5 {
        tokio::time::sleep(Duration::from_millis(350)).await;
        let value = protocol::uint(sequence);
        send(&mut sender, receiver_id, &value).await;
        assert_eq!(
            data(&mut receiver).await,
            protocol::envelope(sender_id, &value)
        );
    }
    tokio::join!(
        expect_application_idle_close(&mut sender),
        expect_application_idle_close(&mut receiver)
    );
    assert_eq!(server.relay.metrics.idle_timeouts.load(Relaxed), 2);
    assert_eq!(server.relay.metrics.variable_bytes.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.transport_disconnects.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn unfinished_fragmented_application_input_still_expires_cleanly() {
    use tokio_tungstenite::tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };
    let server = Server::new(Config {
        idle_timeout_secs: 1,
        ..config()
    })
    .await;
    let (mut ws, id) = server.join("idle-fragment").await;
    let packet = protocol::envelope(id, &protocol::byte_string(&vec![0; 1024 * 1024]));
    // The CBOR bytes are well-formed, but the WebSocket message never finishes.
    ws.send(Message::Frame(Frame::message(
        packet,
        OpCode::Data(Data::Binary),
        false,
    )))
    .await
    .unwrap();
    expect_application_idle_close(&mut ws).await;
    assert_eq!(server.relay.metrics.idle_timeouts.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.incoming_messages.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
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
    for idle_timeout_secs in [0, 10] {
        let server = Server::new(Config {
            ping_interval_secs: 1,
            pong_timeout_secs: 1,
            idle_timeout_secs,
            ..config()
        })
        .await;
        let (mut ws, _) = server.join("heartbeat").await;
        // The heartbeat must also pull an already armed ten-second idle wait
        // forward, rather than waiting for that later alarm.
        tokio::time::sleep(Duration::from_millis(2200)).await;
        assert_eq!(close_code(&mut ws).await, 1001);
        assert_eq!(server.relay.metrics.idle_timeouts.load(Relaxed), 0);
        server.stop().await;
    }
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
async fn small_application_limits_allow_full_sized_controls_before_and_after_join() {
    for joined in [false, true] {
        let server = Server::new(Config {
            max_message_bytes: 17,
            messages_per_second: 0,
            bytes_per_second: 0,
            ..config()
        })
        .await;
        let mut ws = if joined {
            server.join("small-limit").await.0
        } else {
            connect_async(&server.url).await.unwrap().0
        };
        let control = Bytes::from(vec![0x5a; 125]);
        ws.send(Message::Pong(control.clone())).await.unwrap();
        ws.send(Message::Ping(control.clone())).await.unwrap();
        assert_eq!(
            timeout(Duration::from_secs(2), ws.next())
                .await
                .unwrap()
                .unwrap()
                .unwrap(),
            Message::Pong(control)
        );
        // The 2-byte status plus 123 UTF-8 bytes fills the control-frame limit.
        let reason = format!("{}x", "π".repeat(61));
        ws.close(Some(tokio_tungstenite::tungstenite::protocol::CloseFrame {
            code: 4003.into(),
            reason: reason.clone().into(),
        }))
        .await
        .unwrap();
        assert!(
            matches!(timeout(Duration::from_secs(2), ws.next()).await.unwrap().unwrap().unwrap(), Message::Close(Some(frame)) if u16::from(frame.code) == 4003 && frame.reason == reason)
        );
        ws.flush().await.unwrap();
        assert_eq!(server.relay.metrics.protocol_errors.load(Relaxed), 0);
        assert_eq!(server.relay.metrics.close_failures.load(Relaxed), 0);
        server.stop().await;
    }
}

#[tokio::test]
async fn small_application_limits_bound_complete_and_fragmented_data() {
    use tokio_tungstenite::tungstenite::protocol::frame::{
        Frame,
        coding::{Data, OpCode},
    };

    for fragmented in [false, true] {
        let server = Server::new(Config {
            max_message_bytes: 17,
            ..config()
        })
        .await;
        let (mut sender, sender_id) = server.join("data-limit").await;
        let (mut reader, _) = server.join("data-limit").await;
        let value = protocol::byte_string(&[0x5a; 6]);
        let allowed = protocol::envelope(BROADCAST, &value);
        assert_eq!(allowed.len(), 17);
        sender.send(Message::Binary(allowed)).await.unwrap();
        assert_eq!(
            data(&mut reader).await,
            protocol::envelope(sender_id, &value)
        );

        let oversized = protocol::envelope(BROADCAST, &protocol::byte_string(&[0x5a; 7]));
        assert_eq!(oversized.len(), 18);
        if fragmented {
            for (body, opcode, final_frame) in [
                (oversized.slice(..9), Data::Binary, false),
                (oversized.slice(9..), Data::Continue, true),
            ] {
                sender
                    .send(Message::Frame(Frame::message(
                        body,
                        OpCode::Data(opcode),
                        final_frame,
                    )))
                    .await
                    .unwrap();
            }
        } else {
            sender.send(Message::Binary(oversized)).await.unwrap();
        }
        assert_eq!(close_code(&mut sender).await, 1009);
        server.stop().await;
    }
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_routing_keeps_progress_through_registry_growth_and_retirement() {
    let server = Server::new(Config {
        messages_per_second: 0,
        bytes_per_second: 0,
        ..config()
    })
    .await;
    let (mut sender, id) = server.join("registry-health").await;
    let (mut receiver, target) = server.join("registry-health").await;
    let finished = CancellationToken::new();
    let phase = AtomicU64::new(0);
    let population = async {
        let service = &server;
        let sockets = futures_util::stream::iter(0..2048)
            .map(|i| async move {
                let room = format!("wave-{i}");
                service.join(&room).await.0
            })
            .buffer_unordered(32)
            .collect::<Vec<_>>()
            .await;
        assert_eq!(server.relay.counts(), (2050, 2050));
        phase.store(1, Relaxed);
        futures_util::stream::iter(sockets)
            .map(|mut socket| async move {
                socket
                    .send(Message::Close(Some(
                        tokio_tungstenite::tungstenite::protocol::CloseFrame {
                            code: 1000.into(),
                            reason: "registry-wave".into(),
                        },
                    )))
                    .await
                    .unwrap();
                assert_eq!(close_code(&mut socket).await, 1000);
                assert!(
                    timeout(Duration::from_secs(3), socket.next())
                        .await
                        .unwrap()
                        .is_none()
                );
            })
            .buffer_unordered(32)
            .collect::<Vec<_>>()
            .await;
        finished.cancel();
    };
    let healthy = async {
        let mut counts = [0u64; 2];
        let mut payload = [0x5a; 1024];
        while !finished.is_cancelled() {
            let stage = phase.load(Relaxed) as usize;
            let sequence: u64 = counts.iter().sum();
            payload[..8].copy_from_slice(&sequence.to_le_bytes());
            let value = protocol::byte_string(&payload);
            send(&mut sender, target, &value).await;
            assert_eq!(data(&mut receiver).await, protocol::envelope(id, &value));
            counts[stage] += 1;
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        counts
    };
    let (_, progress) = tokio::join!(population, healthy);
    assert!(
        progress[0] > 0 && progress[1] > 0,
        "no progress in phase: {progress:?}"
    );
    assert_eq!(server.relay.counts(), (2, 2));
    let metrics = &server.relay.metrics;
    assert_eq!(metrics.joined.load(Relaxed), 2050);
    assert_eq!(metrics.disconnected.load(Relaxed), 2048);
    assert_eq!(metrics.close_failures.load(Relaxed), 0);
    assert_eq!(metrics.transport_disconnects.load(Relaxed), 0);
    assert_eq!(metrics.protocol_errors.load(Relaxed), 0);
    assert_eq!(metrics.routing_copied_bytes.load(Relaxed), 0);
    // Both original peers remain usable after all retirement thresholds.
    send(&mut sender, target, &[0xa1, 0x61, b'x', 0xf5]).await;
    assert_eq!(
        data(&mut receiver).await,
        protocol::envelope(id, &[0xa1, 0x61, b'x', 0xf5])
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recipient_that_stops_reading_is_closed_as_a_slow_consumer_and_the_sender_survives() {
    use std::time::Instant;
    let server = Server::new(Config {
        max_message_bytes: 64 * 1024,
        queue_bytes: 256 * 1024,
        total_queue_bytes: 16 * 1024 * 1024,
        write_timeout_secs: 5,
        ..config()
    })
    .await;
    let (mut sender, sender_id) = server.join("slow-consumer").await;
    let (mut slow, slow_id) = server.join("slow-consumer").await;
    // Read the sender's next application message, skipping the slow peer's
    // departure notice, which may arrive at any point once it is closed.
    async fn echo(ws: &mut Socket, expected: &Bytes) {
        let departure_prefix = protocol::envelope(BROADCAST, &[]);
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let packet = data(ws).await;
            if packet == *expected {
                return;
            }
            assert!(
                packet.starts_with(&departure_prefix[..departure_prefix.len() - 1]),
                "unexpected packet {packet:?}"
            );
            assert!(Instant::now() < deadline);
        }
    }
    // The slow peer never reads. Its kernel window fills first, then its
    // per-session output allowance; the relay must close it, not the sender.
    let value = protocol::byte_string(&vec![0x5a; 32 * 1024]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.relay.metrics.slow_consumers.load(Relaxed) == 0 {
        assert!(Instant::now() < deadline, "slow consumer was never closed");
        send(&mut sender, slow_id, &value).await;
        // Routing stays responsive for the sender throughout.
        send(&mut sender, sender_id, &[7]).await;
        echo(&mut sender, &protocol::envelope(sender_id, &[7])).await;
    }
    assert_eq!(server.relay.metrics.slow_consumers.load(Relaxed), 1);
    // The slow peer observes either a 1013 Close or, when a frame was blocked
    // part way through, a dropped transport. Either way it is gone.
    let outcome = timeout(Duration::from_secs(5), async {
        loop {
            match slow.next().await {
                Some(Ok(Message::Close(frame))) => {
                    assert_eq!(u16::from(frame.unwrap().code), 1013);
                    return;
                }
                Some(Ok(_)) => continue,
                Some(Err(_)) | None => return,
            }
        }
    })
    .await;
    assert!(outcome.is_ok(), "slow peer was not retired");
    drop(slow);
    wait_counter(&server.relay.metrics.disconnected, 1).await;
    // The sender was never penalized for its peer's slowness.
    send(&mut sender, sender_id, &[8]).await;
    echo(&mut sender, &protocol::envelope(sender_id, &[8])).await;
    sender.close(None).await.unwrap();
    let acknowledged = timeout(Duration::from_secs(3), async {
        while let Some(Ok(message)) = sender.next().await {
            if matches!(message, Message::Close(_)) {
                return true;
            }
        }
        false
    })
    .await;
    assert_eq!(acknowledged, Ok(true), "sender must still close normally");
    server.stop().await;
}

async fn join_at(url: &str, channel: &str) -> (Socket, u64) {
    let mut request = url.into_client_request().unwrap();
    request.headers_mut().insert(
        "sec-websocket-protocol",
        protocol::SUBPROTOCOL.parse().unwrap(),
    );
    let (mut ws, _) = connect_async(request).await.unwrap();
    ws.send(Message::Binary(protocol::text(channel)))
        .await
        .unwrap();
    let welcome = data(&mut ws).await;
    (ws, Decoder::new(&welcome).u64().unwrap())
}

/// Nothing may broadly stall other work. While one large room takes a stream
/// of broadcasts above the live-fanout quantum, members join and leave it so
/// departure notices become deferred jobs, and a global observer keeps
/// revoking and restoring its disconnect flag so barriers scan the observer
/// queue, a session in an unrelated room must keep echoing with bounded
/// latency, and shutdown must still finish inside the fixture's deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unrelated_traffic_stays_responsive_under_large_room_churn_and_revocation() {
    use std::{sync::atomic::AtomicBool, time::Instant};
    const MEMBERS: usize = 1100; // above FANOUT_QUANTUM and RETIREMENT_QUANTUM
    let server = Server::new(Config {
        workers: Some(4),
        max_channel_members: 4096,
        ..config()
    })
    .await;
    let url = server.url.clone();
    // Crowd members drain their sockets and count application frames.
    let received = Arc::new(AtomicU64::new(0));
    let mut readers = tokio::task::JoinSet::new();
    for _ in 0..MEMBERS {
        let (mut ws, _) = join_at(&url, "crowd").await;
        let received = received.clone();
        readers.spawn(async move {
            while let Some(Ok(message)) = ws.next().await {
                match message {
                    Message::Binary(_) => {
                        received.fetch_add(1, Relaxed);
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        });
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut churn = tokio::task::JoinSet::new();
    // Broadcaster: a steady stream of room broadcasts, each a continuation.
    {
        let (mut ws, _) = join_at(&url, "crowd").await;
        let stop = stop.clone();
        churn.spawn(async move {
            let value = protocol::byte_string(&vec![0x5a; 1024]);
            let mut sent = 0u64;
            while !stop.load(Relaxed) {
                send(&mut ws, BROADCAST, &value).await;
                sent += 1;
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            let _ = ws.close(None).await;
            sent
        });
    }
    // Join and leave churn: every departure notifies the whole crowd.
    {
        let url = url.clone();
        let stop = stop.clone();
        churn.spawn(async move {
            let mut departures = 0u64;
            while !stop.load(Relaxed) {
                let (mut ws, _) = join_at(&url, "crowd").await;
                let _ = ws.close(None).await;
                departures += 1;
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
            departures
        });
    }
    // Observer revocation: every clear publishes a new snapshot and scans
    // the observer shards and the publication index for pending notices;
    // with both bits set, broadcasts are captured as well as departures.
    {
        let (mut ws, _) = join_at(&url, "re_globl").await;
        assert_eq!(
            command(&mut ws, 0, &protocol::text("host-test")).await,
            protocol::reply(0, &[0xf5])
        );
        let stop = stop.clone();
        churn.spawn(async move {
            let mut toggles = 0u64;
            let mut notices = 0u64;
            let mut captured = 0u64;
            while !stop.load(Relaxed) {
                for flags in [5u64, 0] {
                    // While the flags are set, departure notices and captured
                    // broadcasts interleave with the reply; the barrier
                    // delivers pending ones first.
                    let reply = protocol::reply(1, &protocol::uint(flags));
                    let mut packet = command(&mut ws, 1, &protocol::uint(flags)).await;
                    while packet != reply {
                        if protocol::parse(&packet).unwrap().0 == BROADCAST {
                            notices += 1;
                        } else {
                            captured += 1;
                        }
                        packet = data(&mut ws).await;
                    }
                    // Keep the flags set long enough for departures and
                    // broadcasts to land while they are on, so each
                    // revocation has work to flush.
                    tokio::time::sleep(Duration::from_millis(if flags == 5 { 30 } else { 5 }))
                        .await;
                }
                toggles += 1;
            }
            let _ = ws.close(None).await;
            assert!(notices > 0, "the observer saw no departure notices");
            assert!(captured > 0, "the observer captured no broadcasts");
            toggles
        });
    }
    // The probe lives in its own room and must never wait long for an echo.
    let (mut probe, probe_id) = join_at(&url, "probe").await;
    let end = Instant::now() + Duration::from_millis(2500);
    let mut samples = 0u32;
    let mut worst = Duration::ZERO;
    let mut total = Duration::ZERO;
    while Instant::now() < end {
        let started = Instant::now();
        send(&mut probe, probe_id, &[0x18, samples as u8]).await;
        assert_eq!(
            data(&mut probe).await,
            protocol::envelope(probe_id, &[0x18, samples as u8])
        );
        let elapsed = started.elapsed();
        worst = worst.max(elapsed);
        total += elapsed;
        samples += 1;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stop.store(true, Relaxed);
    let mut churn_counts = Vec::new();
    while let Some(result) = churn.join_next().await {
        churn_counts.push(result.unwrap());
    }
    eprintln!(
        "probe: {samples} echoes, worst {worst:?}, mean {:?}; churn counts {churn_counts:?}; crowd frames {}",
        total / samples.max(1),
        received.load(Relaxed)
    );
    assert!(samples >= 50, "probe produced too few samples: {samples}");
    assert!(
        worst < Duration::from_secs(1),
        "unrelated echo stalled for {worst:?} during large-room churn"
    );
    assert!(churn_counts.iter().all(|&count| count > 0));
    assert!(received.load(Relaxed) > 0);
    let _ = probe.close(None).await;
    // Shutdown with the crowd still connected must finish inside the
    // fixture's three-second bound and leave every gauge at zero.
    server.stop().await;
    readers.abort_all();
    while readers.join_next().await.is_some() {}
}

/// The other stall-prone path: room creation and retirement write the
/// channel index, every membership change invalidates the global recipient
/// view, which the next global broadcast rebuilds from every room, and the
/// admin channel listing walks the whole index; with an observer crowd larger
/// than one turn, every departure is a queued observer job, worker turns keep
/// taking the shards, and every revocation is a chunked barrier scan racing
/// them. Under continuous room churn and a stream of
/// global broadcasts, a session in a stable room must keep echoing and the
/// listing must keep answering, both with bounded latency.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unrelated_traffic_stays_responsive_under_room_churn_and_global_broadcasts() {
    use std::{sync::atomic::AtomicBool, time::Instant};
    const ROOMS: usize = 150;
    const PER_ROOM: usize = 4;
    let server = Server::new(Config {
        workers: Some(4),
        max_channels: 10_000,
        ..config()
    })
    .await;
    let url = server.url.clone();
    // A population spread over many rooms; every member drains its socket.
    let received = Arc::new(AtomicU64::new(0));
    let mut readers = tokio::task::JoinSet::new();
    for room in 0..ROOMS {
        for _ in 0..PER_ROOM {
            let (mut ws, _) = join_at(&url, &format!("r{room}")).await;
            let received = received.clone();
            readers.spawn(async move {
                while let Some(Ok(message)) = ws.next().await {
                    match message {
                        Message::Binary(_) => {
                            received.fetch_add(1, Relaxed);
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            });
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut churn = tokio::task::JoinSet::new();
    // Global broadcaster: each broadcast rebuilds the invalidated global view.
    {
        let (mut ws, _) = join_at(&url, "re_globl").await;
        let stop = stop.clone();
        churn.spawn(async move {
            let value = protocol::byte_string(&vec![0x5a; 256]);
            let mut sent = 0u64;
            while !stop.load(Relaxed) {
                send(&mut ws, BROADCAST, &value).await;
                sent += 1;
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
            let _ = ws.close(None).await;
            sent
        });
    }
    // Room churn: fresh rooms are created and retired as fast as the client
    // can join and leave, so the index is written continuously.
    for lane in 0..2u64 {
        let url = url.clone();
        let stop = stop.clone();
        churn.spawn(async move {
            let mut created = 0u64;
            while !stop.load(Relaxed) {
                let (mut ws, _) = join_at(&url, &format!("churn-{lane}-{created}")).await;
                let _ = ws.close(None).await;
                created += 1;
            }
            created
        });
    }
    // Listing: the admin channel page walks the index while it churns.
    {
        let admin = server.admin;
        let stop = stop.clone();
        churn.spawn(async move {
            let mut pages = 0u64;
            let mut worst = Duration::ZERO;
            while !stop.load(Relaxed) {
                let started = Instant::now();
                let response =
                    http_at(admin, "GET", "/v1/channels?limit=64", Some("admin-test")).await;
                worst = worst.max(started.elapsed());
                assert!(
                    response.starts_with("HTTP/1.1 200 "),
                    "listing failed: {}",
                    response.lines().next().unwrap_or_default()
                );
                let body = response.split_once("\r\n\r\n").unwrap().1;
                let page: serde_json::Value = serde_json::from_str(body).unwrap();
                let rows = page.as_array().expect("a channel page is a JSON array");
                assert!(!rows.is_empty() && rows.len() <= 64);
                pages += 1;
            }
            assert!(
                worst < Duration::from_secs(1),
                "channel listing stalled for {worst:?} during room churn"
            );
            pages
        });
    }
    // A crowd of disconnect observers, larger than one retirement turn, so
    // every departure above becomes a queued job in an observer shard and
    // worker turns keep taking the shards. The observers drain their
    // sockets and count notices.
    const OBSERVERS: usize = 130; // above RETIREMENT_QUANTUM
    let notified = Arc::new(AtomicU64::new(0));
    for _ in 0..OBSERVERS {
        let (mut ws, _) = join_at(&url, "re_globl").await;
        // Global broadcasts already reach this member; skip them while
        // waiting for each reply.
        for (op, value, reply) in [
            (0, protocol::text("host-test"), protocol::reply(0, &[0xf5])),
            (1, protocol::uint(4), protocol::reply(1, &protocol::uint(4))),
        ] {
            let mut packet = command(&mut ws, op, &value).await;
            while packet != reply {
                packet = data(&mut ws).await;
            }
        }
        let notified = notified.clone();
        readers.spawn(async move {
            while let Some(Ok(message)) = ws.next().await {
                match message {
                    Message::Binary(packet) => {
                        if protocol::parse(&packet).is_ok_and(|(sender, _)| sender == BROADCAST) {
                            notified.fetch_add(1, Relaxed);
                        }
                    }
                    Message::Close(_) => break,
                    _ => {}
                }
            }
        });
    }
    // One more observer keeps clearing and restoring its flag: every clear
    // is a barrier scan of the shards, chunk by chunk, racing the
    // departures and turns above.
    {
        let (mut ws, _) = join_at(&url, "re_globl").await;
        assert_eq!(
            command(&mut ws, 0, &protocol::text("host-test")).await,
            protocol::reply(0, &[0xf5])
        );
        let stop = stop.clone();
        churn.spawn(async move {
            let mut toggles = 0u64;
            let mut notices = 0u64;
            while !stop.load(Relaxed) {
                for flags in [4u64, 0] {
                    let reply = protocol::reply(1, &protocol::uint(flags));
                    let mut packet = command(&mut ws, 1, &protocol::uint(flags)).await;
                    while packet != reply {
                        // Global broadcasts reach `re_globl` members too;
                        // only departure notices count.
                        if protocol::parse(&packet).unwrap().0 == BROADCAST {
                            notices += 1;
                        }
                        packet = data(&mut ws).await;
                    }
                    tokio::time::sleep(Duration::from_millis(if flags == 4 { 10 } else { 2 }))
                        .await;
                }
                toggles += 1;
            }
            let _ = ws.close(None).await;
            assert!(notices > 0, "the observer saw no departure notices");
            toggles
        });
    }
    let (mut probe, probe_id) = join_at(&url, "probe").await;
    let end = Instant::now() + Duration::from_millis(2500);
    let mut samples = 0u32;
    let mut worst = Duration::ZERO;
    while Instant::now() < end {
        let started = Instant::now();
        send(&mut probe, probe_id, &[0x18, samples as u8]).await;
        let expected = protocol::envelope(probe_id, &[0x18, samples as u8]);
        // Global broadcasts reach the probe's room too; only the echo counts.
        loop {
            let packet = data(&mut probe).await;
            if packet == expected {
                break;
            }
            assert_ne!(protocol::parse(&packet).unwrap().0, probe_id);
        }
        worst = worst.max(started.elapsed());
        samples += 1;
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    stop.store(true, Relaxed);
    let mut counts = Vec::new();
    while let Some(result) = churn.join_next().await {
        counts.push(result.unwrap());
    }
    eprintln!(
        "probe: {samples} echoes, worst {worst:?}; broadcasts, rooms created per lane, pages listed and observer toggles {counts:?}; frames {}; notices to the observer crowd {}",
        received.load(Relaxed),
        notified.load(Relaxed)
    );
    assert!(samples >= 50);
    assert!(
        worst < Duration::from_secs(1),
        "unrelated echo stalled for {worst:?} during room churn"
    );
    assert!(counts.iter().all(|&count| count > 0));
    assert!(received.load(Relaxed) > 0);
    assert!(
        notified.load(Relaxed) > 0,
        "the observer crowd saw no departures"
    );
    let _ = probe.close(None).await;
    // Churned rooms must all have retired; only the stable rooms, the probe's
    // room and re_globl remain until shutdown clears them.
    assert!(server.relay.counts().1 <= ROOMS + 2);
    server.stop().await;
    readers.abort_all();
    while readers.join_next().await.is_some() {}
}
