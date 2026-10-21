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
use tokio::{
    sync::{Notify, mpsc},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;
use wtransport::{
    Connection, Endpoint, RecvStream, SendStream, VarInt, endpoint::endpoint_side::Server,
};

struct Output {
    connection: Connection,
    queue: Mutex<mpsc::Sender<Envelope>>,
    scratch: Mutex<Vec<u8>>,
    relay: Arc<Relay>,
    retirement: Arc<Notify>,
}
impl Output {
    fn submit(&self, packet: Submission<'_>, queue: &mpsc::Sender<Envelope>) -> bool {
        if !packet.frame.unreliable {
            return queue.try_send(packet.into_owned()).is_ok();
        }
        let frame = packet.frame;
        let Some(maximum) = self.connection.max_datagram_size() else {
            self.relay.metrics.datagrams_dropped.fetch_add(1, Relaxed);
            return true;
        };
        if frame.len() > maximum {
            self.relay.metrics.datagrams_oversized.fetch_add(1, Relaxed);
            self.relay.metrics.datagrams_dropped.fetch_add(1, Relaxed);
            return true;
        }
        let mut scratch = self.scratch.lock();
        scratch.clear();
        scratch.extend_from_slice(frame.prefix());
        scratch.extend_from_slice(frame.payload());
        if self.connection.send_datagram(scratch.as_slice()).is_ok() {
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
impl PeerOutput for Output {
    fn supports_retirement(&self) -> bool {
        true
    }
    fn wake_retirement(&self) {
        self.retirement.notify_one();
    }
    fn send(&self, packet: Submission<'_>) -> bool {
        self.submit(packet, &self.queue.lock())
    }
    fn close(&self, reason: Disconnect) {
        self.connection.close(
            VarInt::from_u32(reason.code.into()),
            reason.reason.as_bytes(),
        );
    }
    fn send_gated(&self, packet: Submission<'_>, gate: &dyn Fn() -> bool) -> bool {
        let queue = self.queue.lock();
        !gate() || self.submit(packet, &queue)
    }
}

/// Read one record without allocating its declared size before data arrives.
/// Keep this future alive across select branches: partial reads are not cancelled.
pub async fn read_record(stream: &mut RecvStream, maximum: usize) -> Result<Bytes> {
    read_record_budgeted(stream, maximum, None).await
}
async fn read_record_budgeted(
    stream: &mut RecvStream,
    maximum: usize,
    budget: Option<&Arc<tungstenite::protocol::ReadBudget>>,
) -> Result<Bytes> {
    let mut header = [0; 4];
    stream.read_exact(&mut header).await?;
    let size = u32::from_be_bytes(header) as usize;
    anyhow::ensure!(size > 0 && size <= maximum, "invalid record length");
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

pub async fn write_record(stream: &mut SendStream, bytes: &[u8]) -> Result<()> {
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

/// Run a preconfigured TLS/QUIC endpoint with the existing relay core.
pub async fn serve(
    endpoint: Endpoint<Server>,
    relay: Arc<Relay>,
    shutdown: CancellationToken,
) -> Result<()> {
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
                sessions.spawn(async move {
                    let _lease = ConnectionLease(relay.clone());
                    let _permit = permit;
                    let handshake = async {
                        let request = incoming.await?;
                        if request.path() != "/war" { request.not_found().await; bail!("unknown WebTransport path"); }
                        if request.origin().is_some_and(|origin| !relay.config.origin_policy.allowed.is_empty() && !relay.config.origin_policy.allowed.iter().any(|allowed| allowed == origin)) {
                            relay.metrics.origin_rejected.fetch_add(1, Relaxed);
                            request.forbidden().await;
                            bail!("origin rejected");
                        }
                        request.accept().await.context("accept WebTransport")
                    };
                    let connection = match tokio::time::timeout(Duration::from_secs(relay.config.handshake_timeout_secs), handshake).await {
                        Ok(Ok(connection)) => connection,
                        _ => { relay.metrics.handshake_errors.fetch_add(1, Relaxed); return; }
                    };
                    if let Err(error) = session(connection.clone(), relay, retirement).await {
                        tracing::debug!(%error, "WebTransport session ended");
                        connection.close(VarInt::from_u32(1002), b"Session ended");
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
struct ConnectionLease(Arc<Relay>);
impl Drop for ConnectionLease {
    fn drop(&mut self) {
        self.0.metrics.connections.fetch_sub(1, Relaxed);
    }
}

async fn session(connection: Connection, relay: Arc<Relay>, retirement: Arc<Notify>) -> Result<()> {
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
    let (queue, mut outgoing) = mpsc::channel(relay.config.queue_messages);
    let output = Arc::new(Output {
        connection: connection.clone(),
        queue: Mutex::new(queue),
        scratch: Mutex::new(Vec::new()),
        relay: relay.clone(),
        retirement,
    });
    let registration = relay
        .subscribe_with_output(
            protocol::join(&join).map_err(|e| anyhow::anyhow!(e.reason))?,
            output,
        )
        .map_err(|e| anyhow::anyhow!(e.reason))?;
    let limiter = Mutex::new(RateLimiter::new(
        relay.config.messages_per_second,
        relay.config.bytes_per_second,
        Instant::now(),
    ));
    let route = async |bytes: Bytes, unreliable: bool| {
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
        loop {
            route(
                read_record_budgeted(
                    &mut receive,
                    relay.config.max_message_bytes,
                    relay.metrics.read_budget.as_ref(),
                )
                .await?,
                false,
            )
            .await?;
            tokio::task::yield_now().await;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let datagrams = async {
        loop {
            let bytes = connection.receive_datagram().await?.payload();
            relay.metrics.datagrams_received.fetch_add(1, Relaxed);
            if bytes.len() > relay.config.max_message_bytes {
                registration.peer.close(Disconnect::PROTOCOL);
                bail!("datagram exceeds message limit");
            }
            route(bytes, true).await?;
            tokio::task::yield_now().await;
        }
        #[allow(unreachable_code)]
        Ok::<(), anyhow::Error>(())
    };
    let writer = async {
        while let Some(packet) = outgoing.recv().await {
            tokio::time::timeout(
                Duration::from_secs(relay.config.write_timeout_secs),
                write_frame(&mut send, &packet.frame),
            )
            .await??;
            relay.metrics.deliveries.fetch_add(1, Relaxed);
            relay
                .metrics
                .delivered_bytes
                .fetch_add(packet.frame.len() as u64, Relaxed);
        }
        Ok::<(), anyhow::Error>(())
    };
    tokio::select! {
        result = reliable => result,
        result = datagrams => result,
        result = writer => result,
        _ = connection.closed() => Ok(()),
    }
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
