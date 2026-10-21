//! HTTP/3 transport. One reliable bidirectional stream carries length-prefixed
//! CBOR records; datagrams carry complete, explicitly unreliable CBOR routes.
use crate::{
    Relay,
    frame::Frame,
    protocol::{self, Disconnect},
    relay::{Envelope, FANOUT_QUANTUM, PeerOutput, Submission},
    server::RateLimiter,
};
use anyhow::{Context, Result, bail};
use bytes::{Bytes, BytesMut};
use parking_lot::Mutex;
use std::{
    sync::{Arc, atomic::Ordering::Relaxed},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::{sync::Notify, task::JoinSet};
use tokio_util::sync::CancellationToken;
use web_transport_quinn::{SendStream, Session as Connection};
use wtransport::quinn::{Endpoint, VarInt};

mod queue;

struct Output {
    // Session includes HTTP metadata and per-clone polling state. These users
    // call its shared async API, so keep one handle rather than copying it into
    // each long-lived future and output sink.
    connection: Arc<Connection>,
    queue: queue::Queue<Envelope>,
    scratch: Mutex<Vec<u8>>,
    relay: Arc<Relay>,
    retirement: Arc<Notify>,
    activity: Option<Arc<Mutex<Instant>>>,
}
impl Output {
    fn submit(&self, packet: Submission<'_>, queue: &mut queue::State<Envelope>) -> bool {
        if !packet.frame.unreliable {
            let wake = queue.is_empty();
            let allocated = queue.allocated_bytes();
            let accepted = queue.try_push(packet.into_owned());
            let growth = queue.allocated_bytes() - allocated;
            if growth != 0 {
                self.relay
                    .metrics
                    .output_queue_capacity_bytes
                    .fetch_add(growth, Relaxed);
            }
            if accepted && wake {
                self.queue.ready.notify_one();
            }
            return accepted;
        }
        if queue.is_closed() {
            return false;
        }
        let frame = packet.frame;
        let maximum = self.connection.max_datagram_size();
        if frame.len() > maximum {
            self.relay.metrics.datagrams_oversized.fetch_add(1, Relaxed);
            self.relay.metrics.datagrams_dropped.fetch_add(1, Relaxed);
            return true;
        }
        let mut scratch = self.scratch.lock();
        scratch.clear();
        scratch.extend_from_slice(frame.prefix());
        scratch.extend_from_slice(frame.payload());
        if self
            .connection
            .send_datagram(Bytes::copy_from_slice(scratch.as_slice()))
            .is_ok()
        {
            if let Some(activity) = &self.activity {
                *activity.lock() = Instant::now();
            }
            self.relay.metrics.datagrams_submitted.fetch_add(1, Relaxed);
            self.relay
                .metrics
                .datagram_bytes_submitted
                .fetch_add(frame.len() as u64, Relaxed);
        } else {
            self.relay.metrics.datagrams_dropped.fetch_add(1, Relaxed);
        }
        // Unreliable pressure drops a message, not the session or its commands.
        true
    }
}
impl Drop for Output {
    fn drop(&mut self) {
        self.relay
            .metrics
            .output_queue_capacity_bytes
            .fetch_sub(self.queue.state.get_mut().allocated_bytes(), Relaxed);
    }
}
impl PeerOutput for Output {
    fn supports_retirement(&self) -> bool {
        true
    }
    fn wake_retirement(&self) {
        self.retirement.notify_one();
    }
    fn send(&self, packet: Submission<'_>) -> bool {
        self.submit(packet, &mut self.queue.state.lock())
    }
    fn close(&self, reason: Disconnect) {
        self.connection
            .close(u32::from(reason.code), reason.reason.as_bytes());
    }
    fn send_gated(&self, packet: Submission<'_>, gate: &dyn Fn() -> bool) -> bool {
        let mut queue = self.queue.state.lock();
        !gate() || self.submit(packet, &mut queue)
    }
}

/// Read one record without allocating its declared size before data arrives.
/// Keep this future alive across select branches: partial reads are not cancelled.
pub async fn read_record<S: AsyncRead + Unpin>(stream: &mut S, maximum: usize) -> Result<Bytes> {
    read_record_budgeted(stream, maximum, None).await
}
async fn read_record_budgeted<S: AsyncRead + Unpin>(
    stream: &mut S,
    maximum: usize,
    budget: Option<&Arc<tungstenite::protocol::ReadBudget>>,
) -> Result<Bytes> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    let size = u32::from_be_bytes(header) as usize;
    anyhow::ensure!(size <= maximum, "invalid record length");
    // Reserve the full announced record plus growth headroom before allocating.
    // The lease is released when immutable ownership passes to routing.
    let _lease = budget
        .map(|budget| budget.try_lease(size.saturating_mul(2)))
        .transpose()?;
    let mut body = BytesMut::with_capacity(size.min(4096));
    while body.len() < size {
        let count = (size - body.len()).min(4096);
        let start = body.len();
        body.resize(start + count, 0);
        stream.read_exact(&mut body[start..]).await?;
    }
    Ok(body.freeze())
}

pub async fn write_record<S: AsyncWrite + Unpin>(stream: &mut S, bytes: &[u8]) -> Result<()> {
    let size = u32::try_from(bytes.len()).context("record exceeds u32 framing")?;
    stream.write_all(&size.to_be_bytes()).await?;
    stream.write_all(bytes).await?;
    Ok(())
}
async fn write_frame(stream: &mut SendStream, frame: &Frame) -> Result<()> {
    let size = u32::try_from(frame.len()).context("record exceeds u32 framing")?;
    stream.write_all(&size.to_be_bytes()).await?;
    stream.write_all(frame.prefix()).await?;
    stream.write_all(frame.payload()).await?;
    Ok(())
}

/// Bind the shared UDP socket explicitly so bursts from many sessions do not
/// compete for a small per-socket OS default. The OS may cap requested sizes.
pub fn bind_endpoint(
    config: &crate::Config,
    transport: &wtransport::ServerConfig,
) -> Result<Endpoint> {
    let socket = std::net::UdpSocket::bind(config.bind)?;
    let settings = socket2::SockRef::from(&socket);
    if config.udp_receive_buffer_bytes != 0 {
        settings.set_recv_buffer_size(config.udp_receive_buffer_bytes)?;
    }
    if config.udp_send_buffer_bytes != 0 {
        settings.set_send_buffer_size(config.udp_send_buffer_bytes)?;
    }
    tracing::info!(
        receive_buffer_bytes = settings.recv_buffer_size()?,
        send_buffer_bytes = settings.send_buffer_size()?,
        "QUIC UDP socket buffers"
    );
    socket.set_nonblocking(true)?;
    Ok(Endpoint::new(
        transport.quic_endpoint_config().clone(),
        Some(transport.quic_config().clone()),
        socket,
        Arc::new(wtransport::quinn::TokioRuntime),
    )?)
}

/// Run a preconfigured TLS/QUIC endpoint with the existing relay core.
pub async fn serve(
    endpoint: Endpoint,
    relay: Arc<Relay>,
    shutdown: CancellationToken,
) -> Result<()> {
    let endpoint = Arc::new(endpoint);
    anyhow::ensure!(
        relay
            .metrics
            .webtransport_endpoint
            .set(endpoint.clone())
            .is_ok(),
        "relay already has a WebTransport endpoint"
    );
    let mut sessions = JoinSet::new();
    let retirement = Arc::new(Notify::new());
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            _ = async {
                if relay.has_retirements() { tokio::task::yield_now().await; }
                else { retirement.notified().await; }
            } => relay.progress_retirements(FANOUT_QUANTUM),
            Some(result) = sessions.join_next(), if !sessions.is_empty() => { if let Err(error) = result { tracing::warn!(%error, "WebTransport task failed"); } },
            incoming = endpoint.accept() => {
                let Some(incoming) = incoming else { break; };
                if relay.is_draining() || relay.metrics.connections.fetch_update(Relaxed, Relaxed, |n| (n < relay.config.max_connections).then_some(n + 1)).is_err() {
                    incoming.refuse();
                    relay.metrics.rejected.fetch_add(1, Relaxed);
                    continue;
                }
                let permit = if let Some(limit) = &relay.ip_limit {
                    match limit.acquire(incoming.remote_address().ip()) {
                        Some(permit) => Some(permit),
                        None => { incoming.refuse(); relay.metrics.connections.fetch_sub(1, Relaxed); relay.metrics.ip_rejected.fetch_add(1, Relaxed); continue; }
                    }
                } else { None };
                relay.metrics.accepted.fetch_add(1, Relaxed);
                let relay = relay.clone();
                let retirement = retirement.clone();
                let lease = ConnectionLease(relay.clone());
                // The larger admission future is cold. Release its storage after
                // the handshake instead of retaining it in every live task.
                let handshake = Box::pin(accept_connection(incoming, relay.clone()));
                sessions.spawn(async move {
                    let _lease = lease;
                    let _permit = permit;
                    let connection = match tokio::time::timeout(Duration::from_secs(relay.config.handshake_timeout_secs), handshake).await {
                        Ok(Ok(connection)) => connection,
                        _ => { relay.metrics.handshake_errors.fetch_add(1, Relaxed); return; }
                    };
                    if let Err(error) = session(connection.clone(), relay, retirement).await {
                        tracing::debug!(%error, "WebTransport session ended");
                        connection.close(1002, b"Session ended");
                    }
                });
            }
        }
    }
    relay.begin_shutdown();
    endpoint.close(VarInt::from_u32(1001), b"Server Shutdown");
    let drained = async { while sessions.join_next().await.is_some() {} };
    if tokio::time::timeout(
        Duration::from_secs(relay.config.shutdown_timeout_secs),
        drained,
    )
    .await
    .is_err()
    {
        sessions.abort_all();
        while sessions.join_next().await.is_some() {}
    }
    Ok(())
}

async fn accept_connection(
    incoming: wtransport::quinn::Incoming,
    relay: Arc<Relay>,
) -> Result<Arc<Connection>> {
    let quic = incoming.await?;
    let request = web_transport_quinn::Request::accept(quic).await?;
    if request.url.path() != "/war" {
        request.reject(axum::http::StatusCode::NOT_FOUND).await?;
        bail!("unknown WebTransport path");
    }
    if !relay.config.origin_policy.allows(&request.headers) {
        relay.metrics.origin_rejected.fetch_add(1, Relaxed);
        request.reject(axum::http::StatusCode::FORBIDDEN).await?;
        bail!("origin rejected");
    }
    request
        .ok()
        .await
        .map(Arc::new)
        .context("accept WebTransport")
}

struct ConnectionLease(Arc<Relay>);
impl Drop for ConnectionLease {
    fn drop(&mut self) {
        self.0.metrics.connections.fetch_sub(1, Relaxed);
    }
}

// Match the native receive fairness quantum: amortize scheduling while bounding
// ready input admitted in one turn. Large fanout keeps its separate work quantum.
const INPUT_QUANTUM: usize = 256;

async fn session(
    connection: Arc<Connection>,
    relay: Arc<Relay>,
    retirement: Arc<Notify>,
) -> Result<()> {
    let ((mut send, mut receive), join) =
        tokio::time::timeout(Duration::from_secs(relay.config.join_timeout_secs), async {
            let (send, mut receive) = connection.accept_bi().await?;
            let join = read_record_budgeted(
                &mut receive,
                relay.config.max_message_bytes,
                relay.metrics.read_budget.as_ref(),
            )
            .await?;
            Ok::<_, anyhow::Error>(((send, receive), join))
        })
        .await??;
    let activity =
        (relay.config.idle_timeout_secs != 0).then(|| Arc::new(Mutex::new(Instant::now())));
    let output = Arc::new(Output {
        connection: connection.clone(),
        queue: queue::Queue::new(relay.config.queue_messages),
        scratch: Mutex::new(Vec::new()),
        relay: relay.clone(),
        retirement,
        activity: activity.clone(),
    });
    let registration = relay
        .subscribe_with_output(
            protocol::join(&join).map_err(|e| anyhow::anyhow!(e.reason))?,
            output.clone(),
        )
        .map_err(|e| anyhow::anyhow!(e.reason))?;
    let finishing = CancellationToken::new();
    let mut outgoing = output.queue.receiver();
    let writer = async {
        loop {
            let packet = if finishing.is_cancelled() {
                outgoing.close();
                outgoing.recv().await
            } else {
                tokio::select! {
                    packet = outgoing.recv() => packet,
                    _ = finishing.cancelled() => { outgoing.close(); outgoing.recv().await }
                }
            };
            let Some(packet) = packet else {
                break;
            };
            tokio::time::timeout(
                Duration::from_secs(relay.config.write_timeout_secs),
                write_frame(&mut send, &packet.frame),
            )
            .await
            .inspect_err(|_| {
                relay.metrics.write_timeouts.fetch_add(1, Relaxed);
            })?
            .inspect_err(|_| {
                relay.metrics.write_errors.fetch_add(1, Relaxed);
            })?;
            if let Some(activity) = &activity {
                *activity.lock() = Instant::now();
            }
            relay.metrics.deliveries.fetch_add(1, Relaxed);
            relay
                .metrics
                .delivered_bytes
                .fetch_add(packet.frame.len() as u64, Relaxed);
        }
        if finishing.is_cancelled() {
            write_record(&mut send, &[]).await?;
        }
        Ok::<(), anyhow::Error>(())
    };
    tokio::pin!(writer);
    let result = {
        let limiter = Mutex::new(RateLimiter::new(
            relay.config.messages_per_second,
            relay.config.bytes_per_second,
            Instant::now(),
        ));
        let route = async |bytes: Bytes, unreliable: bool| {
            if let Some(activity) = &activity {
                *activity.lock() = Instant::now();
            }
            if !limiter.lock().allow(bytes.len(), Instant::now()) {
                relay.metrics.rate_limited.fetch_add(1, Relaxed);
                registration.peer.close(Disconnect::RATE);
                return Err(anyhow::anyhow!("rate exceeded"));
            }
            let mut pending = Pending {
                registration: &registration,
                work: None,
            };
            let mut batch = relay.receive_batch();
            let result = if unreliable {
                batch.receive_datagram(&registration.peer, bytes, &mut pending.work, FANOUT_QUANTUM)
            } else {
                let size = bytes.len();
                batch.receive_bounded(
                    &registration.peer,
                    bytes,
                    size,
                    &mut pending.work,
                    FANOUT_QUANTUM,
                )
            };
            batch.publish();
            drop(batch);
            while let Some(work) = pending.work.as_mut().filter(|work| work.active()) {
                tokio::task::yield_now().await;
                work.resume(FANOUT_QUANTUM)
                    .map_err(|e| anyhow::anyhow!(e.reason))?;
            }
            result.map(|_| ()).map_err(|error| {
                registration.peer.close(error.reason);
                anyhow::anyhow!(error.reason.reason)
            })
        };
        let reliable = async {
            let mut remaining = INPUT_QUANTUM;
            loop {
                let bytes = read_record_budgeted(
                    &mut receive,
                    relay.config.max_message_bytes,
                    relay.metrics.read_budget.as_ref(),
                )
                .await?;
                if bytes.is_empty() {
                    return Ok::<_, anyhow::Error>(());
                }
                route(bytes, false).await?;
                remaining -= 1;
                if remaining == 0 {
                    tokio::task::yield_now().await;
                    remaining = INPUT_QUANTUM;
                }
            }
        };
        let datagrams = async {
            let mut remaining = INPUT_QUANTUM;
            loop {
                let bytes = connection.read_datagram().await?;
                relay.metrics.datagrams_received.fetch_add(1, Relaxed);
                if bytes.len() > relay.config.max_message_bytes {
                    registration.peer.close(Disconnect::PROTOCOL);
                    bail!("datagram exceeds message limit");
                }
                route(bytes, true).await?;
                remaining -= 1;
                if remaining == 0 {
                    tokio::task::yield_now().await;
                    remaining = INPUT_QUANTUM;
                }
            }
            #[allow(unreachable_code)]
            Ok::<(), anyhow::Error>(())
        };
        let idle = async {
            let Some(activity) = &activity else {
                return std::future::pending::<Result<()>>().await;
            };
            let duration = Duration::from_secs(relay.config.idle_timeout_secs);
            loop {
                let deadline = *activity.lock() + duration;
                tokio::time::sleep_until(deadline.into()).await;
                if activity.lock().elapsed() >= duration {
                    relay.metrics.idle_timeouts.fetch_add(1, Relaxed);
                    registration.peer.close(Disconnect::IDLE);
                    return Ok(());
                }
            }
        };
        tokio::select! {
            result = idle => result.map(|_| false),
            result = reliable => result.map(|_| true),
            result = datagrams => result.map(|_| false),
            result = &mut writer => result.map(|_| false),
            _ = connection.closed() => Ok(false),
        }
    };
    // Routing futures are gone before registration: cancelled publications have
    // transferred their continuation to retirement, and no later input is admitted.
    drop(registration);
    if result? {
        finishing.cancel();
        tokio::time::timeout(
            Duration::from_secs(relay.config.write_timeout_secs),
            &mut writer,
        )
        .await??;
        // Keep the ACK deliverable until the client closes QUIC, or a bounded deadline.
        let _ = tokio::time::timeout(
            Duration::from_secs(relay.config.write_timeout_secs),
            connection.closed(),
        )
        .await;
        connection.close(1000, b"Graceful close complete");
    }
    Ok(())
}

// Preserve an already accepted publication when any session future is cancelled.
struct Pending<'a> {
    registration: &'a crate::relay::Registration,
    work: Option<Box<crate::relay::RoomFanout>>,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        if let Some(work) = self.work.take() {
            self.registration.detach_fanout(work);
        }
    }
}
