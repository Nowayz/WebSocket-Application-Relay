//! Relay behaviour end to end over real WebTransport (UDP, TLS and HTTP/3).
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};

use bytes::Bytes;
use minicbor::{Decoder, Encoder};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};
use tokio_util::sync::CancellationToken;
use war_relay::{
    Config, Relay,
    protocol::{self, BROADCAST},
    webtransport::{read_record, write_record},
};
use wtransport::{
    Connection, Endpoint, Identity, RecvStream, SendStream, VarInt,
    endpoint::{ConnectOptions, endpoint_side},
    error::ConnectionError,
    tls::Sha256Digest,
};

#[path = "relay_integration/channel_listing.rs"]
mod channel_listing;
#[path = "relay_integration/handshake.rs"]
mod handshake;
#[path = "relay_integration/listeners.rs"]
mod listeners;
#[path = "relay_integration/origin_policy.rs"]
mod origin_policy;

type ClientEndpoint = Arc<Endpoint<endpoint_side::Client>>;

fn client_endpoint(
    hash: &Sha256Digest,
    ip: [u8; 4],
    transport: Option<wtransport::quinn::TransportConfig>,
) -> ClientEndpoint {
    let mut config = wtransport::ClientConfig::builder()
        .with_bind_address(SocketAddr::from((ip, 0)))
        .with_server_certificate_hashes([hash.clone()])
        .build();
    if let Some(transport) = transport {
        config
            .quic_config_mut()
            .transport_config(Arc::new(transport));
    }
    Arc::new(Endpoint::client(config).unwrap())
}

struct Server {
    relay: Arc<Relay>,
    endpoint: ClientEndpoint,
    hash: Sha256Digest,
    address: SocketAddr,
    url: String,
    admin: SocketAddr,
    shutdown: CancellationToken,
    task: Option<JoinHandle<anyhow::Result<()>>>,
    admin_task: JoinHandle<()>,
    /// Outlives `serve`, as an embedder's must, so shutdown packets can leave.
    _ingress: war_relay::webtransport::EndpointIngress,
}

impl Server {
    async fn new(mut config: Config) -> Self {
        let identity = Identity::self_signed(["localhost", "127.0.0.1"]).unwrap();
        let hash = identity.certificate_chain().as_slice()[0].hash();
        let transport = wtransport::ServerConfig::builder()
            .with_bind_address("127.0.0.1:0".parse().unwrap())
            .with_custom_transport(identity, war_relay::webtransport::server_transport_config())
            .build();
        config.bind = "127.0.0.1:0".parse().unwrap();
        let (quic, ingress) =
            war_relay::webtransport::EndpointIngress::bind(&config, transport).unwrap();
        let address = quic.local_addr().unwrap();
        let relay = Relay::new(config).unwrap();
        let admin = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let admin_address = admin.local_addr().unwrap();
        let router = war_relay::admin_router(relay.clone());
        let admin_task = tokio::spawn(async move { axum::serve(admin, router).await.unwrap() });
        let shutdown = CancellationToken::new();
        let task = tokio::spawn({
            let (relay, shutdown) = (relay.clone(), shutdown.clone());
            war_relay::webtransport::serve(quic, relay, shutdown)
        });
        Self {
            relay,
            endpoint: client_endpoint(&hash, [127, 0, 0, 1], None),
            hash,
            address,
            url: format!("https://127.0.0.1:{}/war", address.port()),
            admin: admin_address,
            shutdown,
            task: Some(task),
            admin_task,
            _ingress: ingress,
        }
    }

    fn options(&self) -> ConnectOptions {
        ConnectOptions::builder(&self.url).build()
    }

    async fn join(&self, channel: &str) -> Client {
        Client::join(&self.endpoint, self.options(), &protocol::text(channel)).await
    }

    /// An established session whose join record has not been sent.
    async fn open(&self) -> (Connection, SendStream, RecvStream) {
        open(&self.endpoint, self.options()).await
    }

    async fn http(&self, method: &str, path: &str, token: Option<&str>) -> String {
        http(self.admin, method, path, token).await
    }

    async fn stop(mut self) {
        self.shutdown.cancel();
        timeout(Duration::from_secs(3), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let metrics = &self.relay.metrics;
        assert_eq!(self.relay.counts(), (0, 1));
        assert_eq!(metrics.connections.load(Relaxed), 0);
        assert_eq!(metrics.queued_bytes.load(Relaxed), 0);
        assert_eq!(metrics.output_queue_capacity_bytes.load(Relaxed), 0);
        assert_eq!(metrics.read_budget.as_ref().unwrap().reserved(), 0);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.shutdown.cancel();
        self.admin_task.abort();
    }
}

fn config() -> Config {
    Config {
        admin_token: Some("admin-test".into()),
        auth_password: Some("host-test".into()),
        ..Config::default()
    }
}

async fn http(admin: SocketAddr, method: &str, path: &str, token: Option<&str>) -> String {
    let mut socket = tokio::net::TcpStream::connect(admin).await.unwrap();
    let auth = token.map_or(String::new(), |s| format!("Authorization: Bearer {s}\r\n"));
    socket.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(3), socket.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8(bytes).unwrap()
}

async fn eventually(mut condition: impl FnMut() -> bool) {
    timeout(Duration::from_secs(3), async {
        while !condition() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

async fn open(
    endpoint: &ClientEndpoint,
    options: ConnectOptions,
) -> (Connection, SendStream, RecvStream) {
    let connection = timeout(Duration::from_secs(3), endpoint.connect(options))
        .await
        .unwrap()
        .unwrap();
    let (send, receive) = connection.open_bi().await.unwrap().await.unwrap();
    (connection, send, receive)
}

/// Wait for the relay to end `connection`. Shutdown closes QUIC with its code;
/// this client reports a relay session close (policy or error) as a local
/// close without one, so that case returns `None`.
async fn closed(connection: &Connection) -> Option<u64> {
    match timeout(Duration::from_secs(3), connection.closed())
        .await
        .unwrap()
    {
        ConnectionError::ApplicationClosed(close) => Some(close.code().into_inner()),
        ConnectionError::LocallyClosed => None,
        other => panic!("expected the relay to close the session, got {other}"),
    }
}

/// Wire bytes of one length-prefixed record, for writing it in pieces.
fn wire(record: &[u8]) -> Vec<u8> {
    let mut wire = (record.len() as u32).to_be_bytes().to_vec();
    wire.extend_from_slice(record);
    wire
}

struct Client {
    connection: Connection,
    send: SendStream,
    receive: RecvStream,
    id: u64,
}

impl Client {
    async fn join(endpoint: &ClientEndpoint, options: ConnectOptions, join: &[u8]) -> Self {
        let (connection, mut send, mut receive) = open(endpoint, options).await;
        write_record(&mut send, join).await.unwrap();
        let welcome = timeout(Duration::from_secs(3), read_record(&mut receive, 64))
            .await
            .unwrap()
            .unwrap();
        let id = Decoder::new(&welcome).u64().unwrap();
        assert!(id != 0 && id != BROADCAST);
        Self {
            connection,
            send,
            receive,
            id,
        }
    }

    async fn write(&mut self, record: &[u8]) {
        write_record(&mut self.send, record).await.unwrap();
    }

    async fn send(&mut self, target: u64, value: &[u8]) {
        self.write(&protocol::envelope(target, value)).await;
    }

    async fn read(&mut self) -> Bytes {
        timeout(
            Duration::from_secs(3),
            read_record(&mut self.receive, 1 << 20),
        )
        .await
        .unwrap()
        .unwrap()
    }

    /// Nothing arrives for a short while.
    async fn quiet(&mut self) -> bool {
        timeout(
            Duration::from_millis(30),
            read_record(&mut self.receive, 1 << 20),
        )
        .await
        .is_err()
    }

    async fn command(&mut self, op: u8, value: &[u8]) -> Bytes {
        self.write(&protocol::request(op, value)).await;
        self.read().await
    }

    async fn authenticate(&mut self) {
        assert_eq!(
            self.command(0, &protocol::text("host-test")).await,
            protocol::reply(0, &[0xf5])
        );
    }

    async fn closed(&self) -> Option<u64> {
        closed(&self.connection).await
    }

    /// End the session gracefully: an empty record, answered by an empty
    /// record once the relay has removed the registration.
    async fn close(mut self) {
        self.write(&[]).await;
        while !self.read().await.is_empty() {}
        self.connection.close(VarInt::from_u32(0), b"");
    }
}

/// Join many sessions, 32 handshakes at a time.
async fn join_many(server: &Server, channels: impl IntoIterator<Item = String>) -> Vec<Client> {
    let channels: Vec<_> = channels.into_iter().collect();
    let mut clients = Vec::with_capacity(channels.len());
    for chunk in channels.chunks(32) {
        let mut joining = JoinSet::new();
        for channel in chunk {
            let (endpoint, options) = (server.endpoint.clone(), server.options());
            let join = protocol::text(channel);
            joining.spawn(async move { Client::join(&endpoint, options, &join).await });
        }
        clients.extend(joining.join_all().await);
    }
    clients
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn routes_typed_cbor_private_and_channel_messages_without_changing_payload() {
    let server = Server::new(config()).await;
    let mut a = server.join("room").await;
    let mut b = server.join("room").await;
    let mut outsider = server.join("elsewhere").await;
    // {"text": "π", "bytes": h'0001', "items": [true, null, -1]} encoded independently.
    let value = [
        0xa3, 0x64, b't', b'e', b'x', b't', 0x62, 0xcf, 0x80, 0x65, b'b', b'y', b't', b'e', b's',
        0x42, 0, 1, 0x65, b'i', b't', b'e', b'm', b's', 0x83, 0xf5, 0xf6, 0x20,
    ];
    a.send(BROADCAST, &value).await;
    assert_eq!(b.read().await, protocol::envelope(a.id, &value));
    assert!(outsider.quiet().await);
    assert!(a.quiet().await);
    a.send(outsider.id, &protocol::text("across channels"))
        .await;
    assert_eq!(
        outsider.read().await,
        protocol::envelope(a.id, &protocol::text("across channels"))
    );
    a.send(a.id, &[0xf5]).await;
    assert_eq!(a.read().await, protocol::envelope(a.id, &[0xf5]));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn partial_record_input_leaves_direct_output_flowing_and_ordered() {
    let server = Server::new(config()).await;
    let mut sender = server.join("partial").await;
    let mut recipient = server.join("partial").await;
    let value = protocol::byte_string(&vec![0x5a; 70_000]);
    let record = wire(&protocol::envelope(recipient.id, &value));
    // Split the length prefix itself, then the routing header and value. The
    // reader must retain both while this same session receives other output.
    sender.send.write_all(&record[..2]).await.unwrap();
    let during = protocol::text("while input is partial");
    recipient.send(sender.id, &during).await;
    assert_eq!(
        sender.read().await,
        protocol::envelope(recipient.id, &during)
    );
    sender.send.write_all(&record[2..21]).await.unwrap();
    sender.send.write_all(&record[21..]).await.unwrap();
    assert_eq!(
        recipient.read().await,
        protocol::envelope(sender.id, &value)
    );
    let after = protocol::text("after partial");
    sender.send(recipient.id, &after).await;
    assert_eq!(
        recipient.read().await,
        protocol::envelope(sender.id, &after)
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn global_listeners_and_global_broadcast_preserve_global_scope() {
    let server = Server::new(config()).await;
    let mut global = server.join("re_globl").await;
    let mut a = server.join("alpha").await;
    let mut b = server.join("beta").await;
    global.authenticate().await;
    assert_eq!(global.command(1, &[7]).await, protocol::reply(1, &[7]));
    a.send(BROADCAST, &[1]).await;
    assert_eq!(global.read().await, protocol::envelope(a.id, &[1]));
    a.send(b.id, &[2]).await;
    assert_eq!(b.read().await, protocol::envelope(a.id, &[2]));
    assert_eq!(global.read().await, protocol::envelope(a.id, &[2]));
    global.send(BROADCAST, &[3]).await;
    assert_eq!(a.read().await, protocol::envelope(global.id, &[3]));
    assert_eq!(b.read().await, protocol::envelope(global.id, &[3]));
    let departed = a.id;
    a.close().await;
    assert_eq!(
        global.read().await,
        protocol::envelope(BROADCAST, &protocol::uint(departed))
    );
    assert!(b.quiet().await);
    server.stop().await;
}

#[tokio::test]
async fn variables_are_typed_scoped_and_deleted_when_last_member_leaves() {
    let server = Server::new(config()).await;
    let mut a = server.join("vars").await;
    let mut other = server.join("other").await;
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
        a.command(4, &set.into_writer()).await,
        protocol::reply(4, &[0xf5])
    );
    assert_eq!(
        a.command(5, &protocol::text("key")).await,
        protocol::reply(200, &[0x82, 0xf5, 0xa1, 0x61, b'x', 0x18, 42])
    );
    assert_eq!(
        other.command(5, &protocol::text("key")).await,
        protocol::reply(200, &[0x82, 0xf4, 0xf6])
    );
    a.close().await;
    eventually(|| {
        !server
            .relay
            .channels()
            .iter()
            .any(|c| c.name.as_deref() == Some("vars"))
    })
    .await;
    assert_eq!(server.relay.metrics.variable_bytes.load(Relaxed), 0);
    let mut new = server.join("vars").await;
    assert_eq!(
        new.command(5, &protocol::text("key")).await,
        protocol::reply(200, &[0x82, 0xf4, 0xf6])
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fixed_id_takeover_disconnects_old_session_and_keeps_new_routing() {
    let server = Server::new(config()).await;
    let mut first = server.join("one").await;
    let mut next = server.join("two").await;
    let mut sender = server.join("three").await;
    first.authenticate().await;
    next.authenticate().await;
    assert_eq!(
        first.command(3, &protocol::uint(42)).await,
        protocol::reply(3, &protocol::uint(42))
    );
    assert_eq!(
        next.command(3, &protocol::uint(42)).await,
        protocol::reply(3, &protocol::uint(42))
    );
    first.closed().await;
    sender.send(42, &[0xf5]).await;
    assert_eq!(next.read().await, protocol::envelope(sender.id, &[0xf5]));
    // ID 0 is reserved; requesting it is a policy violation.
    next.write(&protocol::request(3, &protocol::uint(0))).await;
    next.closed().await;
    eventually(|| server.relay.counts() == (1, 2)).await;
    server.stop().await;
}

#[tokio::test]
async fn authentication_channel_queries_and_admin_drain_are_enforced() {
    let server = Server::new(config()).await;
    let mut client = server.join("room").await;
    assert_eq!(
        client.command(0, &protocol::text("wrong")).await,
        protocol::reply(0, &[0xf4])
    );
    client.authenticate().await;
    let reply = client.command(2, &[0xf6]).await;
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
    for (method, path) in [("POST", "/v1/drain"), ("GET", "/metrics")] {
        for token in [None, Some("wrong")] {
            let response = server.http(method, path, token).await;
            assert!(response.starts_with("HTTP/1.1 401"), "{path}");
        }
    }
    assert!(
        server
            .http("GET", "/metrics", Some("admin-test"))
            .await
            .contains("war_sessions 1\n")
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
    assert!(
        timeout(
            Duration::from_secs(3),
            server.endpoint.connect(server.options())
        )
        .await
        .unwrap()
        .is_err()
    );
    // An existing session still routes while admission is drained.
    let id = Decoder::new(&client.command(3, &protocol::uint(123)).await)
        .array()
        .unwrap();
    assert_eq!(id, Some(2));
    client.send(123, &[1]).await;
    assert_eq!(client.read().await, protocol::envelope(123, &[1]));
    assert!(
        server
            .http("POST", "/v1/resume", Some("admin-test"))
            .await
            .starts_with("HTTP/1.1 204")
    );
    let second = server.join("room").await;
    let path = format!("/v1/sessions/{}/disconnect", second.id);
    assert!(
        server
            .http("POST", &path, Some("admin-test"))
            .await
            .starts_with("HTTP/1.1 204")
    );
    second.closed().await;
    assert_eq!(
        client.read().await,
        protocol::envelope(BROADCAST, &protocol::uint(second.id))
    );
    assert!(
        server
            .http("POST", &path, Some("admin-test"))
            .await
            .starts_with("HTTP/1.1 404")
    );
    server.stop().await;
}

#[tokio::test]
async fn read_admission_refuses_announced_records_before_and_after_join_and_keeps_routing() {
    let server = Server::new(Config {
        total_read_buffer_bytes: 100_000,
        ..config()
    })
    .await;
    let mut healthy = server.join("budget-good").await;
    for joined in [false, true] {
        let (connection, mut send, _receive) = if joined {
            let client = server.join("budget-bad").await;
            (client.connection, client.send, client.receive)
        } else {
            server.open().await
        };
        // The header alone needs more staging (twice its size) than the budget.
        send.write_all(&65536u32.to_be_bytes()).await.unwrap();
        closed(&connection).await;
        healthy.send(healthy.id, &[0x18, 42]).await;
        assert_eq!(
            healthy.read().await,
            protocol::envelope(healthy.id, &[0x18, 42])
        );
    }
    let budget = server.relay.metrics.read_budget.as_ref().unwrap();
    assert_eq!(budget.rejected(), 2);
    assert!(budget.peak() <= 100_000);
    let text = server.http("GET", "/metrics", Some("admin-test")).await;
    assert!(text.contains("war_read_buffer_reservations_rejected_total 2\n"));
    assert!(text.contains("war_read_buffer_limit_bytes 100000\n"));
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 2);
    server.stop().await;
}

#[tokio::test]
async fn invalid_joins_close_without_joining_and_shutdown_closes_pending_sessions() {
    let server = Server::new(Config {
        max_message_bytes: 1024,
        ..config()
    })
    .await;
    for record in [
        wire(&[0xff]),
        wire(&protocol::text("seventeen-bytes!!")),
        wire(&protocol::uint(7)),
        wire(&[0x61, b'a', 0x61, b'b']),
        // The announced join exceeds the message limit.
        1025u32.to_be_bytes().to_vec(),
    ] {
        let (connection, mut send, _receive) = server.open().await;
        send.write_all(&record).await.unwrap();
        closed(&connection).await;
    }
    eventually(|| server.relay.metrics.connections.load(Relaxed) == 0).await;
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 0);
    // Shutdown also ends a session that holds an incomplete join and one
    // that holds an incomplete application record.
    let (pending, mut send, _receive) = server.open().await;
    send.write_all(&[0, 0, 0, 8, 0x67]).await.unwrap();
    let mut joined = server.join("shutdown-held").await;
    joined.send.write_all(&[0, 0, 1, 0, 0x82]).await.unwrap();
    eventually(|| {
        server
            .relay
            .metrics
            .read_budget
            .as_ref()
            .unwrap()
            .reserved()
            == 16 + 512
    })
    .await;
    server.shutdown.cancel();
    assert_eq!(closed(&pending).await, Some(1001));
    assert_eq!(joined.closed().await, Some(1001));
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 1);
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
    let mut healthy = server.join("deadline-health").await;
    let (pending, mut send, _receive) = server.open().await;
    send.write_all(&[0, 0, 0, 8, 0x67]).await.unwrap();
    let budget = server.relay.metrics.read_budget.as_ref().unwrap();
    eventually(|| budget.reserved() == 16).await;
    let started = Instant::now();
    let (_, echoes) = tokio::join!(closed(&pending), async {
        let mut echoes = 0;
        while started.elapsed() < Duration::from_millis(1200) {
            healthy.send(healthy.id, &[0x18, 42]).await;
            assert_eq!(
                healthy.read().await,
                protocol::envelope(healthy.id, &[0x18, 42])
            );
            echoes += 1;
            sleep(Duration::from_millis(15)).await;
        }
        echoes
    });
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert!(echoes > 20, "healthy routing stalled: {echoes} echoes");
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 1);
    eventually(|| server.relay.metrics.connections.load(Relaxed) == 1).await;
    assert_eq!(budget.reserved(), 0);
    server.stop().await;
}

#[tokio::test]
async fn admin_cancellation_retires_membership_while_a_record_is_incomplete() {
    let server = Server::new(config()).await;
    let mut healthy = server.join("cancel-health").await;
    let mut source = server.join("cancel-held").await;
    source.send.write_all(&[0, 0, 0x10, 0, 0x82]).await.unwrap();
    let budget = server.relay.metrics.read_budget.as_ref().unwrap();
    eventually(|| budget.reserved() == 2 * 0x1000).await;
    let path = format!("/v1/sessions/{}/disconnect", source.id);
    assert!(
        server
            .http("POST", &path, Some("admin-test"))
            .await
            .starts_with("HTTP/1.1 204")
    );
    source.closed().await;
    eventually(|| server.relay.counts() == (1, 2)).await;
    assert_eq!(server.relay.metrics.disconnected.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.queued_bytes.load(Relaxed), 0);
    eventually(|| budget.reserved() == 0).await;
    healthy.send(healthy.id, &[0x18, 42]).await;
    assert_eq!(
        healthy.read().await,
        protocol::envelope(healthy.id, &[0x18, 42])
    );
    server.stop().await;
}

#[tokio::test]
async fn aborting_the_embedding_service_releases_sessions_without_disconnect_fanout() {
    let mut server = Server::new(config()).await;
    let _clients = join_many(&server, (0..64).map(|_| "abort-group".into())).await;
    let _pending = server.open().await;
    eventually(|| server.relay.metrics.connections.load(Relaxed) == 65).await;
    let delivered = server.relay.metrics.deliveries.load(Relaxed);
    let task = server.task.take().unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    eventually(|| server.relay.metrics.connections.load(Relaxed) == 0).await;
    assert_eq!(server.relay.counts(), (0, 1));
    assert_eq!(server.relay.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.disconnected.load(Relaxed), 64);
    assert_eq!(server.relay.metrics.deliveries.load(Relaxed), delivered);
}

#[tokio::test]
async fn aborting_the_embedding_service_stops_admission_and_closes_peers() {
    let mut server = Server::new(config()).await;
    let client = server.join("abort").await;
    let task = server.task.take().unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert!(server.relay.is_draining());
    client.closed().await;
}

#[tokio::test]
async fn malformed_packets_and_unauthorized_commands_close_the_session() {
    let server = Server::new(config()).await;
    for record in [
        // A truncated envelope, then trailing data after a valid one.
        vec![0x82, 1, 0xff],
        [&protocol::envelope(1, &[0xf6])[..], &[0]].concat(),
        // Channel listing without authentication, an unknown op, and SET
        // without a value.
        protocol::request(2, &[0xf6]).to_vec(),
        protocol::request(255, &[0xf6]).to_vec(),
        protocol::request(4, &[0x82, 0x61, b'x']).to_vec(),
    ] {
        let mut client = server.join("malformed").await;
        client.write(&record).await;
        client.closed().await;
        eventually(|| server.relay.counts() == (0, 1)).await;
    }
    assert_eq!(server.relay.metrics.disconnected.load(Relaxed), 5);
    server.stop().await;
}

#[tokio::test]
async fn reserved_simple_encodings_never_reach_recipients_or_channel_variables() {
    let server = Server::new(Config { ..config() }).await;
    let mut healthy = server.join("healthy-cbor").await;
    for value in 0..32u8 {
        for prefix in [&[][..], &[0x81], &[0xa1, 0], &[0xc0], &[0xa1]] {
            let mut bad = server.join("invalid-cbor").await;
            let mut encoded = prefix.to_vec();
            encoded.extend_from_slice(&[0xf8, value]);
            if prefix == [0xa1] {
                encoded.push(0xf6);
            }
            bad.send(healthy.id, &encoded).await;
            timeout(Duration::from_secs(2), async {
                tokio::select! {
                    _ = bad.connection.closed() => {}
                    incoming = read_record(&mut healthy.receive, 1 << 20) => {
                        panic!("malformed value reached a healthy recipient: {incoming:?}")
                    }
                }
            })
            .await
            .unwrap();
            let probe = protocol::uint(u64::from(value));
            healthy.send(healthy.id, &probe).await;
            assert_eq!(healthy.read().await, protocol::envelope(healthy.id, &probe));
        }
        let mut bad = server.join("healthy-cbor").await;
        // [0, [4, ["key", simple(value)]]]; a command must not store it either.
        bad.write(&[0x82, 0, 0x82, 4, 0x82, 0x63, b'k', b'e', b'y', 0xf8, value])
            .await;
        bad.closed().await;
        assert_eq!(server.relay.metrics.variable_bytes.load(Relaxed), 0);
        // The bad peer shared this channel, so its departure is observable.
        assert_eq!(
            healthy.read().await,
            protocol::envelope(BROADCAST, &protocol::uint(bad.id))
        );
    }
    let mut valid = server.join("valid-cbor").await;
    for value in (32..=255u8)
        .map(|value| vec![0xf8, value])
        .chain((0xe0..=0xf7).map(|value| vec![value]))
    {
        valid.send(healthy.id, &value).await;
        assert_eq!(healthy.read().await, protocol::envelope(valid.id, &value));
    }
    assert_eq!(server.relay.metrics.disconnected.load(Relaxed), 32 * 6);
    healthy.close().await;
    valid.close().await;
    server.stop().await;
}

#[tokio::test]
async fn application_input_and_receive_only_output_extend_idle_policy() {
    let server = Server::new(Config {
        idle_timeout_secs: 1,
        ..config()
    })
    .await;
    let mut sender = server.join("idle-source").await;
    let mut receiver = server.join("idle-receiver").await;
    assert_eq!(
        sender.command(4, &[0x82, 0x61, b'x', 1]).await,
        protocol::reply(4, &[0xf5])
    );
    assert!(server.relay.metrics.variable_bytes.load(Relaxed) > 0);
    // The sender receives no application replies during this sequence. The
    // recipient sends no application input. Both stay active beyond one timeout.
    for sequence in 1..=5 {
        sleep(Duration::from_millis(350)).await;
        let value = protocol::uint(sequence);
        sender.send(receiver.id, &value).await;
        assert_eq!(receiver.read().await, protocol::envelope(sender.id, &value));
    }
    assert_eq!(server.relay.metrics.idle_timeouts.load(Relaxed), 0);
    tokio::join!(sender.closed(), receiver.closed());
    assert_eq!(server.relay.metrics.idle_timeouts.load(Relaxed), 2);
    eventually(|| server.relay.metrics.variable_bytes.load(Relaxed) == 0).await;
    server.stop().await;
}

#[tokio::test]
async fn unfinished_record_input_does_not_extend_idle_policy() {
    let server = Server::new(Config {
        idle_timeout_secs: 1,
        ..config()
    })
    .await;
    let mut client = server.join("idle-partial").await;
    let record = wire(&protocol::envelope(
        client.id,
        &protocol::byte_string(&vec![0; 64 * 1024]),
    ));
    // The CBOR bytes are well-formed, but the record never finishes.
    let started = Instant::now();
    for chunk in record[..32 * 1024].chunks(4096) {
        client.send.write_all(chunk).await.unwrap();
        sleep(Duration::from_millis(50)).await;
    }
    client.closed().await;
    assert!(started.elapsed() < Duration::from_millis(1500));
    assert_eq!(server.relay.metrics.idle_timeouts.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.incoming_messages.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn variable_and_message_size_limits_are_real_network_limits() {
    let server = Server::new(Config {
        max_message_bytes: 128,
        queue_bytes: 256,
        max_variable_bytes: 4,
        ..config()
    })
    .await;
    let mut global = server.join("re_globl").await;
    let mut set = Encoder::new(Vec::new());
    set.array(2)
        .unwrap()
        .str("key")
        .unwrap()
        .str("too big")
        .unwrap();
    global
        .write(&protocol::request(4, &set.into_writer()))
        .await;
    global.closed().await;
    assert_eq!(server.relay.metrics.variable_bytes.load(Relaxed), 0);
    let mut oversized = server.join("re_globl").await;
    oversized
        .send(BROADCAST, &protocol::byte_string(&[0; 200]))
        .await;
    oversized.closed().await;
    server.stop().await;
}

#[tokio::test]
async fn message_limit_admits_an_exact_size_record_and_closes_one_byte_over() {
    let server = Server::new(Config {
        max_message_bytes: 17,
        ..config()
    })
    .await;
    let mut sender = server.join("data-limit").await;
    let mut reader = server.join("data-limit").await;
    let value = protocol::byte_string(&[0x5a; 6]);
    let allowed = protocol::envelope(BROADCAST, &value);
    assert_eq!(allowed.len(), 17);
    sender.write(&allowed).await;
    assert_eq!(reader.read().await, protocol::envelope(sender.id, &value));
    let oversized = protocol::envelope(BROADCAST, &protocol::byte_string(&[0x5a; 7]));
    assert_eq!(oversized.len(), 18);
    sender.write(&oversized).await;
    sender.closed().await;
    assert_eq!(
        reader.read().await,
        protocol::envelope(BROADCAST, &protocol::uint(sender.id))
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_of_a_large_channel_does_not_generate_disconnect_fanout() {
    let server = Server::new(config()).await;
    let mut clients = join_many(&server, (0..100).map(|_| "shutdown".into())).await;
    eventually(|| server.relay.counts() == (100, 2)).await;
    server.shutdown.cancel();
    for client in &mut clients {
        assert_eq!(client.closed().await, Some(1001));
        // No departure notice precedes the shutdown close.
        let next = timeout(
            Duration::from_secs(3),
            read_record(&mut client.receive, 1024),
        )
        .await
        .unwrap();
        assert!(next.is_err(), "unexpected record: {next:?}");
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
    let _members = join_many(&server, (0..300).map(|i| format!("c{i:015}"))).await;
    let mut host = server.join("host").await;
    host.authenticate().await;
    let reply = host.command(2, &[0xf6]).await;
    assert!(reply.len() > 128 + 4096);
    let (_, value) = protocol::parse(&reply).unwrap();
    let mut d = Decoder::new(value);
    assert_eq!(d.array().unwrap(), Some(2));
    assert_eq!(d.u8().unwrap(), 2);
    assert_eq!(d.array().unwrap(), Some(302));
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn simultaneous_graceful_closes_finish_during_notification_pressure() {
    let server = Server::new(Config {
        queue_messages: 4,
        ..config()
    })
    .await;
    let clients = join_many(&server, (0..128).map(|_| "close-burst".into())).await;
    let gate = Arc::new(tokio::sync::Barrier::new(clients.len() + 1));
    let mut closing = JoinSet::new();
    for mut client in clients {
        let gate = gate.clone();
        closing.spawn(async move {
            gate.wait().await;
            // Departure notices may precede the acknowledgement, or the relay
            // may close a peer whose small queue overflowed first.
            if write_record(&mut client.send, &[]).await.is_err() {
                client.closed().await;
                return false;
            }
            loop {
                match timeout(
                    Duration::from_secs(3),
                    read_record(&mut client.receive, 1024),
                )
                .await
                .unwrap()
                {
                    Ok(record) if record.is_empty() => break true,
                    Ok(record) => assert_eq!(protocol::parse(&record).unwrap().0, BROADCAST),
                    Err(_) => {
                        client.closed().await;
                        break false;
                    }
                }
            }
        });
    }
    gate.wait().await;
    let acknowledged = closing.join_all().await.into_iter().filter(|a| *a).count();
    assert!(acknowledged > 0);
    eventually(|| server.relay.counts() == (0, 1)).await;
    assert_eq!(server.relay.metrics.disconnected.load(Relaxed), 128);
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn private_routing_keeps_progress_through_registry_growth_and_retirement() {
    const WAVE: usize = 1100;
    let server = Server::new(Config { ..config() }).await;
    let mut sender = server.join("registry-health").await;
    let mut receiver = server.join("registry-health").await;
    let finished = CancellationToken::new();
    let phase = AtomicU64::new(0);
    let population = async {
        let clients = join_many(&server, (0..WAVE).map(|i| format!("wave-{i}"))).await;
        assert_eq!(server.relay.counts(), (WAVE + 2, WAVE + 2));
        phase.store(1, Relaxed);
        let mut clients = clients.into_iter();
        loop {
            let mut closing = JoinSet::new();
            for client in clients.by_ref().take(32) {
                closing.spawn(client.close());
            }
            if closing.is_empty() {
                break;
            }
            closing.join_all().await;
        }
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
            sender.send(receiver.id, &value).await;
            assert_eq!(receiver.read().await, protocol::envelope(sender.id, &value));
            counts[stage] += 1;
            sleep(Duration::from_millis(1)).await;
        }
        counts
    };
    let (_, progress) = tokio::join!(population, healthy);
    assert!(
        progress[0] > 0 && progress[1] > 0,
        "no progress in phase: {progress:?}"
    );
    eventually(|| server.relay.counts() == (2, 2)).await;
    let metrics = &server.relay.metrics;
    assert_eq!(metrics.joined.load(Relaxed), WAVE as u64 + 2);
    assert_eq!(metrics.disconnected.load(Relaxed), WAVE as u64);
    // Both original peers remain usable after all retirement thresholds.
    sender.send(receiver.id, &[0xa1, 0x61, b'x', 0xf5]).await;
    assert_eq!(
        receiver.read().await,
        protocol::envelope(sender.id, &[0xa1, 0x61, b'x', 0xf5])
    );
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recipient_that_stops_reading_is_closed_as_a_slow_consumer_and_the_sender_survives() {
    let server = Server::new(Config {
        max_message_bytes: 64 * 1024,
        queue_bytes: 256 * 1024,
        total_queue_bytes: 16 * 1024 * 1024,
        write_timeout_secs: 5,
        ..config()
    })
    .await;
    let mut sender = server.join("slow-consumer").await;
    // A small stream window makes the relay's QUIC output fill quickly.
    let mut transport = wtransport::quinn::TransportConfig::default();
    transport.stream_receive_window(wtransport::quinn::VarInt::from_u32(64 * 1024));
    let slow_endpoint = client_endpoint(&server.hash, [127, 0, 0, 1], Some(transport));
    let slow = Client::join(
        &slow_endpoint,
        server.options(),
        &protocol::text("slow-consumer"),
    )
    .await;
    // Read the sender's next echo, skipping the slow peer's departure notice,
    // which may arrive at any point once it is closed.
    async fn echo(client: &mut Client, value: &[u8]) {
        let expected = protocol::envelope(client.id, value);
        let departure = protocol::envelope(BROADCAST, &[]);
        loop {
            let packet = client.read().await;
            if packet == expected {
                return;
            }
            assert!(
                packet.starts_with(&departure[..departure.len() - 1]),
                "unexpected packet {packet:?}"
            );
        }
    }
    // The slow peer never reads. Its QUIC windows fill first, then its
    // per-session output allowance; the relay must close it, not the sender.
    let value = protocol::byte_string(&vec![0x5a; 32 * 1024]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while server.relay.metrics.slow_consumers.load(Relaxed) == 0 {
        assert!(Instant::now() < deadline, "slow consumer was never closed");
        sender.send(slow.id, &value).await;
        // Routing stays responsive for the sender throughout.
        sender.send(sender.id, &[7]).await;
        echo(&mut sender, &[7]).await;
    }
    assert_eq!(server.relay.metrics.slow_consumers.load(Relaxed), 1);
    slow.closed().await;
    eventually(|| server.relay.metrics.disconnected.load(Relaxed) == 1).await;
    // The sender was never penalized for its peer's slowness.
    sender.send(sender.id, &[8]).await;
    echo(&mut sender, &[8]).await;
    sender.close().await;
    server.stop().await;
}

/// Send a command and read until its reply, counting departure notices (from
/// BROADCAST) and other data in between.
async fn command_amid_traffic(
    client: &mut Client,
    op: u8,
    value: &[u8],
    reply: &[u8],
) -> (u64, u64) {
    let reply = protocol::reply(op, reply);
    let (mut notices, mut other) = (0, 0);
    let mut packet = client.command(op, value).await;
    while packet != reply {
        match protocol::parse(&packet).unwrap().0 {
            BROADCAST => notices += 1,
            0 => panic!("unexpected control reply {packet:?}"),
            _ => other += 1,
        }
        packet = client.read().await;
    }
    (notices, other)
}

/// Spawn a reader that drains `client` and counts the records matching `count`.
fn drain(
    readers: &mut JoinSet<()>,
    mut client: Client,
    counter: Arc<AtomicU64>,
    count: fn(&[u8]) -> bool,
) {
    readers.spawn(async move {
        while let Ok(record) = read_record(&mut client.receive, 1 << 20).await {
            if count(&record) {
                counter.fetch_add(1, Relaxed);
            }
        }
    });
}

/// Probe a session in its own room for `duration`; return samples and the
/// worst echo latency. Global broadcasts reach the probe too; only echoes count.
async fn probe(server: &Server, duration: Duration) -> (u32, Duration) {
    let mut probe = server.join("probe").await;
    let end = Instant::now() + duration;
    let mut samples = 0u32;
    let mut worst = Duration::ZERO;
    while Instant::now() < end {
        let started = Instant::now();
        let value = [0x18, samples as u8];
        probe.send(probe.id, &value).await;
        let expected = protocol::envelope(probe.id, &value);
        loop {
            let packet = probe.read().await;
            if packet == expected {
                break;
            }
            assert_ne!(protocol::parse(&packet).unwrap().0, probe.id);
        }
        worst = worst.max(started.elapsed());
        samples += 1;
        sleep(Duration::from_millis(10)).await;
    }
    probe.connection.close(VarInt::from_u32(0), b"");
    (samples, worst)
}

/// Nothing may broadly stall other work. While one large room takes a stream
/// of broadcasts above the live-fanout quantum, members join and leave it so
/// departure notices become deferred jobs, and a global observer keeps
/// revoking and restoring its disconnect flag so barriers scan the observer
/// queue, a session in an unrelated room must keep echoing with bounded
/// latency, and shutdown must still finish inside the fixture's deadline.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unrelated_traffic_stays_responsive_under_large_room_churn_and_revocation() {
    const MEMBERS: usize = 1100; // above FANOUT_QUANTUM and RETIREMENT_QUANTUM
    let server = Server::new(config()).await;
    let received = Arc::new(AtomicU64::new(0));
    let mut readers = JoinSet::new();
    for client in join_many(&server, (0..MEMBERS).map(|_| "crowd".into())).await {
        drain(&mut readers, client, received.clone(), |_| true);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut churn = JoinSet::new();
    // Broadcaster: a steady stream of room broadcasts, each a continuation.
    {
        let mut client = server.join("crowd").await;
        let stop = stop.clone();
        churn.spawn(async move {
            let value = protocol::byte_string(&vec![0x5a; 1024]);
            let mut sent = 0u64;
            while !stop.load(Relaxed) {
                client.send(BROADCAST, &value).await;
                sent += 1;
                sleep(Duration::from_millis(20)).await;
            }
            client.connection.close(VarInt::from_u32(0), b"");
            sent
        });
    }
    // Join and leave churn: every departure notifies the whole crowd.
    {
        let (endpoint, url) = (server.endpoint.clone(), server.url.clone());
        let stop = stop.clone();
        churn.spawn(async move {
            let mut departures = 0u64;
            while !stop.load(Relaxed) {
                let options = ConnectOptions::builder(&url).build();
                Client::join(&endpoint, options, &protocol::text("crowd"))
                    .await
                    .close()
                    .await;
                departures += 1;
                sleep(Duration::from_millis(40)).await;
            }
            departures
        });
    }
    // Observer revocation: every clear publishes a new snapshot and scans
    // the observer shards and the publication index for pending notices;
    // with both bits set, broadcasts are captured as well as departures.
    {
        let mut client = server.join("re_globl").await;
        client.authenticate().await;
        let stop = stop.clone();
        churn.spawn(async move {
            let (mut toggles, mut notices, mut captured) = (0u64, 0, 0);
            while !stop.load(Relaxed) {
                for flags in [5u64, 0] {
                    let (n, c) = command_amid_traffic(
                        &mut client,
                        1,
                        &protocol::uint(flags),
                        &protocol::uint(flags),
                    )
                    .await;
                    notices += n;
                    captured += c;
                    // Keep the flags set long enough for departures and
                    // broadcasts to land, so each revocation has work to flush.
                    sleep(Duration::from_millis(if flags == 5 { 30 } else { 5 })).await;
                }
                toggles += 1;
            }
            client.connection.close(VarInt::from_u32(0), b"");
            assert!(notices > 0, "the observer saw no departure notices");
            assert!(captured > 0, "the observer captured no broadcasts");
            toggles
        });
    }
    let (samples, worst) = probe(&server, Duration::from_millis(1500)).await;
    stop.store(true, Relaxed);
    let counts = churn.join_all().await;
    eprintln!("probe: {samples} echoes, worst {worst:?}; churn counts {counts:?}");
    assert!(samples >= 50, "probe produced too few samples: {samples}");
    assert!(
        worst < Duration::from_secs(1),
        "unrelated echo stalled for {worst:?} during large-room churn"
    );
    assert!(counts.iter().all(|&count| count > 0));
    assert!(received.load(Relaxed) > 0);
    // Shutdown with the crowd still connected must finish inside the
    // fixture's three-second bound and leave every gauge at zero.
    server.stop().await;
    readers.abort_all();
}

/// The other stall-prone path: room creation and retirement write the
/// channel index, every membership change invalidates the global recipient
/// view, which the next global broadcast rebuilds from every room, and the
/// admin channel listing walks the whole index; with an observer crowd larger
/// than one turn, every departure is a queued observer job, worker turns keep
/// taking the shards, and every revocation is a chunked barrier scan racing
/// them. Under continuous room churn and a stream of global broadcasts, a
/// session in a stable room must keep echoing and the listing must keep
/// answering, both with bounded latency.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unrelated_traffic_stays_responsive_under_room_churn_and_global_broadcasts() {
    const ROOMS: usize = 150;
    const PER_ROOM: usize = 4;
    const OBSERVERS: usize = 130; // above RETIREMENT_QUANTUM
    let server = Server::new(config()).await;
    let received = Arc::new(AtomicU64::new(0));
    let mut readers = JoinSet::new();
    let rooms = (0..ROOMS * PER_ROOM).map(|i| format!("r{}", i % ROOMS));
    for client in join_many(&server, rooms).await {
        drain(&mut readers, client, received.clone(), |_| true);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut churn = JoinSet::new();
    // Global broadcaster: each broadcast rebuilds the invalidated global view.
    {
        let mut client = server.join("re_globl").await;
        let stop = stop.clone();
        churn.spawn(async move {
            let value = protocol::byte_string(&vec![0x5a; 256]);
            let mut sent = 0u64;
            while !stop.load(Relaxed) {
                client.send(BROADCAST, &value).await;
                sent += 1;
                sleep(Duration::from_millis(15)).await;
            }
            client.connection.close(VarInt::from_u32(0), b"");
            sent
        });
    }
    // Room churn: fresh rooms are created and retired as fast as the client
    // can join and leave, so the index is written continuously.
    for lane in 0..2u64 {
        let (endpoint, url) = (server.endpoint.clone(), server.url.clone());
        let stop = stop.clone();
        churn.spawn(async move {
            let mut created = 0u64;
            while !stop.load(Relaxed) {
                let options = ConnectOptions::builder(&url).build();
                let join = protocol::text(&format!("churn-{lane}-{created}"));
                Client::join(&endpoint, options, &join).await.close().await;
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
                    http(admin, "GET", "/v1/channels?limit=64", Some("admin-test")).await;
                worst = worst.max(started.elapsed());
                assert!(response.starts_with("HTTP/1.1 200 "), "{response}");
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
    // every departure above becomes a queued job in an observer shard.
    let notified = Arc::new(AtomicU64::new(0));
    let observers = join_many(&server, (0..OBSERVERS).map(|_| "re_globl".into())).await;
    for mut client in observers {
        // Global broadcasts already reach this member; skip them.
        command_amid_traffic(&mut client, 0, &protocol::text("host-test"), &[0xf5]).await;
        let flags = protocol::uint(4);
        command_amid_traffic(&mut client, 1, &flags, &flags).await;
        drain(&mut readers, client, notified.clone(), |record| {
            protocol::parse(record).is_ok_and(|(sender, _)| sender == BROADCAST)
        });
    }
    // One more observer keeps clearing and restoring its flag: every clear
    // is a barrier scan of the shards, chunk by chunk, racing the
    // departures and turns above.
    {
        let mut client = server.join("re_globl").await;
        command_amid_traffic(&mut client, 0, &protocol::text("host-test"), &[0xf5]).await;
        let stop = stop.clone();
        churn.spawn(async move {
            let (mut toggles, mut notices) = (0u64, 0);
            while !stop.load(Relaxed) {
                for (flags, pause) in [(4, 10), (0, 2)] {
                    let flags = protocol::uint(flags);
                    notices += command_amid_traffic(&mut client, 1, &flags, &flags).await.0;
                    sleep(Duration::from_millis(pause)).await;
                }
                toggles += 1;
            }
            client.connection.close(VarInt::from_u32(0), b"");
            assert!(notices > 0, "the observer saw no departure notices");
            toggles
        });
    }
    let (samples, worst) = probe(&server, Duration::from_millis(1500)).await;
    stop.store(true, Relaxed);
    let counts = churn.join_all().await;
    eprintln!("probe: {samples} echoes, worst {worst:?}; churn counts {counts:?}");
    assert!(samples >= 50, "probe produced too few samples: {samples}");
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
    // Churned rooms must all have retired; only the stable rooms, the probe's
    // room and re_globl can remain until shutdown clears them.
    eventually(|| server.relay.counts().1 <= ROOMS + 2).await;
    server.stop().await;
    readers.abort_all();
}
