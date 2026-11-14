//! Browser-origin admission of the WebTransport session request.
use super::*;
use war_relay::origin::OriginPolicy;

const APP: &str = "https://app.example";

fn policy(origins: &[&str]) -> OriginPolicy {
    OriginPolicy {
        allowed: origins.iter().map(|origin| (*origin).into()).collect(),
    }
}

fn with_origin(server: &Server, origin: Option<&str>) -> ConnectOptions {
    let options = ConnectOptions::builder(&server.url);
    match origin {
        Some(origin) => options.add_header("origin", origin).build(),
        None => options.build(),
    }
}

async fn joined(server: &Server, origin: Option<&str>) -> Client {
    let options = with_origin(server, origin);
    Client::join(&server.endpoint, options, &protocol::text("origin")).await
}

#[tokio::test]
async fn denied_origins_release_admission_and_leave_routing_usable() {
    let server = Server::new(Config {
        max_connections: 2,
        origin_policy: policy(&[APP]),
        ..config()
    })
    .await;
    let mut healthy = joined(&server, Some(APP)).await;
    let denied = [
        "https://app.example.evil",
        "https://app.example/",
        "http://app.example",
        "https://app.example:444",
        "https://APP.example",
        "",
        "null",
        "https://app.example https://other.example",
        "https://app.example,https://other.example",
    ];
    for origin in denied {
        let attempt = timeout(
            Duration::from_secs(3),
            server.endpoint.connect(with_origin(&server, Some(origin))),
        )
        .await
        .unwrap();
        assert!(attempt.is_err(), "{origin:?} was admitted");
        // With two connection slots, a leaked one would refuse the next try.
        eventually(|| server.relay.metrics.connections.load(Relaxed) == 1).await;
        assert_eq!(server.relay.counts(), (1, 2));
        assert_eq!(server.relay.metrics.queued_bytes.load(Relaxed), 0);
        healthy.send(healthy.id, &[0x18, 42]).await;
        assert_eq!(
            healthy.read().await,
            protocol::envelope(healthy.id, &[0x18, 42])
        );
    }
    let rejected = denied.len() as u64;
    assert_eq!(server.relay.metrics.origin_rejected.load(Relaxed), rejected);
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 1);
    let metrics = server.http("GET", "/metrics", Some("admin-test")).await;
    assert!(metrics.contains(&format!("war_origin_rejected_total {rejected}\n")));
    healthy.close().await;
    server.stop().await;
}

#[tokio::test]
async fn exact_origins_originless_native_clients_and_explicit_null_can_join() {
    let ipv6 = "http://[::1]:4180";
    let server = Server::new(Config {
        origin_policy: policy(&[APP, ipv6, "null"]),
        ..config()
    })
    .await;
    for origin in [Some(APP), Some(ipv6), None, Some("null")] {
        let mut client = joined(&server, origin).await;
        // Nonminimal definite CBOR must remain byte-identical after admission.
        let value = [0xa1, 0x78, 1, b'x', 0x18, 1];
        client.send(client.id, &value).await;
        assert_eq!(client.read().await, protocol::envelope(client.id, &value));
        client.close().await;
        eventually(|| server.relay.metrics.connections.load(Relaxed) == 0).await;
    }
    assert_eq!(server.relay.metrics.origin_rejected.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.handshake_errors.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn no_origin_policy_admits_any_origin() {
    let server = Server::new(config()).await;
    for origin in ["https://unlisted.example", "null", ""] {
        let mut client = joined(&server, Some(origin)).await;
        client.send(client.id, &[0xf6]).await;
        assert_eq!(client.read().await, protocol::envelope(client.id, &[0xf6]));
        client.close().await;
        eventually(|| server.relay.metrics.connections.load(Relaxed) == 0).await;
    }
    assert_eq!(server.relay.metrics.origin_rejected.load(Relaxed), 0);
    server.stop().await;
}

/// `/metrics` documents origin and per-IP refusals as included in
/// `rejected_total`, and an origin refusal is not a failed handshake.
#[tokio::test]
async fn origin_and_address_refusals_are_counted_as_rejections() {
    let server = Server::new(Config {
        max_connections_per_ip: 1,
        origin_policy: policy(&[APP]),
        ..config()
    })
    .await;
    let denied = server
        .endpoint
        .connect(with_origin(&server, Some("https://other.example")))
        .await;
    assert!(denied.is_err());
    eventually(|| server.relay.metrics.connections.load(Relaxed) == 0).await;
    let _held = joined(&server, Some(APP)).await;
    assert!(server.endpoint.connect(server.options()).await.is_err());
    assert_eq!(server.relay.metrics.origin_rejected.load(Relaxed), 1);
    eventually(|| server.relay.metrics.ip_rejected.load(Relaxed) == 1).await;
    assert_eq!(server.relay.metrics.handshake_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.rejected.load(Relaxed), 2);
    server.stop().await;
}
