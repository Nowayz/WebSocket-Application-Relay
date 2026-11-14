//! The handshake timeout bounds QUIC connections that never open a session.
use super::*;

async fn connections(server: &Server, expected: usize) {
    eventually(|| server.relay.metrics.connections.load(Relaxed) == expected).await;
}

#[tokio::test]
async fn unfinished_handshakes_time_out_and_release_their_connection() {
    let server = Server::new(Config {
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
    // Sessions from the same and other addresses are admitted alongside it.
    let same = client_endpoint(&server.hash, [127, 0, 0, 2], None);
    let mut first = Client::join(&same, server.options(), &protocol::text("handshake")).await;
    let second = server.join("handshake").await;
    connections(&server, 3).await;
    timeout(Duration::from_secs(3), half.closed())
        .await
        .unwrap();
    assert_eq!(server.relay.metrics.handshake_errors.load(Relaxed), 1);
    connections(&server, 2).await;
    first.send(first.id, &[0xf6]).await;
    assert_eq!(first.read().await, protocol::envelope(first.id, &[0xf6]));
    first.close().await;
    second.close().await;
    connections(&server, 0).await;
    assert_eq!(server.relay.metrics.rejected.load(Relaxed), 0);
    server.stop().await;
}
