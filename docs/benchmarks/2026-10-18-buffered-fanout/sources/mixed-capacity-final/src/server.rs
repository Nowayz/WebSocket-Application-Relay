//! Native readiness-driven relay. One event loop owns each decoder; socket
//! output is serialized and can be submitted directly by any routing worker.
//! Idle connections need no task, message channel, or receive allocation.
mod socket;
mod timers;

#[cfg(test)]
mod accept_tests;
#[cfg(test)]
mod arena_tests;
#[cfg(all(test, target_os = "linux"))]
mod close_tests;
#[cfg(test)]
mod fairness_tests;
#[cfg(test)]
mod ownership_tests;

pub use crate::admin::admin_router;

use std::{
    collections::{HashMap, VecDeque},
    io::{self, Write},
    num::NonZeroUsize,
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering::{Acquire, Relaxed},
        },
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use bytes::Bytes;
use mio::{Events, Interest, Poll, Registry, Token, net::TcpListener};
use slab::Slab;
use tokio::{net::TcpListener as AsyncListener, task::JoinSet};
use tokio_tungstenite::tungstenite::{
    self, Error, Message, WebSocket,
    handshake::{
        HandshakeError, MidHandshake,
        server::{Callback, ErrorResponse, Request, Response, ServerHandshake},
    },
    protocol::{CloseFrame, ReadLease, ReadStorage, WebSocketConfig, frame::coding::CloseCode},
};
use tokio_util::sync::CancellationToken;

use crate::{
    Relay,
    protocol::{self, Disconnect},
    relay::{ReceiveBatch, Registration, RoomFanout},
};
use socket::{Signal, Socket, Stream};
use timers::Timers;

const LISTENER: Token = Token(0);
const WAKE: Token = Token(1);
const READ_QUANTUM: usize = 256;
const FANOUT_QUANTUM: usize = 1024;
const ACCEPT_QUANTUM: usize = 16;

/// Run pre-bound listeners. Tokio services management and signals; native data
/// sockets belong to a fixed number of readiness workers. Config's backlog
/// applies to the pre-bound socket and every additional native listener.
pub async fn serve(
    listener: AsyncListener,
    admin: Option<AsyncListener>,
    relay: Arc<Relay>,
    shutdown: CancellationToken,
) -> Result<()> {
    let listener = listener.into_std()?;
    let workers = relay.config.workers.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map_or(2, usize::from)
            .min(16)
    });
    let worker_cores = if relay.config.pin_workers {
        let mut cores = core_affinity::get_core_ids()
            .context("cannot discover allowed CPUs for native worker affinity")?;
        anyhow::ensure!(
            cores.len() >= workers,
            "pin-workers needs one allowed CPU per native worker: {} CPUs for {workers} workers",
            cores.len()
        );
        cores.truncate(workers);
        Some(cores)
    } else {
        None
    };
    let mut prepared = Vec::with_capacity(workers);
    for listener in worker_listeners(listener, workers, relay.config.listen_backlog)? {
        prepared.push(Worker::new(listener, relay.clone())?);
    }
    let signals = StopOnDrop {
        signals: prepared
            .iter()
            .map(|worker| worker.signal.clone())
            .collect(),
        relay: relay.clone(),
    };
    let mut tasks = JoinSet::new();
    for (index, worker) in prepared.into_iter().enumerate() {
        let (finished, result) = tokio::sync::oneshot::channel();
        let core = worker_cores.as_ref().map(|cores| cores[index]);
        // These loops own dedicated OS threads. An embedder's Tokio blocking
        // pool may be small or busy and must not limit native I/O concurrency.
        let thread = std::thread::Builder::new()
            .name(format!("war-io-{index}"))
            .spawn(move || {
                let outcome = (|| {
                    if let Some(core) = core {
                        anyhow::ensure!(
                            core_affinity::set_for_current(core),
                            "cannot pin native I/O worker {index} to allowed CPU {}",
                            core.id
                        );
                        tracing::info!(worker = index, cpu = core.id, "pinned native I/O worker");
                    }
                    worker.run()
                })();
                let _ = finished.send(outcome);
            })
            .context("cannot start native I/O worker")?;
        tasks.spawn(async move {
            let outcome = result.await;
            thread
                .join()
                .map_err(|_| anyhow::anyhow!("native I/O worker panicked"))?;
            outcome.context("native I/O worker stopped without reporting")?
        });
    }
    let admin_stop = CancellationToken::new();
    if let Some(admin) = admin {
        let stop = admin_stop.clone();
        let router = admin_router(relay.clone());
        tasks.spawn(async move {
            axum::serve(admin, router)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .context("admin listener failed")
        });
    }
    let result = tokio::select! {
        _ = shutdown.cancelled() => Ok(()),
        result = tasks.join_next() => match result {
            Some(Ok(Err(error))) => Err(error),
            Some(Err(error)) => Err(error.into()),
            _ => Err(anyhow::anyhow!("service worker stopped unexpectedly")),
        },
    };
    relay.begin_shutdown();
    for signal in &signals.signals {
        signal.stop();
    }
    admin_stop.cancel();
    shutdown.cancel();
    let deadline = Duration::from_secs(relay.config.shutdown_timeout_secs);
    let cleanup = async {
        while let Some(worker) = tasks.join_next().await {
            worker.context("service worker panicked")??;
        }
        Ok::<_, anyhow::Error>(())
    };
    // Workers have their own bounded shutdown deadline. Give cleanup one extra
    // second to return those results rather than detaching an active data loop.
    tokio::time::timeout(deadline + Duration::from_secs(1), cleanup)
        .await
        .context("service shutdown exceeded its deadline")??;
    result
}

fn worker_listeners(
    listener: std::net::TcpListener,
    workers: usize,
    backlog: u16,
) -> io::Result<Vec<std::net::TcpListener>> {
    socket2::SockRef::from(&listener).listen(i32::from(backlog))?;
    let mut listeners = Vec::with_capacity(workers);
    // Linux distributes new flows across independent accept queues. Cloning one
    // listen socket lets a single ready worker consume all arrivals instead.
    #[cfg(target_os = "linux")]
    {
        let address = listener.local_addr()?;
        let original = socket2::SockRef::from(&listener);
        original.set_reuse_port(true)?;
        let only_v6 = address.is_ipv6().then(|| original.only_v6()).transpose()?;
        for _ in 1..workers {
            let socket = socket2::Socket::new(
                socket2::Domain::for_address(address),
                socket2::Type::STREAM,
                Some(socket2::Protocol::TCP),
            )?;
            socket.set_reuse_address(true)?;
            socket.set_reuse_port(true)?;
            if let Some(only_v6) = only_v6 {
                socket.set_only_v6(only_v6)?;
            }
            socket.set_nonblocking(true)?;
            socket.bind(&address.into())?;
            socket.listen(i32::from(backlog))?;
            listeners.push(socket.into());
        }
    }
    #[cfg(not(target_os = "linux"))]
    for _ in 1..workers {
        listeners.push(listener.try_clone()?);
    }
    listeners.push(listener);
    Ok(listeners)
}

struct StopOnDrop {
    signals: Vec<Arc<Signal>>,
    relay: Arc<Relay>,
}
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        // Also covers cancellation of the embedding serve future. Publish
        // notification suppression before any worker observes the stop signal.
        self.relay.begin_shutdown();
        for signal in &self.signals {
            signal.stop();
        }
    }
}

struct Worker {
    poll: Poll,
    registry: Arc<Registry>,
    listener: TcpListener,
    signal: Arc<Signal>,
    relay: Arc<Relay>,
    connections: Slab<Connection>,
    tokens: HashMap<Token, usize>,
    next_token: usize,
    ready: VecDeque<Token>,
    timers: Timers,
    scratch: ReadStorage,
    compacted: bool,
    accept_queued: bool,
    accept_retry: Option<Instant>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        // A fatal poll/accept error or panic also retires membership. Suppress
        // fanout before fields drop, even before the supervisor sees the error.
        self.relay.begin_shutdown();
    }
}

impl Worker {
    fn new(listener: std::net::TcpListener, relay: Arc<Relay>) -> io::Result<Self> {
        let poll = Poll::new()?;
        let registry = Arc::new(poll.registry().try_clone()?);
        let mut listener = TcpListener::from_std(listener);
        registry.register(&mut listener, LISTENER, Interest::READABLE)?;
        let signal = Arc::new(Signal {
            waker: mio::Waker::new(&registry, WAKE)?,
            stopping: AtomicBool::new(false),
            pending: parking_lot::Mutex::new(VecDeque::new()),
        });
        Ok(Self {
            poll,
            registry,
            listener,
            signal,
            relay,
            connections: Slab::new(),
            tokens: HashMap::new(),
            next_token: 2,
            ready: VecDeque::new(),
            timers: Timers::default(),
            scratch: ReadStorage::default(),
            compacted: false,
            accept_queued: false,
            accept_retry: None,
        })
    }

    fn schedule(&mut self, token: Token) {
        let Some(&slot) = self.tokens.get(&token) else {
            return;
        };
        if let Some(connection) = self.connections.get_mut(slot)
            && connection.token == token
        {
            // Input/timer readiness upgrades an already pending output wake.
            connection.output_only = false;
            if !connection.scheduled {
                connection.scheduled = true;
                self.ready.push_back(token);
            }
        }
    }

    fn schedule_accept(&mut self) {
        // Another listener edge may arrive before this continuation runs.
        // Keep one queued turn, as data sockets do with their scheduled flag.
        if !self.accept_queued {
            self.accept_queued = true;
            self.ready.push_back(LISTENER);
        }
    }

    fn accept(&mut self) -> io::Result<()> {
        if self.accept_retry.is_some() {
            return Ok(());
        }
        for _ in 0..ACCEPT_QUANTUM {
            let (stream, address) = match self.listener.accept() {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    self.relay.metrics.accept_errors.fetch_add(1, Relaxed);
                    let delay = crate::transport::accept_retry_delay(&error);
                    if delay.is_zero() {
                        continue;
                    }
                    return self.pause_accept(error, "accept", delay);
                }
                Ok(accepted) => accepted,
            };
            let metrics = &self.relay.metrics;
            metrics.accepted.fetch_add(1, Relaxed);
            if self.relay.is_draining()
                || metrics
                    .connections
                    .fetch_update(Relaxed, Relaxed, |count| {
                        (count < self.relay.config.max_connections).then_some(count + 1)
                    })
                    .is_err()
            {
                metrics.rejected.fetch_add(1, Relaxed);
                continue;
            }
            let mut guard = ConnectionGuard(metrics.clone(), None);
            // Local bindings drop in reverse declaration order. Keep this
            // descriptor ahead of its admission guard on every setup refusal,
            // just as Socket keeps its stream ahead of its guard after setup.
            let mut stream = stream;
            if let Some(limit) = &self.relay.ip_limit {
                let Some(permit) = limit.acquire(address.ip()) else {
                    metrics.rejected.fetch_add(1, Relaxed);
                    metrics.ip_rejected.fetch_add(1, Relaxed);
                    continue;
                };
                guard.1 = Some(permit);
            }
            if let Err(error) = stream.set_nodelay(true) {
                return self.setup_failed(error, "TCP_NODELAY");
            }
            let config = &self.relay.config;
            if config.tcp_send_buffer_bytes != 0
                && let Err(error) = socket2::SockRef::from(&stream)
                    .set_send_buffer_size(config.tcp_send_buffer_bytes)
            {
                return self.setup_failed(error, "SO_SNDBUF");
            }
            if config.tcp_receive_buffer_bytes != 0
                && let Err(error) = socket2::SockRef::from(&stream)
                    .set_recv_buffer_size(config.tcp_receive_buffer_bytes)
            {
                return self.setup_failed(error, "SO_RCVBUF");
            }
            let lease = match &metrics.read_budget {
                Some(budget) => {
                    match budget.try_lease(self.relay.config.handshake_read_reservation()) {
                        Ok(lease) => Some(lease),
                        Err(_) => {
                            metrics.rejected.fetch_add(1, Relaxed);
                            let _ = stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 0\r\n\r\n");
                            continue;
                        }
                    }
                }
                None => None,
            };
            // Tokens never repeat, even when slab slots are reused. Use the
            // full pointer width rather than spending half on a slot index:
            // on 64-bit hosts, heavy churn cannot exhaust a 32-bit generation.
            let token = Token(self.next_token);
            self.next_token = self
                .next_token
                .checked_add(1)
                .ok_or_else(|| io::Error::other("connection token space exhausted"))?;
            let slot = self.connections.vacant_key();
            let socket = match Socket::new(
                stream,
                token,
                self.registry.clone(),
                self.signal.clone(),
                guard,
                &self.relay,
            ) {
                Ok(socket) => socket,
                Err(error) => return self.setup_failed(error, "register socket"),
            };
            let connection = Connection::new(socket, token, lease, &self.relay);
            let deadline = connection.deadline(&self.relay);
            self.connections.insert(connection);
            self.tokens.insert(token, slot);
            self.timers.set(slot, deadline);
            self.schedule(token);
        }
        // EPOLLET requires an explicit continuation when we yield before
        // WouldBlock. The listener uses the same bounded ready queue.
        self.schedule_accept();
        Ok(())
    }

    fn setup_failed(&mut self, error: io::Error, operation: &'static str) -> io::Result<()> {
        self.relay.metrics.setup_errors.fetch_add(1, Relaxed);
        self.relay.metrics.rejected.fetch_add(1, Relaxed);
        self.pause_accept(error, operation, Duration::from_secs(1))
    }

    fn pause_accept(
        &mut self,
        error: io::Error,
        operation: &'static str,
        delay: Duration,
    ) -> io::Result<()> {
        // Stop readiness notifications during resource pressure. Existing
        // sockets, timers and shutdown remain serviced; retry drains queued TCP.
        self.registry.deregister(&mut self.listener)?;
        self.accept_retry = Some(Instant::now() + delay);
        tracing::warn!(%error, operation, "native admission failed; retrying in one second");
        Ok(())
    }

    fn reclaim_connections(&mut self) {
        if self.connections.is_empty() {
            self.scratch = ReadStorage::default();
            // Keep small arenas warm for ordinary reconnect traffic.
            if self.connections.capacity() > 256 || self.compacted {
                self.connections = Slab::new();
                self.tokens = HashMap::new();
                self.timers = Timers::default();
                self.compacted = false;
            }
        } else if self.connections.capacity() > 256
            && self.connections.len() <= self.connections.capacity() / 8
        {
            // Slots are private to this worker. Ready/pending events and the
            // poller carry stable tokens, while timers are rekeyed with each
            // move. Socket owners and shared payload allocations stay in place.
            self.connections.compact(|connection, from, to| {
                *self.tokens.get_mut(&connection.token).expect("live token") = to;
                self.timers.rekey(from, to);
                true
            });
            self.timers.shrink_to_fit(self.connections.len());
            self.compacted = true;
        }
    }

    fn run(mut self) -> Result<()> {
        let mut events = Events::with_capacity(1024);
        let mut shutdown_deadline = None;
        let relay = self.relay.clone();
        let mut incoming = relay.receive_batch();
        loop {
            if self.signal.stopping.load(Acquire) && shutdown_deadline.is_none() {
                if self.accept_retry.take().is_none() {
                    self.registry.deregister(&mut self.listener)?;
                }
                shutdown_deadline = Some(
                    Instant::now() + Duration::from_secs(self.relay.config.shutdown_timeout_secs),
                );
                for (_, connection) in self.connections.iter_mut() {
                    if !connection.scheduled {
                        connection.scheduled = true;
                        self.ready.push_back(connection.token);
                    }
                }
            }
            if shutdown_deadline.is_some_and(|d| d <= Instant::now())
                || (shutdown_deadline.is_some() && self.connections.is_empty())
            {
                break;
            }
            let now = Instant::now();
            if self.accept_retry.is_some_and(|deadline| deadline <= now) {
                self.accept_retry = None;
                self.registry
                    .register(&mut self.listener, LISTENER, Interest::READABLE)?;
                // Drain an already queued flow even without a fresh edge.
                self.accept()?;
            }
            while let Some(slot) = self.timers.pop_due(now) {
                if let Some(connection) = self.connections.get(slot) {
                    self.schedule(connection.token);
                }
            }
            // A bounded ready batch makes progress on buffered continuations
            // without starving new socket events, timers, or shutdown signals.
            let batch = self.ready.len().min(1024);
            for _ in 0..batch {
                let token = self.ready.pop_front().expect("ready batch");
                if token == LISTENER {
                    self.accept_queued = false;
                    if shutdown_deadline.is_none() {
                        self.accept()?;
                    }
                    continue;
                }
                let Some(&slot) = self.tokens.get(&token) else {
                    continue;
                };
                let Some(connection) = self.connections.get_mut(slot).filter(|c| c.token == token)
                else {
                    continue;
                };
                connection.scheduled = false;
                connection.socket.notified();
                let keep = connection.drive_batched(
                    &self.relay,
                    &mut self.scratch,
                    shutdown_deadline.is_some(),
                    &mut incoming,
                );
                match keep {
                    Drive::Drop => {
                        self.timers.remove(slot);
                        self.connections.remove(slot);
                        self.tokens.remove(&token);
                        if self.tokens.capacity() > 256
                            && self.tokens.len() <= self.tokens.capacity() / 8
                        {
                            self.tokens.shrink_to(self.tokens.len() * 2);
                        }
                        self.reclaim_connections();
                    }
                    Drive::Wait | Drive::Again => {
                        let deadline = self.connections[slot].deadline(&self.relay);
                        self.timers.set(slot, deadline);
                        if keep == Drive::Again {
                            // Retain the reason: a write-quantum continuation
                            // still needs no speculative input read.
                            self.connections[slot].scheduled = true;
                            self.ready.push_back(token);
                        }
                    }
                }
            }
            // Publish before polling, even when no more input will arrive.
            incoming.publish();
            {
                let mut pending = self.signal.pending.lock();
                while let Some(token) = pending.pop_front() {
                    let Some(&slot) = self.tokens.get(&token) else {
                        continue;
                    };
                    if let Some(connection) = self.connections.get_mut(slot)
                        && connection.token == token
                        && !connection.scheduled
                    {
                        connection.scheduled = true;
                        connection.output_only = true;
                        self.ready.push_back(token);
                    }
                }
            }
            if shutdown_deadline.is_some() && self.connections.is_empty() {
                break;
            }
            let now = Instant::now();
            let mut timeout = self.timers.next().map(|d| d.saturating_duration_since(now));
            for deadline in [shutdown_deadline, self.accept_retry].into_iter().flatten() {
                timeout = Some(
                    timeout.map_or(deadline.saturating_duration_since(now), |t| {
                        t.min(deadline.saturating_duration_since(now))
                    }),
                );
            }
            if !self.ready.is_empty() {
                timeout = Some(Duration::ZERO);
            }
            match self.poll.poll(&mut events, timeout) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            }
            for event in &events {
                match event.token() {
                    LISTENER if shutdown_deadline.is_none() && !self.accept_queued => {
                        self.accept()?
                    }
                    LISTENER | WAKE => {}
                    token => self.schedule(token),
                }
            }
        }
        Ok(())
    }
}

struct ConnectionGuard(
    Arc<crate::metrics::Metrics>,
    Option<crate::admission::IpPermit>,
);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Relaxed);
    }
}

struct UpgradePolicy(Arc<Relay>);
impl Callback for UpgradePolicy {
    fn on_request(
        self,
        request: &Request,
        mut response: Response,
    ) -> std::result::Result<Response, ErrorResponse> {
        if !self.0.config.origin_policy.allows(request.headers()) {
            self.0.metrics.rejected.fetch_add(1, Relaxed);
            self.0.metrics.origin_rejected.fetch_add(1, Relaxed);
            return Err(crate::origin::forbidden());
        }
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
    }
}

type Upgrade = MidHandshake<ServerHandshake<Stream, UpgradePolicy>>;
// Both phases keep their metadata inline in the connection slab. Input and
// frame buffers retain their own lifetimes across resumptions and slab moves.
enum Stage {
    Upgrade(Option<Upgrade>),
    WebSocket(WebSocket<Stream>),
}

struct Connection {
    token: Token,
    scheduled: bool,
    output_only: bool,
    room_fanout: Option<Box<RoomFanout>>,
    socket: Arc<Socket>,
    stage: Stage,
    handshake_lease: Option<ReadLease>,
    registration: Option<Registration>,
    phase_deadline: Instant,
    closing: Option<Closing>,
    limiter: RateLimiter,
    next_ping: Instant,
    pong_deadline: Option<Instant>,
    reclaim_deadline: Option<Instant>,
}

struct Closing {
    deadline: Instant,
    flush_only: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Drive {
    Drop,
    Wait,
    Again,
}

impl Connection {
    fn new(
        socket: Arc<Socket>,
        token: Token,
        lease: Option<ReadLease>,
        relay: &Arc<Relay>,
    ) -> Self {
        let config = &relay.config;
        let ws_config = WebSocketConfig::default()
            .read_buffer_size(config.read_buffer_bytes)
            .shared_read_buffer(true)
            .write_buffer_size(4096)
            .max_write_buffer_size(config.queue_bytes + 4096)
            .max_message_size(Some(config.max_message_bytes))
            .max_frame_size(Some(config.max_message_bytes.max(125)));
        let handshake = ServerHandshake::start(
            Stream {
                socket: socket.clone(),
                remaining_upgrade: Some(crate::config::MAX_HTTP_UPGRADE_BYTES),
            },
            UpgradePolicy(relay.clone()),
            Some(ws_config),
        );
        let now = Instant::now();
        Self {
            token,
            scheduled: false,
            output_only: false,
            room_fanout: None,
            socket,
            stage: Stage::Upgrade(Some(handshake)),
            handshake_lease: lease,
            registration: None,
            phase_deadline: now + Duration::from_secs(config.handshake_timeout_secs),
            closing: None,
            limiter: RateLimiter::new(config.messages_per_second, config.bytes_per_second, now),
            next_ping: now + Duration::from_secs(config.ping_interval_secs.max(1)),
            pong_deadline: None,
            reclaim_deadline: None,
        }
    }

    fn deadline(&self, relay: &Relay) -> Option<Instant> {
        if let Some(closing) = &self.closing {
            return Some(closing.deadline);
        }
        if matches!(self.stage, Stage::Upgrade(_)) || self.registration.is_none() {
            return Some(self.phase_deadline);
        }
        let mut deadline = self.socket.deadline();
        if let Some(next) = self.room_fanout.as_ref().and_then(|f| f.deadline()) {
            deadline = Some(deadline.map_or(next, |d| d.min(next)));
        }
        for next in [
            self.pong_deadline,
            self.reclaim_deadline,
            (relay.config.ping_interval_secs > 0).then_some(self.next_ping),
        ]
        .into_iter()
        .flatten()
        {
            deadline = Some(deadline.map_or(next, |d| d.min(next)));
        }
        if relay.config.idle_timeout_secs > 0 {
            let next = self.socket.state.lock().activity
                + Duration::from_secs(relay.config.idle_timeout_secs);
            deadline = Some(deadline.map_or(next, |d| d.min(next)));
        }
        deadline
    }

    #[cfg(test)]
    fn drive(&mut self, relay: &Arc<Relay>, scratch: &mut ReadStorage, stopping: bool) -> Drive {
        self.drive_batched(relay, scratch, stopping, &mut relay.receive_batch())
    }

    fn drive_batched(
        &mut self,
        relay: &Arc<Relay>,
        scratch: &mut ReadStorage,
        stopping: bool,
        incoming: &mut ReceiveBatch<'_>,
    ) -> Drive {
        let now = Instant::now();
        if self.output_only
            && !self.room_fanout.as_ref().is_some_and(|f| f.active())
            && !stopping
            && self.registration.is_some()
            && self.closing.is_none()
            && self.pong_deadline.is_none_or(|d| d > now)
            && self.reclaim_deadline.is_none_or(|d| d > now)
            && (relay.config.ping_interval_secs == 0 || self.next_ping > now)
            && let Some(result) = self
                .socket
                .flush_output_only(now, Duration::from_secs(relay.config.idle_timeout_secs))
        {
            return match result {
                Ok(true) => Drive::Again,
                Ok(false) => Drive::Wait,
                Err(_) => {
                    relay.metrics.write_errors.fetch_add(1, Relaxed);
                    Drive::Drop
                }
            };
        }
        self.output_only = false;
        if let Stage::Upgrade(handshake) = &mut self.stage {
            if stopping {
                return Drive::Drop;
            }
            if now >= self.phase_deadline {
                relay.metrics.handshake_errors.fetch_add(1, Relaxed);
                return Drive::Drop;
            }
            match handshake.take().expect("pending handshake").handshake() {
                Ok(mut ws) => {
                    ws.get_mut().remaining_upgrade = None;
                    if let Some(lease) = self.handshake_lease.take()
                        && ws.set_read_budget(lease).is_err()
                    {
                        return Drive::Drop;
                    }
                    if self.socket.control_flushed().is_err() {
                        return Drive::Drop;
                    }
                    self.phase_deadline = now + Duration::from_secs(relay.config.join_timeout_secs);
                    self.stage = Stage::WebSocket(ws);
                }
                Err(HandshakeError::Interrupted(mid)) => {
                    *handshake = Some(mid);
                    return Drive::Wait;
                }
                Err(HandshakeError::Failure(Error::Http(response))) if response.status() == 403 => {
                    return Drive::Drop;
                }
                Err(_) => {
                    relay.metrics.handshake_errors.fetch_add(1, Relaxed);
                    return Drive::Drop;
                }
            }
        }
        let Stage::WebSocket(ws) = &mut self.stage else {
            unreachable!()
        };
        ws.swap_read_storage(scratch);
        let result = self.drive_websocket(relay, now, stopping, incoming);
        let Stage::WebSocket(ws) = &mut self.stage else {
            unreachable!()
        };
        if let Some(mut storage) = ws.take_read_storage() {
            // A small message on another connection must not inherit and pin a
            // previous large receive slab. Refill lazily on the next read.
            storage.discard_if_larger_than(relay.config.read_buffer_bytes.max(14));
            *scratch = storage;
        }
        result
    }

    fn start_close(
        &mut self,
        reason: Disconnect,
        relay: &Relay,
        acknowledge: bool,
        now: Instant,
    ) -> Drive {
        if self.closing.is_some() {
            return Drive::Wait;
        }
        if let Some(fanout) = &mut self.room_fanout {
            fanout.clear();
        }
        if reason.code == 1002 && self.registration.is_some() {
            relay.metrics.protocol_errors.fetch_add(1, Relaxed);
        }
        if reason == Disconnect::RATE {
            relay.metrics.rate_limited.fetch_add(1, Relaxed);
        }
        if self.registration.is_none() && !acknowledge && reason != Disconnect::SHUTDOWN {
            relay.metrics.join_rejected.fetch_add(1, Relaxed);
        }
        // Remove membership before waiting on a peer's Close acknowledgement.
        self.registration.take();
        if self.socket.end_output() {
            return Drive::Drop;
        }
        let Stage::WebSocket(ws) = &mut self.stage else {
            return Drive::Drop;
        };
        self.closing = Some(Closing {
            deadline: now + relay.config.write_timeout(),
            flush_only: acknowledge,
        });
        if acknowledge {
            return self.finish_peer_close(relay);
        }
        match ws.close(Some(CloseFrame {
            code: CloseCode::from(reason.code),
            reason: reason.reason.into(),
        })) {
            Ok(()) => {}
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(error) => {
                tracing::debug!(token = self.token.0, %error, "Close send failed");
                relay.metrics.close_failures.fetch_add(1, Relaxed);
                return Drive::Drop;
            }
        }
        Drive::Again
    }

    /// A received Close completes the exchange once queued control bytes reach
    /// TCP. Flush in this turn; only actual backpressure needs another event.
    fn finish_peer_close(&mut self, relay: &Relay) -> Drive {
        self.closing.as_mut().expect("closing exchange").flush_only = true;
        let Stage::WebSocket(ws) = &mut self.stage else {
            unreachable!()
        };
        match ws.flush() {
            Ok(()) | Err(Error::ConnectionClosed | Error::AlreadyClosed) => Drive::Drop,
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => Drive::Wait,
            Err(error) => {
                tracing::debug!(token = self.token.0, %error, "Close acknowledgement flush failed");
                relay.metrics.close_failures.fetch_add(1, Relaxed);
                Drive::Drop
            }
        }
    }

    fn drive_websocket(
        &mut self,
        relay: &Arc<Relay>,
        now: Instant,
        stopping: bool,
        incoming: &mut ReceiveBatch<'_>,
    ) -> Drive {
        let (fatal, requested) = {
            let state = self.socket.state.lock();
            (state.fatal, state.requested)
        };
        if fatal {
            return Drive::Drop;
        }
        if self.closing.is_none() {
            if let Some(reason) = requested.or(stopping.then_some(Disconnect::SHUTDOWN)) {
                // A notification burst may fill an untouched queue while the
                // peer's Close is already readable. Preserve that handshake
                // before starting a competing overload Close. Partial output
                // still forces TCP retirement in start_close.
                if reason == Disconnect::OVERLOADED {
                    let Stage::WebSocket(ws) = &mut self.stage else {
                        unreachable!()
                    };
                    if matches!(
                        ws.read_with_frame_limit(
                            NonZeroUsize::new(READ_QUANTUM).expect("read quantum")
                        ),
                        Ok(Some(Message::Close(_)))
                    ) {
                        return self.start_close(
                            Disconnect {
                                code: 1000,
                                reason: "",
                            },
                            relay,
                            true,
                            now,
                        );
                    }
                }
                return self.start_close(reason, relay, false, now);
            }
            if self.socket.deadline().is_some_and(|d| d <= now)
                || self
                    .room_fanout
                    .as_ref()
                    .and_then(|f| f.deadline())
                    .is_some_and(|d| d <= now)
            {
                relay.metrics.write_timeouts.fetch_add(1, Relaxed);
                return Drive::Drop;
            }
            if self.registration.is_none() && now >= self.phase_deadline {
                return self.start_close(
                    Disconnect {
                        code: 1008,
                        reason: "Channel Join Timeout",
                    },
                    relay,
                    false,
                    now,
                );
            }
            if self.pong_deadline.is_some_and(|d| d <= now) {
                return self.start_close(
                    Disconnect {
                        code: 1001,
                        reason: "Heartbeat Timeout",
                    },
                    relay,
                    false,
                    now,
                );
            }
            if self.registration.is_some() && relay.config.idle_timeout_secs > 0 {
                let output_pending = self.socket.deadline().is_some();
                let mut state = self.socket.state.lock();
                if state.activity + Duration::from_secs(relay.config.idle_timeout_secs) <= now {
                    if !output_pending {
                        drop(state);
                        relay.metrics.idle_timeouts.fetch_add(1, Relaxed);
                        return self.start_close(Disconnect::IDLE, relay, false, now);
                    }
                    state.activity = now;
                }
            }
        }
        let mut again = match self.socket.flush() {
            Ok(again) => again,
            Err(_) => {
                relay.metrics.write_errors.fetch_add(1, Relaxed);
                return Drive::Drop;
            }
        };
        let Stage::WebSocket(ws) = &mut self.stage else {
            unreachable!()
        };
        match ws.flush() {
            Ok(()) => {
                if self.socket.control_flushed().is_err() {
                    return Drive::Drop;
                }
            }
            Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
            Err(Error::ConnectionClosed | Error::AlreadyClosed) => return Drive::Drop,
            Err(error) => {
                tracing::debug!(token = self.token.0, %error, closing = self.closing.is_some(), "control flush failed");
                relay.metrics.close_failures.fetch_add(1, Relaxed);
                return Drive::Drop;
            }
        }
        if let Some(closing) = &self.closing {
            if closing.deadline <= now {
                tracing::debug!(
                    token = self.token.0,
                    "Close acknowledgement deadline expired"
                );
                relay.metrics.close_failures.fetch_add(1, Relaxed);
                return Drive::Drop;
            }
            if closing.flush_only || stopping {
                // The peer's Close is already consumed. Reading this closed
                // decoder can terminate it while its reply is still buffered.
                // Only writable readiness or the Close deadline may finish it.
                return if self.socket.state.lock().control_pending {
                    Drive::Wait
                } else {
                    Drive::Drop
                };
            }
        }
        if self.closing.is_none()
            && self.registration.is_some()
            && relay.config.ping_interval_secs > 0
            && self.next_ping <= now
        {
            self.next_ping = now + Duration::from_secs(relay.config.ping_interval_secs);
            if self.pong_deadline.is_none() {
                match ws.send(Message::Ping(Bytes::from_static(b"war"))) {
                    Ok(()) => {
                        if self.socket.control_flushed().is_err() {
                            return Drive::Drop;
                        }
                    }
                    Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(_) => {
                        relay.metrics.write_errors.fetch_add(1, Relaxed);
                        return Drive::Drop;
                    }
                }
                self.pong_deadline =
                    Some(now + Duration::from_secs(relay.config.pong_timeout_secs));
            }
        }
        if self.reclaim_deadline.is_some_and(|d| d <= now) {
            if let Some(copied) = ws.reclaim_read_buffer() {
                relay.metrics.read_buffer_reclaims.fetch_add(1, Relaxed);
                relay
                    .metrics
                    .read_buffer_reclaim_header_bytes
                    .fetch_add(copied as u64, Relaxed);
                self.reclaim_deadline = None;
            } else {
                relay
                    .metrics
                    .read_buffer_reclaims_deferred
                    .fetch_add(1, Relaxed);
                self.reclaim_deadline =
                    Some(now + Duration::from_secs(relay.config.read_buffer_idle_secs));
            }
        }
        if let Some(fanout) = self.room_fanout.as_mut().filter(|f| f.active()) {
            return match fanout.resume(FANOUT_QUANTUM) {
                Ok(()) => Drive::Again,
                Err(reason) => self.start_close(reason, relay, false, now),
            };
        }
        let mut route_budget = READ_QUANTUM;
        for _ in 0..READ_QUANTUM {
            let Stage::WebSocket(ws) = &mut self.stage else {
                unreachable!()
            };
            match ws.read_with_frame_limit(NonZeroUsize::new(1).expect("one frame")) {
                Ok(None) => continue,
                Ok(Some(Message::Close(_))) => {
                    if self.closing.is_some() {
                        return self.finish_peer_close(relay);
                    }
                    return self.start_close(
                        Disconnect {
                            code: 1000,
                            reason: "",
                        },
                        relay,
                        true,
                        now,
                    );
                }
                Ok(Some(_)) if self.closing.is_some() => continue,
                Ok(Some(message)) => {
                    if !self.limiter.allow(message.len(), now) {
                        return self.start_close(Disconnect::RATE, relay, false, now);
                    }
                    match message {
                        Message::Binary(bytes) => {
                            if let Some(registration) = &self.registration {
                                let large = relay.config.read_buffer_idle_secs > 0
                                    && bytes.len() > relay.config.read_buffer_bytes.max(64 * 1024);
                                let capacity = ws.read_storage_capacity().max(bytes.len());
                                let work = match incoming.receive_bounded(
                                    &registration.peer,
                                    bytes,
                                    capacity,
                                    &mut self.room_fanout,
                                    FANOUT_QUANTUM,
                                ) {
                                    Ok(work) => work,
                                    Err(reason) => {
                                        return self.start_close(reason, relay, false, now);
                                    }
                                };
                                route_budget = route_budget.saturating_sub(work);
                                if large {
                                    self.reclaim_deadline = Some(
                                        now + Duration::from_secs(
                                            relay.config.read_buffer_idle_secs,
                                        ),
                                    );
                                }
                            } else {
                                let registration = protocol::join(&bytes).and_then(|channel| {
                                    relay.subscribe_with_output(channel, self.socket.clone())
                                });
                                match registration {
                                    Ok(registration) => {
                                        // The existing random session ID spreads the first
                                        // deadline without another entropy read or per-packet work.
                                        self.next_ping = now
                                            + initial_ping_delay(
                                                relay.config.ping_interval_secs,
                                                registration.peer.id.load(Relaxed),
                                            );
                                        self.registration = Some(registration);
                                    }
                                    Err(reason) => {
                                        return self.start_close(reason, relay, false, now);
                                    }
                                }
                            }
                            if relay.config.idle_timeout_secs > 0 {
                                self.socket.state.lock().activity = Instant::now();
                            }
                        }
                        Message::Ping(_) => match ws.flush() {
                            Ok(()) => {
                                if self.socket.control_flushed().is_err() {
                                    return Drive::Drop;
                                }
                            }
                            Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {}
                            Err(_) => {
                                relay.metrics.write_errors.fetch_add(1, Relaxed);
                                return Drive::Drop;
                            }
                        },
                        Message::Pong(bytes) => {
                            if bytes.as_ref() == b"war" {
                                self.pong_deadline = None;
                            }
                        }
                        _ => {
                            return self.start_close(
                                Disconnect {
                                    code: 1003,
                                    reason: "CBOR Binary Messages Required",
                                },
                                relay,
                                false,
                                now,
                            );
                        }
                    }
                }
                Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                    match self.socket.flush() {
                        Ok(ready) => again |= ready,
                        Err(_) => {
                            relay.metrics.write_errors.fetch_add(1, Relaxed);
                            return Drive::Drop;
                        }
                    }
                    return if again { Drive::Again } else { Drive::Wait };
                }
                Err(Error::ConnectionClosed | Error::AlreadyClosed) => return Drive::Drop,
                Err(
                    Error::Io(_)
                    | Error::Protocol(
                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                    ),
                ) => {
                    if self.registration.is_some() {
                        relay.metrics.transport_disconnects.fetch_add(1, Relaxed);
                    }
                    return Drive::Drop;
                }
                Err(error) => {
                    let reason = match error {
                        Error::Capacity(
                            tungstenite::error::CapacityError::ReadBufferBudgetExceeded,
                        ) => Disconnect {
                            code: 1013,
                            reason: "Input Memory Limit",
                        },
                        Error::Capacity(_) => Disconnect {
                            code: 1009,
                            reason: "Message Too Large",
                        },
                        _ => Disconnect::PROTOCOL,
                    };
                    let result = self.start_close(reason, relay, false, now);
                    // A framing/decoder rejection has no trustworthy input
                    // continuation. Flush its best-effort Close, then retire;
                    // do not reinterpret an acknowledgement as the denied body.
                    if let Some(closing) = &mut self.closing {
                        closing.flush_only = true;
                    }
                    return result;
                }
            }
            if route_budget == 0 {
                break;
            }
        }
        Drive::Again
    }
}

/// Spread a join wave within the first heartbeat interval; subsequent Pings
/// keep the configured cadence. An earlier first Ping never weakens detection.
fn initial_ping_delay(interval_secs: u64, session_id: u64) -> Duration {
    let period_ns = Duration::from_secs(interval_secs).as_nanos();
    let phase = if period_ns == 0 {
        0
    } else {
        u128::from(session_id) % period_ns
    };
    // The remainder cannot exceed the u64 session ID, even for huge intervals.
    Duration::from_nanos(phase as u64)
}

impl Drop for Connection {
    fn drop(&mut self) {
        // This may be the last strong peer/socket reference. Disable output
        // before retiring membership and releasing queued byte reservations.
        self.socket.end_output();
        self.room_fanout.take();
        self.registration.take();
    }
}

struct RateLimiter {
    rates: [u64; 2],
    tokens: [f64; 2],
    updated: Instant,
}
impl RateLimiter {
    fn new(messages: u64, bytes: u64, now: Instant) -> Self {
        Self {
            rates: [messages, bytes],
            tokens: [messages as f64, bytes as f64],
            updated: now,
        }
    }
    fn allow(&mut self, bytes: usize, now: Instant) -> bool {
        if self.rates == [0, 0] {
            return true;
        }
        let elapsed = now.duration_since(self.updated).as_secs_f64();
        self.updated = now;
        for (i, cost) in [1.0, bytes as f64].into_iter().enumerate() {
            let rate = self.rates[i] as f64;
            if rate == 0.0 {
                continue;
            }
            self.tokens[i] = (self.tokens[i] + elapsed * rate).min(rate);
            if self.tokens[i] < cost {
                return false;
            }
            self.tokens[i] -= cost;
        }
        true
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    #[test]
    fn first_heartbeat_phases_stay_within_interval_and_do_not_cluster_join_times() {
        for interval in [0, 1, 15, 86_400, u64::MAX] {
            for id in [0, 1, 1_000_000_000, u64::MAX] {
                let phase = super::initial_ping_delay(interval, id);
                if interval == 0 {
                    assert_eq!(phase, std::time::Duration::ZERO);
                } else {
                    assert!(phase < std::time::Duration::from_secs(interval));
                }
            }
        }
        let phases: std::collections::HashSet<_> = (0..15)
            .map(|second| super::initial_ping_delay(15, second * 1_000_000_000))
            .collect();
        assert_eq!(phases.len(), 15);
    }

    use super::*;

    #[test]
    fn worker_listeners_keep_an_embedders_ipv6_only_boundary() {
        let ipv4 = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = ipv4.local_addr().unwrap().port();
        let original = socket2::Socket::new(
            socket2::Domain::IPV6,
            socket2::Type::STREAM,
            Some(socket2::Protocol::TCP),
        )
        .unwrap();
        original.set_only_v6(true).unwrap();
        original.set_nonblocking(true).unwrap();
        original
            .bind(&std::net::SocketAddr::from((std::net::Ipv6Addr::UNSPECIFIED, port)).into())
            .unwrap();
        original.listen(128).unwrap();
        let listeners = worker_listeners(original.into(), 4, 173).unwrap();
        assert_eq!(listeners.len(), 4);
        for listener in listeners {
            assert!(socket2::SockRef::from(&listener).only_v6().unwrap());
            assert_eq!(listener.local_addr().unwrap().port(), port);
        }
    }
}
