//! HTTP management for the native relay.
use std::{
    num::NonZeroUsize,
    sync::{Arc, atomic::Ordering::Relaxed},
};

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
                if relay.draining.load(Relaxed) {
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
            .render(sessions, channels, relay.draining.load(Relaxed)),
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
    relay.draining.store(draining, Relaxed);
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
