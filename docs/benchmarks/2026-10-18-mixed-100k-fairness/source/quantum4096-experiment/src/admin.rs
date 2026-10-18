//! HTTP management for the native relay.
use std::{num::NonZeroUsize, sync::Arc};

use axum::{
    Router,
    extract::{Path, Query, State},
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
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .is_some_and(|s| {
            protocol::secret_matches(relay.config.admin_token.as_deref(), s.as_bytes())
        })
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
    Path(id): Path<u64>,
) -> StatusCode {
    if !authorized(&relay, &headers, false) {
        return StatusCode::UNAUTHORIZED;
    }
    if relay.disconnect(id) {
        info!(id, "administrator disconnected session");
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    }
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
