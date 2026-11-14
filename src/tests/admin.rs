use super::*;
use crate::Config;
use std::{sync::Barrier, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::timeout,
};
use tokio_util::sync::CancellationToken;

#[test]
fn authorization_compares_exact_configured_token_bytes() {
    for token in ["ascii-fixture", "unicode-π-世界"] {
        let relay = Relay::new(Config {
            admin_token: Some(token.into()),
            ..Config::default()
        })
        .unwrap();
        for (value, expected) in [
            (format!("Bearer {token}").into_bytes(), true),
            (format!("Bearer {token}x").into_bytes(), false),
            (format!("Basic {token}").into_bytes(), false),
            (b"Bearer invalid-\xff".to_vec(), false),
        ] {
            let mut headers = HeaderMap::new();
            headers.insert(
                "authorization",
                axum::http::HeaderValue::from_bytes(&value).unwrap(),
            );
            assert_eq!(authorized(&relay, &headers, false), expected);
            assert_eq!(authorized(&relay, &headers, true), expected);
        }
        assert!(!authorized(&relay, &HeaderMap::new(), false));
        assert!(!authorized(&relay, &HeaderMap::new(), true));
    }
}

#[tokio::test]
async fn unicode_admin_token_authenticates_real_http_without_text_coercion() {
    timeout(Duration::from_secs(2), async {
        let relay = Relay::new(Config { admin_token: Some("unicode-π-世界".into()), ..Config::default() }).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let token = stop.clone();
        let router = admin_router(relay.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).with_graceful_shutdown(token.cancelled_owned()).await.unwrap();
        });
        for (credential, expected) in [
            (None, "401"),
            (Some("Bearer unicode-π-世界x"), "401"),
            (Some("Bearer unicode-π-世界"), "200"),
        ] {
            let mut stream = TcpStream::connect(address).await.unwrap();
            let auth = credential.map(|value| format!("Authorization: {value}\r\n")).unwrap_or_default();
            stream.write_all(format!("GET /v1/channels?limit=1 HTTP/1.1\r\nHost: localhost\r\n{auth}Connection: close\r\n\r\n").as_bytes()).await.unwrap();
            let mut bytes = Vec::new();
            stream.read_to_end(&mut bytes).await.unwrap();
            let response = String::from_utf8(bytes).unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {expected} ")));
        }
        assert_eq!(relay.counts(), (0, 1));
        stop.cancel();
        server.await.unwrap();
    }).await.expect("exact-byte management authentication must complete promptly");
}

#[tokio::test]
async fn empty_channel_page_preserves_json_wire_headers_and_authorization() {
    timeout(Duration::from_secs(2), async {
        let relay = Relay::new(Config {
            admin_token: Some("shutdown-test".into()),
            ..Config::default()
        })
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let stop = CancellationToken::new();
        let token = stop.clone();
        let router = admin_router(relay.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(token.cancelled_owned())
                .await
                .unwrap();
        });
        let empty_path = "/v1/channels?limit=1&after=cmVfZ2xvYmw%3D";
        let response = http(address, "GET", empty_path, true).await;
        assert!(response.starts_with("HTTP/1.1 200 "));
        let (headers, body) = response.split_once("\r\n\r\n").unwrap();
        let headers = headers.to_ascii_lowercase();
        assert!(
            headers
                .lines()
                .any(|line| line == "content-type: application/json")
        );
        assert!(headers.lines().any(|line| line == "content-length: 2"));
        assert_eq!(body, "[]");
        assert!(
            http(address, "GET", empty_path, false)
                .await
                .starts_with("HTTP/1.1 401 ")
        );
        assert!(
            http(address, "GET", "/v1/channels?after=cmVfZ2xvYmw%3D", true)
                .await
                .starts_with("HTTP/1.1 400 ")
        );
        let populated = http(address, "GET", "/v1/channels?limit=1", true).await;
        assert!(populated.starts_with("HTTP/1.1 200 "));
        let body = populated.split_once("\r\n\r\n").unwrap().1;
        let rows: serde_json::Value = serde_json::from_str(body).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(relay.counts(), (0, 1));
        stop.cancel();
        server.await.unwrap();
    })
    .await
    .expect("empty and populated page checks must finish promptly");
}

#[test]
fn shutdown_cannot_be_resumed_by_an_authorized_management_request() {
    let relay = Relay::new(Config {
        admin_token: Some("shutdown-test".into()),
        ..Config::default()
    })
    .unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("authorization", "Bearer shutdown-test".parse().unwrap());
    relay.begin_shutdown();
    assert_eq!(
        set_drain(relay.clone(), headers, false),
        StatusCode::CONFLICT
    );
    assert!(
        relay.is_draining(),
        "shutdown must keep readiness and admission drained"
    );
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn reversible_drain_and_terminal_shutdown_have_distinct_transitions() {
    let relay = Relay::new(Config::default()).unwrap();
    assert!(!relay.is_draining() && !relay.is_stopping());
    assert!(relay.set_draining(true));
    assert!(relay.is_draining() && !relay.is_stopping());
    assert!(relay.set_draining(true));
    assert!(relay.set_draining(false));
    assert!(!relay.is_draining() && !relay.is_stopping());
    relay.begin_shutdown();
    assert!(relay.is_draining() && relay.is_stopping());
    assert!(!relay.set_draining(false));
    assert!(relay.set_draining(true));
    assert!(relay.is_stopping());
    relay.begin_shutdown();
    assert!(relay.is_stopping());
}

#[test]
fn concurrent_resume_and_drain_cannot_overwrite_shutdown() {
    for _ in 0..64 {
        let relay = Relay::new(Config::default()).unwrap();
        let start = Barrier::new(3);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                start.wait();
                for i in 0..256 {
                    let _ = relay.set_draining(i % 2 == 0);
                }
            });
            scope.spawn(|| {
                start.wait();
                relay.begin_shutdown();
            });
            start.wait();
        });
        assert!(relay.is_stopping() && relay.is_draining());
        assert!(!relay.set_draining(false));
    }
}

async fn http(
    address: std::net::SocketAddr,
    method: &str,
    path: &str,
    authorized: bool,
) -> String {
    let mut stream = TcpStream::connect(address).await.unwrap();
    let auth = if authorized {
        "Authorization: Bearer shutdown-test\r\n"
    } else {
        ""
    };
    stream.write_all(format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n{auth}Content-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
    let mut bytes = Vec::new();
    timeout(Duration::from_secs(3), stream.read_to_end(&mut bytes))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8(bytes).unwrap()
}

#[tokio::test]
async fn disconnect_authorizes_before_rejecting_malformed_session_ids() {
    let relay = Relay::new(Config {
        admin_token: Some("shutdown-test".into()),
        ..Config::default()
    })
    .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let stop = CancellationToken::new();
    let stopped = stop.clone();
    let server = tokio::spawn(async move {
        axum::serve(listener, admin_router(relay))
            .with_graceful_shutdown(stopped.cancelled_owned())
            .await
            .unwrap();
    });
    for id in ["bad", "-1", "18446744073709551616", "%FF"] {
        let path = format!("/v1/sessions/{id}/disconnect");
        assert!(
            http(address, "POST", &path, false)
                .await
                .starts_with("HTTP/1.1 401"),
            "{id}"
        );
        assert!(
            http(address, "POST", &path, true)
                .await
                .starts_with("HTTP/1.1 400"),
            "{id}"
        );
    }
    for id in ["0", "1", "18446744073709551615"] {
        let path = format!("/v1/sessions/{id}/disconnect");
        assert!(
            http(address, "POST", &path, false)
                .await
                .starts_with("HTTP/1.1 401"),
            "{id}"
        );
        assert!(
            http(address, "POST", &path, true)
                .await
                .starts_with("HTTP/1.1 404"),
            "{id}"
        );
    }
    assert!(
        http(address, "GET", "/healthz", false)
            .await
            .starts_with("HTTP/1.1 200")
    );
    stop.cancel();
    timeout(Duration::from_secs(1), server)
        .await
        .unwrap()
        .unwrap();
}
