//! Real HTTP and WebSocket origin admission.
use super::*;
use tokio_tungstenite::tungstenite::{
    Error,
    handshake::client::Request,
    http::{StatusCode, header},
    protocol::{CloseFrame, frame::coding::CloseCode},
};
use war_relay::origin::OriginPolicy;

const APP: &str = "https://app.example";

fn policy(origins: &[&str]) -> OriginPolicy {
    OriginPolicy {
        allowed: origins.iter().map(|origin| (*origin).into()).collect(),
    }
}

fn request(url: &str, origins: &[&str]) -> Request {
    let mut request = url.into_client_request().unwrap();
    for origin in origins {
        request
            .headers_mut()
            .append(header::ORIGIN, origin.parse().unwrap());
    }
    request
}

async fn forbidden(request: Request) {
    let error = timeout(Duration::from_secs(3), connect_async(request))
        .await
        .unwrap()
        .unwrap_err();
    let Error::Http(response) = error else {
        panic!("expected HTTP 403, got {error:?}");
    };
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(response.headers()[header::CONNECTION], "close");
    assert_eq!(response.headers()[header::CONTENT_LENGTH], "19");
    assert_eq!(
        response.body().as_deref(),
        Some(&b"Origin Not Allowed\n"[..])
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

async fn joined(request: Request) -> (Socket, u64) {
    let (mut socket, _) = connect_async(request).await.unwrap();
    socket
        .send(Message::Binary(protocol::text("origin")))
        .await
        .unwrap();
    let id = Decoder::new(&data(&mut socket).await).u64().unwrap();
    (socket, id)
}

async fn clean_close(socket: &mut Socket) {
    socket
        .send(Message::Close(Some(CloseFrame {
            code: CloseCode::Normal,
            reason: "origin-check".into(),
        })))
        .await
        .unwrap();
    assert_eq!(close_code(socket).await, 1000);
    assert!(
        timeout(Duration::from_secs(3), socket.next())
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn denied_origins_release_admission_before_join_and_leave_routing_usable() {
    let server = Server::new(Config {
        max_connections: 2,
        origin_policy: policy(&[APP]),
        ..config()
    })
    .await;
    let (mut healthy, id) = joined(request(&server.url, &[APP])).await;
    let budget = server.relay.metrics.read_budget.as_ref().unwrap();
    let admitted = budget.reserved();
    let denied = [
        vec!["https://app.example.evil"],
        vec!["https://app.example/"],
        vec!["http://app.example"],
        vec!["https://app.example:444"],
        vec!["https://APP.example"],
        vec![""],
        vec!["null"],
        vec!["https://app.example https://other.example"],
        vec!["https://app.example,https://other.example"],
        vec![APP, APP],
        vec![APP, "https://other.example"],
    ];
    for origins in &denied {
        forbidden(request(&server.url, origins)).await;
        connections(&server, 1).await;
        assert_eq!(budget.reserved(), admitted);
        assert_eq!(server.relay.counts(), (1, 2));
        assert_eq!(server.relay.metrics.queued_bytes.load(Relaxed), 0);
        send(&mut healthy, id, &[0x18, 42]).await;
        assert_eq!(
            data(&mut healthy).await,
            protocol::envelope(id, &[0x18, 42])
        );
    }
    // Origin denial takes precedence over an unsupported offered protocol.
    let mut bad_protocol = request(&server.url, &["https://other.example"]);
    bad_protocol
        .headers_mut()
        .insert(header::SEC_WEBSOCKET_PROTOCOL, "other.v1".parse().unwrap());
    forbidden(bad_protocol).await;
    connections(&server, 1).await;
    let rejected = denied.len() as u64 + 1;
    assert_eq!(server.relay.metrics.origin_rejected.load(Relaxed), rejected);
    assert_eq!(server.relay.metrics.rejected.load(Relaxed), rejected);
    assert_eq!(server.relay.metrics.handshake_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.join_rejected.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.joined.load(Relaxed), 1);
    assert_eq!(budget.rejected(), 0);
    let metrics = server.http("GET", "/metrics", Some("admin-test")).await;
    assert!(metrics.contains(&format!("war_origin_rejected_total {rejected}\n")));
    clean_close(&mut healthy).await;
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
    for origins in [vec![APP], vec![ipv6], vec![], vec!["null"]] {
        let mut request = request(&server.url, &origins);
        request.headers_mut().insert(
            header::SEC_WEBSOCKET_PROTOCOL,
            protocol::SUBPROTOCOL.parse().unwrap(),
        );
        let (mut socket, response) = connect_async(request).await.unwrap();
        assert_eq!(
            response.headers()[header::SEC_WEBSOCKET_PROTOCOL],
            protocol::SUBPROTOCOL
        );
        socket
            .send(Message::Binary(protocol::text("origin")))
            .await
            .unwrap();
        let id = Decoder::new(&data(&mut socket).await).u64().unwrap();
        // Nonminimal definite CBOR must remain byte-identical after admission.
        let value = [0xa1, 0x78, 1, b'x', 0x18, 1];
        send(&mut socket, id, &value).await;
        assert_eq!(data(&mut socket).await, protocol::envelope(id, &value));
        clean_close(&mut socket).await;
        connections(&server, 0).await;
    }
    assert_eq!(server.relay.metrics.origin_rejected.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.handshake_errors.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    server.stop().await;
}

#[tokio::test]
async fn no_origin_policy_preserves_unrestricted_requests_and_protocol_errors_stay_separate() {
    let server = Server::new(config()).await;
    for origins in [
        vec!["https://unlisted.example"],
        vec!["null"],
        vec![APP, APP],
    ] {
        let (mut socket, id) = joined(request(&server.url, &origins)).await;
        send(&mut socket, id, &[0xf6]).await;
        assert_eq!(data(&mut socket).await, protocol::envelope(id, &[0xf6]));
        clean_close(&mut socket).await;
        connections(&server, 0).await;
    }
    assert_eq!(server.relay.metrics.origin_rejected.load(Relaxed), 0);
    server.stop().await;

    let server = Server::new(Config {
        origin_policy: policy(&[APP]),
        ..config()
    })
    .await;
    let mut bad_protocol = request(&server.url, &[APP]);
    bad_protocol
        .headers_mut()
        .insert(header::SEC_WEBSOCKET_PROTOCOL, "other.v1".parse().unwrap());
    let Error::Http(response) = connect_async(bad_protocol).await.unwrap_err() else {
        panic!("expected unsupported-protocol HTTP response");
    };
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    connections(&server, 0).await;
    assert_eq!(server.relay.metrics.handshake_errors.load(Relaxed), 1);
    assert_eq!(server.relay.metrics.rejected.load(Relaxed), 0);
    assert_eq!(server.relay.metrics.origin_rejected.load(Relaxed), 0);
    server.stop().await;
}
