//! Direct-peer admission counts QUIC connections by remote address.
use super::*;

fn from(server: &Server, address: u8) -> ClientEndpoint {
    client_endpoint(&server.hash, [127, 0, 0, address], None)
}

async fn join_from(server: &Server, endpoint: &ClientEndpoint) -> Client {
    Client::join(endpoint, server.options(), &protocol::text("admission")).await
}

/// The relay counts a refusal just after sending it, so callers wait for counters.
async fn refused(endpoint: &ClientEndpoint, options: ConnectOptions) {
    let attempt = timeout(Duration::from_secs(3), endpoint.connect(options))
        .await
        .unwrap();
    assert!(attempt.is_err(), "a refused connection was admitted");
}

async fn connections(server: &Server, expected: usize) {
    eventually(|| server.relay.metrics.connections.load(Relaxed) == expected).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn per_ip_admission_limits_each_address_and_keeps_other_addresses_usable() {
    let server = Server::new(Config {
        max_connections: 4,
        max_connections_per_ip: 1,
        ..config()
    })
    .await;
    let two = from(&server, 2);
    let first = join_from(&server, &two).await;
    for _ in 0..8 {
        // Forwarding headers cannot claim another address.
        let options = ConnectOptions::builder(&server.url)
            .add_header("x-forwarded-for", "127.0.0.9")
            .add_header("forwarded", "for=127.0.0.9")
            .build();
        refused(&two, options).await;
    }
    eventually(|| server.relay.metrics.ip_rejected.load(Relaxed) == 8).await;
    connections(&server, 1).await;
    let mut healthy = join_from(&server, &from(&server, 3)).await;
    let value = protocol::byte_string(&vec![0x7b; 32 * 1024]);
    healthy.send(healthy.id, &value).await;
    assert_eq!(healthy.read().await, protocol::envelope(healthy.id, &value));
    let fourth = join_from(&server, &from(&server, 4)).await;
    let fifth = join_from(&server, &from(&server, 5)).await;
    connections(&server, 4).await;
    // The global limit refuses a fresh address too.
    refused(&from(&server, 6), server.options()).await;
    // The eight per-address refusals plus this one.
    eventually(|| server.relay.metrics.rejected.load(Relaxed) == 9).await;
    assert_eq!(server.relay.metrics.ip_rejected.load(Relaxed), 8);
    fourth.close().await;
    fifth.close().await;
    first.close().await;
    connections(&server, 1).await;
    join_from(&server, &two).await.close().await;
    healthy.close().await;
    connections(&server, 0).await;
    server.stop().await;
}

#[tokio::test]
async fn unfinished_handshakes_time_out_and_release_connection_and_address_admission() {
    let server = Server::new(Config {
        max_connections: 2,
        max_connections_per_ip: 1,
        handshake_timeout_secs: 1,
        ..config()
    })
    .await;
    // Complete QUIC (with the HTTP/3 ALPN) but never send the session request.
    let client = wtransport::ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes([server.hash.clone()])
        .build();
    let quic = wtransport::quinn::Endpoint::client(SocketAddr::from(([127, 0, 0, 2], 0))).unwrap();
    let half = quic
        .connect_with(client.quic_config().clone(), server.address, "localhost")
        .unwrap()
        .await
        .unwrap();
    connections(&server, 1).await;
    let two = from(&server, 2);
    refused(&two, server.options()).await;
    eventually(|| server.relay.metrics.ip_rejected.load(Relaxed) == 1).await;
    let other = join_from(&server, &from(&server, 3)).await;
    refused(&from(&server, 4), server.options()).await;
    // The per-address refusal plus this global one.
    eventually(|| server.relay.metrics.rejected.load(Relaxed) == 2).await;
    // The deadline frees both the global slot and the address lease.
    timeout(Duration::from_secs(3), half.closed())
        .await
        .unwrap();
    assert_eq!(server.relay.metrics.handshake_errors.load(Relaxed), 1);
    connections(&server, 1).await;
    join_from(&server, &two).await.close().await;
    other.close().await;
    server.stop().await;
}
