//! Real UDP/TLS/HTTP3 integration; no transport mocks.
use bytes::Bytes;
use std::{sync::Arc, time::Duration};
use tokio_util::sync::CancellationToken;
use war_relay::{
    Config, Relay,
    webtransport::{read_record, write_record},
};
use wtransport::{Connection, Endpoint, Identity, RecvStream, SendStream, VarInt};

struct Client {
    connection: Connection,
    send: SendStream,
    receive: RecvStream,
    id: u64,
}
impl Client {
    async fn open(
        endpoint: &Endpoint<wtransport::endpoint::endpoint_side::Client>,
        url: &str,
        room: &str,
    ) -> Self {
        let connection = endpoint.connect(url).await.unwrap();
        let (mut send, mut receive) = connection.open_bi().await.unwrap().await.unwrap();
        let mut join = Vec::new();
        minicbor::Encoder::new(&mut join).str(room).unwrap();
        write_record(&mut send, &join).await.unwrap();
        let welcome = read_record(&mut receive, 1024).await.unwrap();
        let id = minicbor::Decoder::new(&welcome).u64().unwrap();
        Self {
            connection,
            send,
            receive,
            id,
        }
    }
    async fn read(&mut self) -> Bytes {
        tokio::time::timeout(
            Duration::from_secs(3),
            read_record(&mut self.receive, 1 << 20),
        )
        .await
        .unwrap()
        .unwrap()
    }
}
fn route(target: u64, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut e = minicbor::Encoder::new(&mut bytes);
    e.array(2)
        .unwrap()
        .u64(target)
        .unwrap()
        .bytes(payload)
        .unwrap();
    bytes
}
fn verify(bytes: &[u8], sender: u64, payload: &[u8]) {
    let mut d = minicbor::Decoder::new(bytes);
    assert_eq!(d.array().unwrap(), Some(2));
    assert_eq!(d.u64().unwrap(), sender);
    assert_eq!(d.bytes().unwrap(), payload);
    assert_eq!(d.position(), bytes.len());
}
async fn fixture() -> (
    Arc<Relay>,
    Endpoint<wtransport::endpoint::endpoint_side::Client>,
    String,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    fixture_with(Config {
        auth_password: Some("test".into()),
        messages_per_second: 0,
        bytes_per_second: 0,
        ..Config::default()
    })
    .await
}
async fn fixture_with(
    config: Config,
) -> (
    Arc<Relay>,
    Endpoint<wtransport::endpoint::endpoint_side::Client>,
    String,
    CancellationToken,
    tokio::task::JoinHandle<anyhow::Result<()>>,
) {
    let identity = Identity::self_signed(["localhost", "127.0.0.1"]).unwrap();
    let hash = identity.certificate_chain().as_slice()[0].hash();
    let server = Endpoint::server(
        wtransport::ServerConfig::builder()
            .with_bind_address("127.0.0.1:0".parse().unwrap())
            .with_identity(identity)
            .build(),
    )
    .unwrap();
    let url = format!(
        "https://127.0.0.1:{}/war",
        server.local_addr().unwrap().port()
    );
    let client = Endpoint::client(
        wtransport::ClientConfig::builder()
            .with_bind_default()
            .with_server_certificate_hashes([hash])
            .build(),
    )
    .unwrap();
    let relay = Relay::new(config).unwrap();
    let stop = CancellationToken::new();
    let task = tokio::spawn(war_relay::webtransport::serve(
        server,
        relay.clone(),
        stop.clone(),
    ));
    (relay, client, url, stop, task)
}
#[tokio::test]
async fn reliable_and_unreliable_share_membership_and_retire_cleanly() {
    let (relay, endpoint, url, stop, task) = fixture().await;
    let mut a = Client::open(&endpoint, &url, "room").await;
    let mut b = Client::open(&endpoint, &url, "room").await;
    let body = vec![42; 65536];
    write_record(&mut a.send, &route(b.id, &body))
        .await
        .unwrap();
    verify(&b.read().await, a.id, &body);
    write_record(&mut b.send, &route(u64::MAX, b"broadcast"))
        .await
        .unwrap();
    verify(&a.read().await, b.id, b"broadcast");
    // The same CBOR route uses datagrams when explicitly submitted there.
    a.connection
        .send_datagram(route(b.id, b"unreliable"))
        .unwrap();
    let d = tokio::time::timeout(Duration::from_secs(3), b.connection.receive_datagram())
        .await
        .unwrap()
        .unwrap();
    verify(&d, a.id, b"unreliable");
    // Authenticate on the reliable lane before the protected listing command.
    write_record(
        &mut a.send,
        &[0x82, 0, 0x82, 0, 0x64, b't', b'e', b's', b't'],
    )
    .await
    .unwrap();
    assert_eq!(&a.read().await[..], &[0x82, 0, 0x82, 0, 0xf5]);
    // A reliable command still works after unreliable traffic.
    write_record(&mut a.send, &[0x82, 0, 0x82, 2, 0xf6])
        .await
        .unwrap();
    let reply = a.read().await;
    assert_eq!(&reply[..4], &[0x82, 0, 0x82, 2]);
    a.connection.close(VarInt::from_u32(0), b"done");
    b.connection.close(VarInt::from_u32(0), b"done");
    stop.cancel();
    task.await.unwrap().unwrap();
    assert_eq!(relay.counts().0, 0);
    assert_eq!(
        relay
            .metrics
            .connections
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    assert_eq!(
        relay
            .metrics
            .queued_bytes
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}
#[tokio::test]
async fn datagram_commands_are_rejected_without_applying_state() {
    let (relay, endpoint, url, stop, task) = fixture().await;
    let a = Client::open(&endpoint, &url, "room").await;
    // SET variable is forbidden on an unreliable lane.
    a.connection
        .send_datagram([0x82, 0, 0x82, 4, 0x82, 0x61, b'x', 1])
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), a.connection.closed())
        .await
        .unwrap();
    assert_eq!(
        relay
            .metrics
            .variable_bytes
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    stop.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn large_fanout_and_departures_progress_across_scheduler_quanta() {
    let (relay, endpoint, url, stop, task) = fixture().await;
    let mut source = Client::open(&endpoint, &url, "fanout").await;
    let mut peers = Vec::new();
    // Cross the core's 1024-recipient fairness quantum with actual QUIC peers.
    for _ in 0..1025 {
        peers.push(Client::open(&endpoint, &url, "fanout").await);
    }
    write_record(&mut source.send, &route(u64::MAX, b"shared"))
        .await
        .unwrap();
    for peer in &mut peers {
        verify(&peer.read().await, source.id, b"shared");
    }
    source.connection.close(VarInt::from_u32(0), b"done");
    for peer in &mut peers {
        let packet = peer.read().await;
        let mut decoder = minicbor::Decoder::new(&packet);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u64().unwrap(), u64::MAX);
        assert_eq!(decoder.u64().unwrap(), source.id);
    }
    stop.cancel();
    task.await.unwrap().unwrap();
    assert_eq!(relay.counts().0, 0);
    assert_eq!(
        relay
            .metrics
            .queued_bytes
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn application_activity_refreshes_idle_deadline_then_inactive_peer_expires() {
    let (relay, endpoint, url, stop, task) = fixture_with(Config {
        idle_timeout_secs: 1,
        messages_per_second: 0,
        bytes_per_second: 0,
        ..Config::default()
    })
    .await;
    let mut client = Client::open(&endpoint, &url, "idle").await;
    // Continue past the original deadline; each received self-route resets it.
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_millis(450)).await;
        write_record(&mut client.send, &route(client.id, b"active"))
            .await
            .unwrap();
        verify(&client.read().await, client.id, b"active");
    }
    assert_eq!(
        relay
            .metrics
            .idle_timeouts
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
    tokio::time::timeout(Duration::from_secs(3), client.connection.closed())
        .await
        .unwrap();
    assert_eq!(
        relay
            .metrics
            .idle_timeouts
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    stop.cancel();
    task.await.unwrap().unwrap();
    assert_eq!(relay.counts().0, 0);
}

#[tokio::test]
async fn read_admission_rejects_announced_growth_and_releases_all_credit() {
    let (relay, endpoint, url, stop, task) = fixture_with(Config {
        total_read_buffer_bytes: 40960,
        ..Config::default()
    })
    .await;
    let mut client = Client::open(&endpoint, &url, "budget").await;
    // Header alone exceeds the shared staging budget; no body need be allocated.
    client
        .send
        .write_all(&(32768u32.to_be_bytes()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), client.connection.closed())
        .await
        .unwrap();
    let budget = relay.metrics.read_budget.as_ref().unwrap();
    assert_eq!(budget.rejected(), 1);
    assert_eq!(budget.reserved(), 0);
    stop.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn refused_origins_and_connection_admission_preserve_the_live_session() {
    let (relay, endpoint, url, stop, task) = fixture_with(Config {
        max_connections: 1,
        origin_policy: war_relay::origin::OriginPolicy {
            allowed: vec!["https://allowed.example".into()],
        },
        ..Config::default()
    })
    .await;
    let options = wtransport::endpoint::ConnectOptions::builder(&url)
        .add_header("origin", "https://denied.example")
        .build();
    assert!(
        tokio::time::timeout(Duration::from_secs(3), endpoint.connect(options))
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(
        relay
            .metrics
            .origin_rejected
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    tokio::time::timeout(Duration::from_secs(3), async {
        while relay
            .metrics
            .connections
            .load(std::sync::atomic::Ordering::Relaxed)
            != 0
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut client = Client::open(&endpoint, &url, "admit").await;
    assert!(
        tokio::time::timeout(Duration::from_secs(3), endpoint.connect(&url))
            .await
            .unwrap()
            .is_err()
    );
    write_record(&mut client.send, &route(client.id, b"still live"))
        .await
        .unwrap();
    verify(&client.read().await, client.id, b"still live");
    stop.cancel();
    task.await.unwrap().unwrap();
    assert_eq!(
        relay
            .metrics
            .connections
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );
}

#[tokio::test]
async fn unreliable_and_reliable_input_share_the_same_rate_budget() {
    let (relay, endpoint, url, stop, task) = fixture_with(Config {
        messages_per_second: 1,
        ..Config::default()
    })
    .await;
    let mut client = Client::open(&endpoint, &url, "rate").await;
    write_record(&mut client.send, &route(client.id, b"first"))
        .await
        .unwrap();
    verify(&client.read().await, client.id, b"first");
    client
        .connection
        .send_datagram(route(client.id, b"second"))
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), client.connection.closed())
        .await
        .unwrap();
    assert_eq!(
        relay
            .metrics
            .rate_limited
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    stop.cancel();
    task.await.unwrap().unwrap();
}

#[tokio::test]
async fn graceful_close_ack_follows_delivery_and_registration_removal() {
    let (relay, endpoint, url, stop, task) = fixture().await;
    let mut source = Client::open(&endpoint, &url, "graceful").await;
    let mut recipient = Client::open(&endpoint, &url, "graceful").await;
    for _ in 0..16 {
        write_record(&mut source.send, &route(recipient.id, b"before close"))
            .await
            .unwrap();
    }
    write_record(&mut source.send, &[]).await.unwrap();
    assert!(source.read().await.is_empty());
    assert_eq!(relay.counts().0, 1, "ACK must follow registration removal");
    for _ in 0..16 {
        verify(&recipient.read().await, source.id, b"before close");
    }
    let departed = recipient.read().await;
    let mut d = minicbor::Decoder::new(&departed);
    assert_eq!(d.array().unwrap(), Some(2));
    assert_eq!(d.u64().unwrap(), u64::MAX);
    assert_eq!(d.u64().unwrap(), source.id);
    source
        .connection
        .close(VarInt::from_u32(1000), b"ack received");
    stop.cancel();
    task.await.unwrap().unwrap();
}
