//! HTTP/3 transport. One reliable bidirectional stream carries length-prefixed
//! CBOR records; datagrams carry complete, explicitly unreliable CBOR routes.
mod ingress;
pub use ingress::EndpointIngress;

use crate::{
    Relay,
    frame::Frame,
    protocol::{self, Disconnect},
    relay::{Envelope, FANOUT_QUANTUM, PeerOutput, Submission},
    server::RateLimiter,
};
use anyhow::{Context, Result, bail};
use bytes::Bytes;
use parking_lot::Mutex;
use std::{
    future::{Future, poll_fn},
    sync::{Arc, atomic::Ordering::Relaxed},
    task::{Context as TaskContext, Poll, Waker},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::{sync::Notify, task::JoinSet};
use tokio_util::sync::CancellationToken;
use web_transport_quinn::{SendStream, Session as Connection};
use wtransport::quinn::{Endpoint, VarInt};

mod queue;

struct QueuedOutput {
    packet: Envelope,
    offset: usize,
    started: Option<Instant>,
}

struct StreamOutput {
    stream: SendStream,
    busy: bool,
}

struct Output {
    // Session includes HTTP metadata and per-clone polling state. These users
    // call its shared async API, so keep one handle rather than copying it into
    // each long-lived future and output sink.
    connection: Arc<Connection>,
    queue: queue::Queue<QueuedOutput, StreamOutput>,
    relay: Arc<Relay>,
    retirement: Arc<Notify>,
    activity: Option<Arc<Mutex<Instant>>>,
}
impl Output {
    fn submit(
        &self,
        packet: Submission<'_>,
        queue: &mut queue::State<QueuedOutput, StreamOutput>,
        immediate: bool,
    ) -> bool {
        if !packet.frame.unreliable {
            if queue.is_closed() {
                return false;
            }
            let wake = queue.is_empty();
            let mut offset = 0;
            let mut started = None;
            if immediate && wake && !queue.owner.busy {
                let mut chunks = frame_chunks(packet.frame, 0);
                // One cancel-safe poll: Pending consumes no bytes. The queue
                // notification wakes the owning writer, which polls again with
                // its real waker; this producer never waits for write readiness.
                let attempt = queue.owner.stream.write_chunks(&mut chunks);
                tokio::pin!(attempt);
                if let Poll::Ready(Ok(written)) = attempt
                    .as_mut()
                    .poll(&mut TaskContext::from_waker(Waker::noop()))
                {
                    offset = written.bytes;
                    if offset == 4 + packet.frame.len() {
                        self.delivered(packet.frame);
                        return true;
                    }
                    if offset != 0 {
                        started = Some(Instant::now());
                    }
                }
            }
            let allocated = queue.allocated_bytes();
            let accepted = queue.try_push(QueuedOutput {
                packet: packet.into_owned(),
                offset,
                started,
            });
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
        // Quinn retains an owned contiguous datagram until transmission. Build
        // that allocation directly instead of copying through per-peer scratch.
        let mut datagram = Vec::with_capacity(frame.len());
        datagram.extend_from_slice(frame.prefix());
        datagram.extend_from_slice(frame.payload());
        if self.connection.send_datagram(Bytes::from(datagram)).is_ok() {
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
impl Output {
    fn delivered(&self, frame: &Frame) {
        if let Some(activity) = &self.activity {
            *activity.lock() = Instant::now();
        }
        self.relay.metrics.deliveries.fetch_add(1, Relaxed);
        self.relay
            .metrics
            .delivered_bytes
            .fetch_add(frame.len() as u64, Relaxed);
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
        self.submit(packet, &mut self.queue.state.lock(), true)
    }
    fn send_fanout(&self, packet: Submission<'_>) -> bool {
        self.submit(packet, &mut self.queue.state.lock(), false)
    }
    fn close(&self, reason: Disconnect) {
        self.connection
            .close(u32::from(reason.code), reason.reason.as_bytes());
    }
    fn send_gated(&self, packet: Submission<'_>, gate: &dyn Fn() -> bool) -> bool {
        let mut queue = self.queue.state.lock();
        !gate() || self.submit(packet, &mut queue, false)
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
    let mut body = Vec::with_capacity(size.min(4096));
    while body.len() < size {
        let count = (size - body.len()).min(4096);
        let start = body.len();
        if start + count > body.capacity() {
            // Keep geometric growth without retaining a power-of-two allocation
            // larger than the complete record (including its CBOR envelope).
            let capacity = body.capacity().saturating_mul(2).min(size);
            body.reserve_exact(capacity.max(start + count) - start);
        }
        body.resize(start + count, 0);
        stream.read_exact(&mut body[start..]).await?;
    }
    Ok(Bytes::from(body))
}

pub async fn write_record<S: AsyncWrite + Unpin>(stream: &mut S, bytes: &[u8]) -> Result<()> {
    let size = u32::try_from(bytes.len()).context("record exceeds u32 framing")?;
    stream.write_all(&size.to_be_bytes()).await?;
    stream.write_all(bytes).await?;
    Ok(())
}
fn frame_chunks(frame: &Frame, offset: usize) -> [Bytes; 2] {
    let size = u32::try_from(frame.len()).expect("validated message length");
    let mut prefix = [0; 18];
    prefix[..4].copy_from_slice(&size.to_be_bytes());
    let header_len = 4 + frame.prefix().len();
    prefix[4..header_len].copy_from_slice(frame.prefix());
    [
        if offset < header_len {
            Bytes::copy_from_slice(&prefix[offset..header_len])
        } else {
            Bytes::new()
        },
        frame
            .shared_payload()
            .slice(offset.saturating_sub(header_len)..),
    ]
}
async fn write_output_chunks(output: &Output, chunks: &mut [Bytes]) -> Result<()> {
    poll_fn(|cx| {
        let mut state = output.queue.state.lock();
        // Chunk suffixes live outside this temporary future, so a Pending poll
        // preserves all accepted bytes without retaining the stream in a task.
        let result = {
            let write = state.owner.stream.write_all_chunks(chunks);
            tokio::pin!(write);
            write.as_mut().poll(cx)
        };
        if matches!(&result, Poll::Ready(Ok(()))) {
            state.owner.busy = false;
        }
        result
    })
    .await?;
    Ok(())
}
async fn write_frame_from(output: &Output, frame: &Frame, offset: usize) -> Result<()> {
    write_output_chunks(output, &mut frame_chunks(frame, offset)).await
}

/// Default QUIC transport policy used by the relay's server builders.
pub fn server_transport_config() -> wtransport::quinn::TransportConfig {
    let mut transport = wtransport::quinn::TransportConfig::default();
    let mut acknowledgements = wtransport::quinn::AckFrequencyConfig::default();
    // Request eight packets per ACK, with a bounded sparse-traffic delay and
    // immediate acknowledgement of reordering. Unsupported peers ignore this.
    acknowledgements
        .ack_eliciting_threshold(VarInt::from_u32(7))
        .max_ack_delay(Some(Duration::from_millis(5)))
        .reordering_threshold(VarInt::from_u32(1));
    transport.ack_frequency_config(Some(acknowledgements));
    transport
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
    let ((send, mut receive), join) =
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
        queue: queue::Queue::with_owner(
            relay.config.queue_messages,
            StreamOutput {
                stream: send,
                busy: false,
            },
        ),
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
    // A ready input burst must let writers run before one producer can fill
    // an entire recipient queue. Use admission as the smaller fairness bound.
    let record_quantum = INPUT_QUANTUM.min(relay.config.queue_messages);
    let writer = async {
        // Mark the writer active while dequeuing under the submission gate.
        // A popped record still precedes new output even if the queue is empty.
        let take = |packet, owner: &mut StreamOutput| {
            assert!(!owner.busy);
            owner.busy = true;
            packet
        };
        let mut remaining = record_quantum;
        loop {
            let (packet, was_idle) = if finishing.is_cancelled() {
                outgoing.close();
                outgoing.recv_with_idle_map(take).await
            } else {
                tokio::select! {
                    packet = outgoing.recv_with_idle_map(take) => packet,
                    _ = finishing.cancelled() => { outgoing.close(); outgoing.recv_with_idle_map(take).await }
                }
            };
            let Some(queued) = packet else {
                break;
            };
            if was_idle {
                remaining = record_quantum;
            }
            let packet = queued.packet;
            let remaining_time = Duration::from_secs(relay.config.write_timeout_secs)
                .saturating_sub(
                    queued
                        .started
                        .map_or(Duration::ZERO, |started| started.elapsed()),
                );
            tokio::time::timeout(
                remaining_time,
                write_frame_from(&output, &packet.frame, queued.offset),
            )
            .await
            .inspect_err(|_| {
                relay.metrics.write_timeouts.fetch_add(1, Relaxed);
            })?
            .inspect_err(|_| {
                relay.metrics.write_errors.fetch_add(1, Relaxed);
            })?;
            output.delivered(&packet.frame);
            remaining -= 1;
            if remaining == 0 {
                tokio::task::yield_now().await;
                remaining = record_quantum;
            }
        }
        if finishing.is_cancelled() {
            write_output_chunks(&output, &mut [Bytes::from_static(&[0, 0, 0, 0])]).await?;
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
            let mut remaining = record_quantum;
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
                    remaining = record_quantum;
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
            biased;
            result = idle => result.map(|_| false),
            _ = connection.closed() => Ok(false),
            // Flush already admitted output before another ready input batch.
            // Both record loops yield at record_quantum, so priority cannot
            // turn a continuously ready writer into input starvation.
            result = &mut writer => result.map(|_| false),
            result = reliable => result.map(|_| true),
            result = datagrams => result.map(|_| false),
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

#[cfg(test)]
mod immediate_output_tests {
    use super::*;

    #[test]
    fn every_partial_record_offset_reconstructs_the_exact_remaining_bytes() {
        for frame in [
            Frame::integer(u64::MAX),
            Frame::envelope(u64::MAX, Bytes::from_static(b"\x44body")),
        ] {
            let original = frame_chunks(&frame, 0).concat();
            for offset in 0..=original.len() {
                assert_eq!(frame_chunks(&frame, offset).concat(), original[offset..]);
            }
        }
    }
}
