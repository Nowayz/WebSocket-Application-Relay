use std::{
    sync::{Arc, atomic::Ordering::Relaxed},
    time::Duration,
};

use anyhow::{Context, Result};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinSet,
    time::{Instant, interval_at, sleep, sleep_until, timeout},
};
use tokio_tungstenite::{
    accept_hdr_async_with_config,
    tungstenite::{
        self, Message,
        handshake::server::{Request, Response as HandshakeResponse},
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

use crate::{
    Relay,
    protocol::{self, Disconnect},
    transport::{self, WriteFailure},
};

/// Run pre-bound listeners; useful for embedders and tests with ephemeral ports.
pub async fn serve(
    listener: TcpListener,
    admin: Option<TcpListener>,
    relay: Arc<Relay>,
    shutdown: CancellationToken,
) -> Result<()> {
    let slots = Arc::new(Semaphore::new(relay.config.max_connections));
    let sessions_stop = CancellationToken::new();
    let mut tasks = JoinSet::new();
    if let Some(admin) = admin {
        let admin_relay = relay.clone();
        let token = sessions_stop.clone();
        tasks.spawn(async move {
            axum::serve(admin, admin_router(admin_relay))
                .with_graceful_shutdown(token.cancelled_owned())
                .await
                .context("admin listener failed")
                .map(|()| true)
        });
    }
    let result = loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break Ok(()),
            finished = tasks.join_next(), if !tasks.is_empty() => {
                match finished {
                    Some(Ok(Err(error))) => break Err(error),
                    Some(Ok(Ok(true))) => break Err(anyhow::anyhow!("admin listener stopped unexpectedly")),
                    Some(Err(error)) => warn!(%error, "service task panicked"),
                    _ => {}
                }
            }
            accepted = listener.accept() => {
                let (socket, _) = match accepted { Ok(accepted) => accepted, Err(error) => break Err(anyhow::Error::new(error).context("relay accept failed")) };
                relay.metrics.accepted.fetch_add(1, Relaxed);
                if relay.draining.load(Relaxed) {
                    relay.metrics.rejected.fetch_add(1, Relaxed);
                    continue;
                }
                let Ok(permit) = slots.clone().try_acquire_owned() else {
                    relay.metrics.rejected.fetch_add(1, Relaxed);
                    continue;
                };
                let relay = relay.clone();
                let token = sessions_stop.clone();
                relay.metrics.connections.fetch_add(1, Relaxed);
                // The permit and gauge are owned by the future, including during a handshake.
                let connection = ConnectionGuard(relay.metrics.clone());
                tasks.spawn(async move {
                    let (_connection, _permit) = (connection, permit);
                    if let Err(error) = connection_task(socket, relay, token).await { debug!(%error, "connection ended"); }
                    Ok(false)
                });
            }
        }
    };
    // Stop notification fanout before any connection sees shutdown. This avoids
    // quadratic disconnect traffic when a large channel is closed as a unit.
    relay.begin_shutdown();
    sessions_stop.cancel();
    shutdown.cancel();
    drop(listener);
    if timeout(
        Duration::from_secs(relay.config.shutdown_timeout_secs),
        async { while tasks.join_next().await.is_some() {} },
    )
    .await
    .is_err()
    {
        warn!("shutdown deadline reached; cancelling remaining connections");
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    info!(sessions = relay.counts().0, "relay stopped");
    result
}

struct ConnectionGuard(Arc<crate::metrics::Metrics>);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Relaxed);
    }
}

type Socket = transport::ServerSocket<TcpStream>;

async fn connection_task(
    socket: TcpStream,
    relay: Arc<Relay>,
    shutdown: CancellationToken,
) -> Result<()> {
    socket.set_nodelay(true)?;
    let config = &relay.config;
    let websocket_config = WebSocketConfig::default()
        .read_buffer_size(config.read_buffer_bytes)
        .write_buffer_size(4096)
        // Control frames use library staging. Native data uses the relay output
        // budget and borrowed vectored writes, independent of the incoming limit.
        .max_write_buffer_size(config.queue_bytes + 4096)
        .max_message_size(Some(config.max_message_bytes))
        .max_frame_size(Some(config.max_message_bytes));
    let handshake = accept_hdr_async_with_config(
        socket,
        |request: &Request, mut response: HandshakeResponse| {
            if let Some(protocols) = request.headers().get("sec-websocket-protocol") {
                if protocols
                    .to_str()
                    .ok()
                    .is_some_and(|s| s.split(',').any(|s| s.trim() == protocol::SUBPROTOCOL))
                {
                    response.headers_mut().insert(
                        "sec-websocket-protocol",
                        protocol::SUBPROTOCOL.parse().expect("static header"),
                    );
                } else {
                    return Err(tungstenite::http::Response::builder()
                        .status(400)
                        .body(Some("Unsupported subprotocol".into()))
                        .expect("static response"));
                }
            }
            Ok(response)
        },
        Some(websocket_config),
    );
    let ws = tokio::select! {
        _ = shutdown.cancelled() => return Ok(()),
        result = timeout(Duration::from_secs(config.handshake_timeout_secs), handshake) => match result {
            Ok(Ok(ws)) => ws,
            _ => { relay.metrics.handshake_errors.fetch_add(1, Relaxed); return Ok(()); }
        }
    };
    let mut ws = Socket::new(ws);
    let mut limiter = RateLimiter::new(config.messages_per_second, config.bytes_per_second);
    let stopped = shutdown.cancelled();
    tokio::pin!(stopped);
    let deadline = sleep(Duration::from_secs(config.join_timeout_secs));
    tokio::pin!(deadline);
    let subscription = loop {
        let incoming = tokio::select! {
            _ = stopped.as_mut() => { close(&mut ws, Disconnect::SHUTDOWN, &relay, &shutdown).await; return Ok(()); }
            _ = &mut deadline => { close(&mut ws, Disconnect { code: 1008, reason: "Channel Join Timeout" }, &relay, &shutdown).await; return Ok(()); }
            incoming = ws.next() => incoming,
        };
        match incoming {
            Some(Ok(Message::Close(_))) => {
                if !transport::acknowledge_close(ws.closing(), config.write_timeout()).await {
                    relay.metrics.close_failures.fetch_add(1, Relaxed);
                }
                return Ok(());
            }
            Some(Ok(message)) if !limiter.allow(message.len()) => {
                relay.metrics.rate_limited.fetch_add(1, Relaxed);
                close(&mut ws, Disconnect::RATE, &relay, &shutdown).await;
                return Ok(());
            }
            Some(Ok(Message::Binary(bytes))) => {
                match protocol::join(&bytes).and_then(|name| relay.subscribe(name)) {
                    Ok(subscription) => break subscription,
                    Err(reason) => {
                        close(&mut ws, reason, &relay, &shutdown).await;
                        return Ok(());
                    }
                }
            }
            Some(Ok(Message::Ping(_))) => {
                if let Err(failure) = transport::output(
                    ws.flush(),
                    config.write_timeout(),
                    stopped.as_mut(),
                    std::future::pending(),
                )
                .await
                {
                    if let SessionEnd::Close(reason) = output_failed(&relay, failure) {
                        close(&mut ws, reason, &relay, &shutdown).await;
                    }
                    return Ok(());
                }
            }
            Some(Ok(Message::Pong(_))) => {}
            None | Some(Err(_)) => return Ok(()),
            _ => {
                close(&mut ws, cbor_required(), &relay, &shutdown).await;
                return Ok(());
            }
        }
    };
    let crate::relay::Subscription {
        registration,
        mut outgoing,
        close: cancellation,
    } = subscription;
    let peer = &registration.peer;
    // These one-shot futures stay registered while the read loop and its output
    // operations take turns borrowing them. Every completion exits this session;
    // a completed signal is never polled again.
    let closing = transport::close_reason(cancellation);
    tokio::pin!(closing);
    let period = Duration::from_secs(config.ping_interval_secs.max(1));
    let mut heartbeat = interval_at(Instant::now() + period, period);
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut pong_deadline = None;
    let end = loop {
        tokio::select! {
            _ = stopped.as_mut() => break SessionEnd::Close(Disconnect::SHUTDOWN),
            reason = closing.as_mut() => break SessionEnd::Close(reason),
            _ = sleep_until(pong_deadline.unwrap_or_else(|| Instant::now() + Duration::from_secs(86_400))), if pong_deadline.is_some() => {
                break SessionEnd::Close(Disconnect { code: 1001, reason: "Heartbeat Timeout" });
            }
            _ = heartbeat.tick(), if config.ping_interval_secs > 0 => {
                if pong_deadline.is_none() {
                    if let Err(failure) = transport::output(ws.ping(bytes::Bytes::from_static(b"war")), config.write_timeout(), stopped.as_mut(), closing.as_mut()).await { break output_failed(&relay, failure); }
                    pong_deadline = Some(Instant::now() + Duration::from_secs(config.pong_timeout_secs));
                }
            }
            packet = outgoing.recv() => {
                let Some(packet) = packet else { break SessionEnd::Close(Disconnect::SHUTDOWN); };
                if let Err(failure) = transport::output(ws.binary(&packet.frame), config.write_timeout(), stopped.as_mut(), closing.as_mut()).await { break output_failed(&relay, failure); }
                relay.metrics.deliveries.fetch_add(1, Relaxed);
                relay.metrics.delivered_bytes.fetch_add(packet.frame.len() as u64, Relaxed);
            }
            incoming = ws.next() => {
                match incoming {
                    Some(Ok(Message::Close(_))) => break SessionEnd::Acknowledge,
                    Some(Ok(message)) if !limiter.allow(message.len()) => break SessionEnd::Close(Disconnect::RATE),
                    Some(Ok(Message::Binary(bytes))) => {
                        if let Err(reason) = relay.receive(peer, bytes) { break SessionEnd::Close(reason); }
                    }
                    Some(Ok(Message::Ping(_))) => { if let Err(failure) = transport::output(ws.flush(), config.write_timeout(), stopped.as_mut(), closing.as_mut()).await { break output_failed(&relay, failure); } }
                    Some(Ok(Message::Pong(bytes))) => { if bytes.as_ref() == b"war" { pong_deadline = None; } }
                    None | Some(Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed)) => break SessionEnd::Drop(Disconnect { code: 1000, reason: "" }),
                    Some(Err(tungstenite::Error::Io(_) | tungstenite::Error::Protocol(tungstenite::error::ProtocolError::ResetWithoutClosingHandshake))) => {
                        relay.metrics.transport_disconnects.fetch_add(1, Relaxed);
                        break SessionEnd::Drop(Disconnect { code: 1001, reason: "Transport Closed" });
                    }
                    Some(Err(tungstenite::Error::Capacity(_))) => break SessionEnd::Close(Disconnect { code: 1009, reason: "Message Too Large" }),
                    Some(Err(_)) => break SessionEnd::Close(Disconnect::PROTOCOL),
                    _ => break SessionEnd::Close(cbor_required()),
                }
            }
        }
    };
    // Capacity signals can race a peer's already queued Close. When no partial
    // output exists, preserve that handshake instead of sending a competing one.
    let end = match end {
        SessionEnd::Close(reason)
            if reason == Disconnect::OVERLOADED && ws.take_pending_close() =>
        {
            SessionEnd::Acknowledge
        }
        end => end,
    };
    let reason = end.reason();
    if reason.code == 1002 {
        relay.metrics.protocol_errors.fetch_add(1, Relaxed);
    }
    if reason == Disconnect::RATE {
        relay.metrics.rate_limited.fetch_add(1, Relaxed);
    }
    peer.close(reason);
    // Routing membership and queued-byte reservations end immediately, before a
    // potentially slow close handshake. RAII still covers task aborts and panics.
    drop(registration);
    drop(outgoing);
    match end {
        SessionEnd::Close(reason) => close(&mut ws, reason, &relay, &shutdown).await,
        SessionEnd::Acknowledge => {
            if !transport::acknowledge_close(ws.closing(), config.write_timeout()).await {
                relay.metrics.close_failures.fetch_add(1, Relaxed);
            }
        }
        SessionEnd::Drop(_) => {} // A partial/failed write cannot be safely retried.
    }
    Ok(())
}

fn cbor_required() -> Disconnect {
    Disconnect {
        code: 1003,
        reason: "CBOR Binary Messages Required",
    }
}

enum SessionEnd {
    Close(Disconnect),
    Acknowledge,
    Drop(Disconnect),
}

impl SessionEnd {
    fn reason(&self) -> Disconnect {
        match self {
            Self::Close(reason) | Self::Drop(reason) => *reason,
            Self::Acknowledge => Disconnect {
                code: 1000,
                reason: "",
            },
        }
    }
}

fn output_failed(relay: &Relay, failure: WriteFailure) -> SessionEnd {
    let reason = match failure {
        WriteFailure::ClosedBeforeStart(reason) => {
            relay.metrics.aborted_writes.fetch_add(1, Relaxed);
            relay
                .metrics
                .unstarted_writes_cancelled
                .fetch_add(1, Relaxed);
            return SessionEnd::Close(reason);
        }
        WriteFailure::Closed(reason) => {
            relay.metrics.aborted_writes.fetch_add(1, Relaxed);
            reason
        }
        WriteFailure::TimedOut => {
            relay.metrics.write_timeouts.fetch_add(1, Relaxed);
            Disconnect {
                code: 1013,
                reason: "Write Timeout",
            }
        }
        WriteFailure::Transport => {
            relay.metrics.write_errors.fetch_add(1, Relaxed);
            Disconnect {
                code: 1001,
                reason: "Transport Closed",
            }
        }
    };
    SessionEnd::Drop(reason)
}

async fn close(ws: &mut Socket, reason: Disconnect, relay: &Relay, shutdown: &CancellationToken) {
    if !transport::initiate_close(
        ws.closing(),
        Some(CloseFrame {
            code: CloseCode::from(reason.code),
            reason: reason.reason.into(),
        }),
        relay.config.write_timeout(),
        Some(shutdown),
    )
    .await
    {
        relay.metrics.close_failures.fetch_add(1, Relaxed);
    }
}

struct RateLimiter {
    rates: [u64; 2],
    tokens: [f64; 2],
    updated: Instant,
}

impl RateLimiter {
    fn new(messages: u64, bytes: u64) -> Self {
        Self {
            rates: [messages, bytes],
            tokens: [messages as f64, bytes as f64],
            updated: Instant::now(),
        }
    }

    fn allow(&mut self, bytes: usize) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.updated).as_secs_f64();
        self.updated = now;
        for (i, cost) in [1.0, bytes as f64].into_iter().enumerate() {
            if self.rates[i] == 0 {
                continue;
            }
            let rate = self.rates[i] as f64;
            self.tokens[i] = (self.tokens[i] + elapsed * rate).min(rate);
            if self.tokens[i] < cost {
                return false;
            }
            self.tokens[i] -= cost;
        }
        true
    }
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

async fn channels(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> Response {
    if !authorized(&relay, &headers, false) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    Json(relay.channels()).into_response()
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
