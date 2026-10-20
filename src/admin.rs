//! HTTP management for the native relay.
use std::{num::NonZeroUsize, sync::Arc};

use axum::{
    Router,
    extract::{Path, Query, State, rejection::PathRejection},
    http::{HeaderMap, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::{BufMut, BytesMut};
use serde::Deserialize;
use tracing::info;

use crate::{Relay, protocol, relay::MAX_CHANNEL_PAGE_SIZE};

#[derive(Deserialize)]
struct ChannelQuery {
    limit: Option<NonZeroUsize>,
    after: Option<String>,
}

fn cursor_key(cursor: &str) -> Result<[u8; 24], StatusCode> {
    let invalid = StatusCode::BAD_REQUEST;
    if cursor.len() > 24 {
        return Err(invalid);
    }
    let mut raw = [0; 18];
    let len = STANDARD
        .decode_slice(cursor, &mut raw)
        .map_err(|_| invalid)?;
    if len > 16 {
        return Err(invalid);
    }
    let mut key = [0; 24];
    key[..cursor.len()].copy_from_slice(cursor.as_bytes());
    Ok(key)
}

pub fn admin_router(relay: Arc<Relay>) -> Router {
    Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route(
            "/readyz",
            get(|State(relay): State<Arc<Relay>>| async move {
                if relay.is_draining() {
                    StatusCode::SERVICE_UNAVAILABLE
                } else {
                    StatusCode::OK
                }
            }),
        )
        .route("/metrics", get(metrics))
        .route("/v1/channels", get(channels))
        .route("/v1/drain", post(drain))
        .route("/v1/resume", post(resume))
        .route("/v1/sessions/{id}/disconnect", post(disconnect))
        .with_state(relay)
}

fn authorized(relay: &Relay, headers: &HeaderMap, metrics: bool) -> bool {
    if metrics && relay.config.admin_token.is_none() {
        return true;
    }
    headers
        .get("authorization")
        .and_then(|h| h.as_bytes().strip_prefix(b"Bearer "))
        .is_some_and(|s| protocol::secret_matches(relay.config.admin_token.as_deref(), s))
}

async fn metrics(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> Response {
    if !authorized(&relay, &headers, true) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let (sessions, channels) = relay.counts();
    (
        [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
        relay
            .metrics
            .render(sessions, channels, relay.is_draining()),
    )
        .into_response()
}

async fn channels(State(relay): State<Arc<Relay>>, headers: HeaderMap, uri: Uri) -> Response {
    if !authorized(&relay, &headers, false) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let channels = if uri.query().is_some_and(|query| !query.is_empty()) {
        // Authorize before parsing, so malformed queries expose no inspection
        // behavior to callers without the management token.
        let Query(query) = match Query::<ChannelQuery>::try_from_uri(&uri) {
            Ok(query) => query,
            Err(error) => return error.into_response(),
        };
        match query.limit {
            Some(limit) if limit.get() <= MAX_CHANNEL_PAGE_SIZE => {
                let after = match query.after.as_deref().map(cursor_key).transpose() {
                    Ok(after) => after,
                    Err(status) => return status.into_response(),
                };
                relay.channel_page(limit, after)
            }
            Some(_) => {
                return (StatusCode::BAD_REQUEST, "limit must be in 1..=1000").into_response();
            }
            None if query.after.is_some() => {
                return (StatusCode::BAD_REQUEST, "after requires limit").into_response();
            }
            None => relay.channel_listing(),
        }
    } else {
        relay.channel_listing()
    };
    // A normal bounded-name row fits in about 128 bytes. This is a capacity
    // hint, not a limit: escaped names and large counters may grow the buffer.
    // Snapshot and sort-key storage do not borrow the registry during encoding.
    let capacity = channels.len().saturating_mul(128).saturating_add(2);
    let mut buffer = BytesMut::with_capacity(capacity).writer();
    match serde_json::to_writer(&mut buffer, &channels) {
        Ok(()) => (
            [("content-type", "application/json")],
            buffer.into_inner().freeze(),
        )
            .into_response(),
        Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
    }
}

async fn drain(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> StatusCode {
    set_drain(relay, headers, true)
}
async fn resume(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> StatusCode {
    set_drain(relay, headers, false)
}

fn set_drain(relay: Arc<Relay>, headers: HeaderMap, draining: bool) -> StatusCode {
    if !authorized(&relay, &headers, false) {
        return StatusCode::UNAUTHORIZED;
    }
    if !relay.set_draining(draining) {
        return StatusCode::CONFLICT;
    }
    info!(draining, "administrator changed admission state");
    StatusCode::NO_CONTENT
}

async fn disconnect(
    State(relay): State<Arc<Relay>>,
    headers: HeaderMap,
    id: Result<Path<u64>, PathRejection>,
) -> Response {
    if !authorized(&relay, &headers, false) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    let Path(id) = match id {
        Ok(id) => id,
        Err(error) => return error.into_response(),
    };
    let status = if relay.disconnect(id) {
        info!(id, "administrator disconnected session");
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    };
    status.into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use futures_util::{SinkExt, StreamExt};
    use std::{sync::Barrier, time::Duration};
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::{TcpListener, TcpStream},
        time::timeout,
    };
    use tokio_tungstenite::{connect_async, tungstenite::Message};
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

    #[tokio::test]
    async fn disconnect_targets_exact_uint64_sessions_and_preserves_neighbours() {
        use std::sync::atomic::Ordering::Relaxed;

        async fn assigned(
            address: std::net::SocketAddr,
            id: u64,
        ) -> (tokio_tungstenite::WebSocketStream<TcpStream>, u64) {
            let stream = TcpStream::connect(address).await.unwrap();
            let (mut ws, _) = tokio_tungstenite::client_async(format!("ws://{address}"), stream)
                .await
                .unwrap();
            ws.send(Message::Binary(protocol::byte_string(b"admin-ids")))
                .await
                .unwrap();
            let Some(Ok(Message::Binary(welcome))) = ws.next().await else {
                panic!("missing welcome");
            };
            let old = minicbor::Decoder::new(&welcome).u64().unwrap();
            for (op, value, expected) in [
                (
                    0,
                    protocol::byte_string(b"admin-id-auth"),
                    protocol::reply(0, b"\xf5"),
                ),
                (
                    3,
                    protocol::uint(id),
                    protocol::reply(3, &protocol::uint(id)),
                ),
            ] {
                ws.send(Message::Binary(protocol::request(op, &value)))
                    .await
                    .unwrap();
                assert_eq!(ws.next().await.unwrap().unwrap(), Message::Binary(expected));
            }
            (ws, old)
        }

        timeout(Duration::from_secs(5), async {
            let relay = Relay::new(Config {
                workers: Some(2),
                ping_interval_secs: 0,
                auth_password: Some("admin-id-auth".into()),
                admin_token: Some("shutdown-test".into()),
                ..Config::default()
            })
            .unwrap();
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let admin = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let admin_address = admin.local_addr().unwrap();
            let stop = CancellationToken::new();
            let _cancel_on_exit = stop.clone().drop_guard();
            let mut tasks = tokio::task::JoinSet::new();
            tasks.spawn(crate::server::serve(
                listener,
                None,
                relay.clone(),
                stop.clone(),
            ));
            let router = admin_router(relay.clone());
            let stopped = stop.clone();
            tasks.spawn(async move {
                axum::serve(admin, router)
                    .with_graceful_shutdown(stopped.cancelled_owned())
                    .await
                    .map_err(anyhow::Error::from)
            });
            for target in [u32::MAX as u64 + 1, (1u64 << 53) + 1, u64::MAX - 1] {
                let neighbour = target - 1;
                let (mut guard, _) = assigned(address, neighbour).await;
                let (mut victim, old_id) = assigned(address, target).await;
                let path = format!("/v1/sessions/{target}/disconnect");
                assert!(
                    http(admin_address, "POST", &path, false)
                        .await
                        .starts_with("HTTP/1.1 401")
                );
                // Authentication and absent/old IDs must not mutate either session.
                for absent in [old_id, target + 1] {
                    assert!(
                        http(
                            admin_address,
                            "POST",
                            &format!("/v1/sessions/{absent}/disconnect"),
                            true
                        )
                        .await
                        .starts_with("HTTP/1.1 404")
                    );
                }
                let probe = protocol::envelope(target, &protocol::uint(17));
                victim.send(Message::Binary(probe.clone())).await.unwrap();
                assert_eq!(
                    victim.next().await.unwrap().unwrap(),
                    Message::Binary(probe)
                );
                assert!(
                    http(admin_address, "POST", &path, true)
                        .await
                        .starts_with("HTTP/1.1 204")
                );
                let Some(Ok(Message::Close(Some(frame)))) = victim.next().await else {
                    panic!("target {target} did not receive administrative Close");
                };
                assert_eq!(u16::from(frame.code), 4000);
                assert_eq!(frame.reason, "Administrative Disconnect");
                assert!(
                    crate::transport::acknowledge_close(&mut victim, Duration::from_secs(1)).await
                );
                // The live neighbour receives exactly this departed uint64 ID.
                assert_eq!(
                    guard.next().await.unwrap().unwrap(),
                    Message::Binary(protocol::envelope(
                        protocol::BROADCAST,
                        &protocol::uint(target)
                    ))
                );
                let probe = protocol::envelope(neighbour, &protocol::uint(42));
                guard.send(Message::Binary(probe.clone())).await.unwrap();
                assert_eq!(guard.next().await.unwrap().unwrap(), Message::Binary(probe));
                assert!(
                    http(admin_address, "POST", &path, true)
                        .await
                        .starts_with("HTTP/1.1 404")
                );
                assert!(
                    crate::transport::initiate_close(
                        &mut guard,
                        None,
                        Duration::from_secs(1),
                        None
                    )
                    .await
                );
            }
            // Physical socket retirement precedes the owning worker's scratch
            // reclamation within its turn. Wait for both observable resources.
            while relay.metrics.connections.load(Relaxed) != 0
                || relay.metrics.read_budget.as_ref().unwrap().reserved() != 0
            {
                tokio::task::yield_now().await;
            }
            assert_eq!(relay.counts(), (0, 1));
            assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
            assert_eq!(relay.metrics.read_budget.as_ref().unwrap().reserved(), 0);
            stop.cancel();
            while let Some(result) = tasks.join_next().await {
                result.unwrap().unwrap();
            }
            assert_eq!(relay.metrics.native_connection_slots.load(Relaxed), 0);
            assert_eq!(relay.metrics.output_queue_capacity_bytes.load(Relaxed), 0);
        })
        .await
        .expect("exact-ID disconnect, neighbour liveness and retirement must finish promptly");
    }

    #[tokio::test]
    async fn management_readiness_metrics_and_real_admission_stay_stopped() {
        let relay = Relay::new(Config {
            workers: Some(4),
            ping_interval_secs: 0,
            admin_token: Some("shutdown-test".into()),
            ..Config::default()
        })
        .unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let data_address = listener.local_addr().unwrap();
        let url = format!("ws://{data_address}/");
        let admin = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let admin_address = admin.local_addr().unwrap();
        let data_stop = CancellationToken::new();
        let data_task = tokio::spawn(crate::server::serve(
            listener,
            None,
            relay.clone(),
            data_stop.clone(),
        ));
        let admin_stop = CancellationToken::new();
        let router = admin_router(relay.clone());
        let stop = admin_stop.clone();
        let admin_task = tokio::spawn(async move {
            axum::serve(admin, router)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
        });
        let (mut existing, _) = connect_async(&url).await.unwrap();
        existing
            .send(Message::Binary(protocol::text("shutdown-one")))
            .await
            .unwrap();
        let Message::Binary(welcome) = existing.next().await.unwrap().unwrap() else {
            panic!("missing welcome")
        };
        let id = minicbor::Decoder::new(&welcome).u64().unwrap();
        assert!(
            http(admin_address, "POST", "/v1/drain", true)
                .await
                .starts_with("HTTP/1.1 204")
        );
        assert!(
            http(admin_address, "GET", "/readyz", false)
                .await
                .starts_with("HTTP/1.1 503")
        );
        let packet = protocol::envelope(id, &protocol::uint(7));
        existing
            .send(Message::Binary(packet.clone()))
            .await
            .unwrap();
        assert_eq!(
            existing.next().await.unwrap().unwrap(),
            Message::Binary(packet)
        );
        assert!(
            http(admin_address, "POST", "/v1/resume", true)
                .await
                .starts_with("HTTP/1.1 204")
        );
        assert!(
            http(admin_address, "GET", "/readyz", false)
                .await
                .starts_with("HTTP/1.1 200")
        );
        let (mut pending, _) = connect_async(&url).await.unwrap();

        // Model the interval after shutdown publishes its admission decision
        // and before every readiness worker consumes its stop notification.
        relay.begin_shutdown();
        assert!(
            http(admin_address, "POST", "/v1/resume", false)
                .await
                .starts_with("HTTP/1.1 401")
        );
        assert!(
            http(admin_address, "POST", "/v1/resume", true)
                .await
                .starts_with("HTTP/1.1 409")
        );
        assert!(
            http(admin_address, "POST", "/v1/drain", true)
                .await
                .starts_with("HTTP/1.1 204")
        );
        assert!(
            http(admin_address, "GET", "/readyz", false)
                .await
                .starts_with("HTTP/1.1 503")
        );
        assert!(
            http(admin_address, "GET", "/metrics", true)
                .await
                .contains("war_draining 1\n")
        );
        pending
            .send(Message::Binary(protocol::text("shutdown-pending")))
            .await
            .unwrap();
        let Message::Close(frame) = pending.next().await.unwrap().unwrap() else {
            panic!("stopping relay admitted a pending join")
        };
        assert_eq!(u16::from(frame.unwrap().code), 1013);
        pending.flush().await.unwrap();
        assert!(
            timeout(Duration::from_secs(3), pending.next())
                .await
                .unwrap()
                .is_none()
        );
        let mut denied = TcpStream::connect(data_address).await.unwrap();
        let closed = timeout(Duration::from_secs(3), denied.read(&mut [0]))
            .await
            .unwrap();
        assert!(
            matches!(closed, Ok(0))
                || matches!(closed,Err(ref error) if error.kind()==std::io::ErrorKind::ConnectionReset)
        );

        data_stop.cancel();
        let Message::Close(frame) = timeout(Duration::from_secs(3), existing.next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
        else {
            panic!("missing shutdown Close")
        };
        assert_eq!(u16::from(frame.unwrap().code), 1001);
        timeout(Duration::from_secs(3), data_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            http(admin_address, "POST", "/v1/resume", true)
                .await
                .starts_with("HTTP/1.1 409")
        );
        assert!(
            http(admin_address, "GET", "/readyz", false)
                .await
                .starts_with("HTTP/1.1 503")
        );
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(
            relay
                .metrics
                .connections
                .load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(relay.metrics.read_budget.as_ref().unwrap().reserved(), 0);
        admin_stop.cancel();
        timeout(Duration::from_secs(3), admin_task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
