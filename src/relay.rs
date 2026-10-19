use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering::Relaxed},
    },
};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use minicbor::Encoder;
use parking_lot::{Mutex, RwLock};
use serde::Serialize;
#[cfg(test)]
use tokio::sync::{mpsc, watch};

use crate::{
    Config,
    frame::Frame,
    metrics::Metrics,
    protocol::{
        self, BROADCAST, CHANNEL_MESSAGE, Command, DISCONNECT_MESSAGE, Disconnect, GLOBAL_CHANNEL,
        PRIVATE_MESSAGE,
    },
};

mod listing;

pub(crate) const FANOUT_QUANTUM: usize = 1024;

pub(crate) const MAX_CHANNEL_PAGE_SIZE: usize = 1000;

#[repr(u8)]
enum Admission {
    Open,
    Draining,
    Stopping,
}

/// The registry lock protects membership and ID ownership. Output submission is
/// nonblocking; the transport writes immediately or retains a shared frame.
pub struct Relay {
    pub config: Config,
    pub metrics: Arc<Metrics>,
    admission: AtomicU8,
    state: RwLock<Registry>,
    retirements: Mutex<VecDeque<Box<RoomFanout>>>,
    retirement_pending: AtomicBool,
    pub(crate) ip_limit: Option<Arc<crate::admission::IpLimit>>,
}

#[derive(Default)]
struct InputTotals {
    messages: u64,
    bytes: u64,
    reused: u64,
    copied: u64,
}

/// One worker publishes cumulative input statistics at its ready-batch boundary.
/// Admission and retained-byte gauges remain immediate, independent atomics.
pub(crate) struct ReceiveBatch<'a> {
    relay: &'a Relay,
    totals: InputTotals,
}

impl ReceiveBatch<'_> {
    #[cfg(test)]
    pub(crate) fn receive(
        &mut self,
        peer: &Peer,
        bytes: Bytes,
        storage_capacity: usize,
    ) -> Result<usize, Disconnect> {
        self.relay.receive_with_totals(
            peer,
            bytes,
            storage_capacity,
            &mut self.totals,
            deliver_room,
        )
    }

    /// Native workers keep one reusable continuation slot per large publisher.
    pub(crate) fn receive_bounded(
        &mut self,
        peer: &Peer,
        bytes: Bytes,
        storage_capacity: usize,
        pending: &mut Option<Box<RoomFanout>>,
        quantum: usize,
    ) -> Result<usize, Disconnect> {
        self.relay.receive_with_totals(
            peer,
            bytes,
            storage_capacity,
            &mut self.totals,
            |source, recipients, frame| {
                if recipients.len() <= quantum {
                    return deliver_room(source, recipients, frame);
                }
                let work = pending.get_or_insert_with(|| Box::new(RoomFanout::default()));
                work.begin(
                    source,
                    recipients,
                    frame,
                    self.relay.config.write_timeout(),
                    false,
                )?;
                work.resume(quantum)?;
                Ok(quantum)
            },
        )
    }

    pub(crate) fn publish(&mut self) {
        let totals = std::mem::take(&mut self.totals);
        let metrics = &self.relay.metrics;
        for (counter, value) in [
            (&metrics.incoming_messages, totals.messages),
            (&metrics.incoming_bytes, totals.bytes),
            (&metrics.routing_reused_bytes, totals.reused),
            (&metrics.routing_copied_bytes, totals.copied),
        ] {
            if value != 0 {
                counter.fetch_add(value, Relaxed);
            }
        }
    }
}

impl Drop for ReceiveBatch<'_> {
    fn drop(&mut self) {
        self.publish();
    }
}

struct Registry {
    users: HashMap<u64, Arc<Peer>>,
    channels: HashMap<Bytes, Channel>,
    // Only authenticated global members opting into each class belong here.
    // Routing visits eligible listeners, independent of the idle population.
    listeners: [Vec<Arc<Peer>>; 3],
    next_key: u64,
    sessions: usize,
}

impl Registry {
    fn set_listener_flags(&mut self, peer: &Peer, flags: u8) {
        let previous = peer.listener.load(Relaxed);
        peer.listener.store(flags, Relaxed);
        for (bit, listeners) in self.listeners.iter_mut().enumerate() {
            let mask = 1 << bit;
            if (previous ^ flags) & mask == 0 {
                continue;
            }
            if flags & mask != 0 {
                if let Some(member) = self.channels[GLOBAL_CHANNEL].members.get(&peer.key) {
                    listeners.push(member.clone());
                }
            } else if let Some(position) = listeners.iter().position(|p| p.key == peer.key) {
                listeners.swap_remove(position);
                if listeners.capacity() > 256 && listeners.len() <= listeners.capacity() / 8 {
                    listeners.shrink_to(listeners.len() * 2);
                }
            }
        }
    }

    fn listeners(&self, flag: u8) -> &[Arc<Peer>] {
        &self.listeners[flag.trailing_zeros() as usize]
    }
}

fn shrink_sparse_map<K: Eq + Hash, V>(map: &mut HashMap<K, V>) {
    // HashMap iteration scans spare buckets too. After substantial retirement,
    // release excess slots in ownership, membership and channel-name indexes.
    // Keep small maps and 2x headroom to avoid churn. Rehashing moves metadata;
    // peers, queues, name/variable bytes and shared bodies keep their storage.
    let remaining = map.len();
    let capacity = map.capacity();
    if capacity > 256 && remaining <= capacity / 8 {
        map.shrink_to(remaining * 2);
    }
}

#[derive(Default)]
struct Channel {
    members: HashMap<u64, Arc<Peer>>,
    // Membership metadata only. Uncaptured views update in place; a membership
    // change invalidates a captured view while its routing owners finish.
    broadcast: OnceLock<Arc<Vec<Arc<Peer>>>>,
    variables: HashMap<Bytes, Bytes>,
    variable_bytes: usize,
}

impl Channel {
    fn insert(&mut self, peer: Arc<Peer>) {
        if let Some(view) = self.broadcast.get_mut().and_then(Arc::get_mut) {
            // The registry write lock excludes new captures. A unique view
            // has no routing owner, so retain its allocation across churn.
            view.push(peer.clone());
        } else {
            // Active broadcasts keep their immutable membership snapshot.
            // Rebuild lazily rather than copying a view still in use.
            self.broadcast.take();
        }
        self.members.insert(peer.key, peer);
    }

    fn remove(&mut self, key: u64) -> bool {
        if self.members.remove(&key).is_none() {
            return false;
        }
        if let Some(view) = self.broadcast.get_mut().and_then(Arc::get_mut) {
            let index = view
                .iter()
                .position(|peer| peer.key == key)
                .expect("recipient view matches channel membership");
            view.swap_remove(index);
            // Bound spare handles in a retained room after a large wave.
            if view.capacity() > 256 && view.len() <= view.capacity() / 8 {
                view.shrink_to(view.len() * 2);
            }
        } else {
            self.broadcast.take();
        }
        true
    }
}

pub(crate) struct Peer {
    key: u64,
    pub id: AtomicU64,
    channel: Bytes,
    authenticated: AtomicBool,
    listener: AtomicU8,
    closing: AtomicBool,
    output: Arc<dyn PeerOutput>,
    budget: Arc<Budget>,
}

/// One immutable membership snapshot and one charged shared body. Publisher
/// slots reuse their box; retirement jobs belong to the shared work queue.
/// Empty slots retain no payload or peer ownership; metadata is bounded by the
/// admitted connection count and staged bodies share the normal queue budget.
#[derive(Default)]
pub(crate) struct RoomFanout {
    recipients: Option<Arc<Vec<Arc<Peer>>>>,
    packet: Option<Envelope>,
    cursor: usize,
    source: u64,
    snapshot_bytes: usize,
    deadline: Option<std::time::Instant>,
    retirement: bool,
}

impl RoomFanout {
    fn begin(
        &mut self,
        source: &Peer,
        recipients: Arc<Vec<Arc<Peer>>>,
        frame: Frame,
        timeout: std::time::Duration,
        retirement: bool,
    ) -> Result<(), Disconnect> {
        debug_assert!(!self.active());
        let packet = if retirement {
            source.charge(&frame)?
        } else {
            source.reserve(&frame)?
        }
        .ok_or(Disconnect::OVERLOADED)?
        .into_owned();
        // Account conservatively per active publisher, even if several share
        // this view. Churn can otherwise pin many distinct old membership views
        // outside the queue budget while workers advance their cursors.
        let snapshot_bytes = recipients
            .capacity()
            .checked_mul(std::mem::size_of::<Arc<Peer>>())
            .and_then(|n| {
                n.checked_add(
                    std::mem::size_of::<Vec<Arc<Peer>>>()
                        + 2 * std::mem::size_of::<usize>()
                        + if retirement {
                            std::mem::size_of::<Self>() + 2 * std::mem::size_of::<Box<Self>>()
                        } else {
                            0
                        },
                )
            })
            .ok_or(Disconnect::OVERLOADED)?;
        if packet
            .budget
            .metrics
            .queued_bytes
            .fetch_update(Relaxed, Relaxed, |used| {
                used.checked_add(snapshot_bytes)
                    .filter(|&next| next <= packet.budget.global_limit)
            })
            .is_err()
        {
            packet.budget.metrics.global_pressure.fetch_add(1, Relaxed);
            return Err(Disconnect::OVERLOADED);
        }
        self.retirement = retirement;
        self.snapshot_bytes = snapshot_bytes;
        self.packet = Some(packet);
        self.recipients = Some(recipients);
        self.cursor = 0;
        self.source = source.key;
        self.deadline = Some(std::time::Instant::now() + timeout);
        Ok(())
    }

    pub(crate) fn active(&self) -> bool {
        self.packet.is_some()
    }
    pub(crate) fn deadline(&self) -> Option<std::time::Instant> {
        self.deadline
    }
    pub(crate) fn clear(&mut self) {
        if self.snapshot_bytes != 0 {
            self.packet
                .as_ref()
                .expect("charged membership view")
                .budget
                .metrics
                .queued_bytes
                .fetch_sub(self.snapshot_bytes, Relaxed);
            self.snapshot_bytes = 0;
        }
        self.packet = None;
        self.recipients = None;
        self.deadline = None;
    }

    pub(crate) fn resume(&mut self, quantum: usize) -> Result<(), Disconnect> {
        if self.retirement {
            self.resume_with::<true, false>(quantum)
        } else {
            self.resume_with::<false, false>(quantum)
        }
    }

    fn resume_with<const RETIREMENT: bool, const BARRIER: bool>(
        &mut self,
        quantum: usize,
    ) -> Result<(), Disconnect> {
        let recipients = self.recipients.as_ref().expect("live fanout snapshot");
        let frame = &self.packet.as_ref().expect("charged fanout body").frame;
        let end = self.cursor.saturating_add(quantum).min(recipients.len());
        for recipient in &recipients[self.cursor..end] {
            if recipient.key != self.source {
                let result = if BARRIER {
                    recipient.enqueue_barrier(frame)
                } else {
                    recipient.enqueue_fanout(frame)
                };
                if !RETIREMENT {
                    result?;
                }
            }
        }
        self.cursor = end;
        if end == recipients.len() {
            self.clear();
        }
        Ok(())
    }
}

impl Drop for RoomFanout {
    fn drop(&mut self) {
        self.clear();
    }
}

fn deliver_room(
    source: &Peer,
    recipients: Arc<Vec<Arc<Peer>>>,
    frame: Frame,
) -> Result<usize, Disconnect> {
    let work = recipients.len().max(1);
    for recipient in recipients.iter() {
        if recipient.key != source.key {
            recipient.enqueue_fanout(&frame)?;
        }
    }
    Ok(work)
}

struct Budget {
    used: AtomicUsize,
    limit: usize,
    global_limit: usize,
    metrics: Arc<Metrics>,
}

/// Byte reservations survive dequeue and remain charged until the actual write
/// completes (or is cancelled). Dropping any queue or failed send releases them.
pub(crate) struct Envelope {
    pub frame: Frame,
    budget: Arc<Budget>,
}

/// A charged synchronous submission borrows its owners. Only deferred output
/// promotes it to an owned envelope; either form releases the same reservation.
pub(crate) struct Submission<'a> {
    pub frame: &'a Frame,
    budget: Option<&'a Arc<Budget>>,
}

impl Submission<'_> {
    pub(crate) fn into_owned(mut self) -> Envelope {
        let budget = self.budget.take().expect("live submission").clone();
        Envelope {
            frame: self.frame.clone(),
            budget,
        }
    }
}

impl Drop for Submission<'_> {
    fn drop(&mut self) {
        if let Some(budget) = self.budget {
            budget.release(self.frame.len());
        }
    }
}

impl Budget {
    fn release(&self, bytes: usize) {
        self.used.fetch_sub(bytes, Relaxed);
        self.metrics.queued_bytes.fetch_sub(bytes, Relaxed);
    }
}

/// A transport owns ordering and backpressure. Sending never waits for socket
/// readiness, and close only signals the owning I/O loop.
pub(crate) trait PeerOutput: Send + Sync {
    fn send(&self, packet: Submission<'_>) -> bool;
    fn close(&self, reason: Disconnect);

    /// Ordered control barriers may drain ready output before reserving a slot.
    fn send_barrier(&self, packet: Submission<'_>) -> bool {
        self.send(packet)
    }

    /// Native workers can advance retirement work independently of the source.
    fn supports_retirement(&self) -> bool {
        false
    }
    fn wake_retirement(&self) {}

    /// Multicast may defer to recipient owners for parallel, batched output.
    /// Simple/test transports retain their ordinary FIFO submission contract.
    fn send_fanout(&self, packet: Submission<'_>) -> bool {
        self.send(packet)
    }
}

#[cfg(test)]
struct AsyncOutput {
    outgoing: mpsc::Sender<Envelope>,
    close: watch::Sender<Option<Disconnect>>,
}

#[cfg(test)]
impl PeerOutput for AsyncOutput {
    fn send(&self, packet: Submission<'_>) -> bool {
        self.outgoing.try_send(packet.into_owned()).is_ok()
    }

    fn close(&self, reason: Disconnect) {
        self.close.send_replace(Some(reason));
    }
}

impl Drop for Envelope {
    fn drop(&mut self) {
        self.budget.release(self.frame.len());
    }
}

impl Peer {
    pub(crate) fn close(&self, reason: Disconnect) -> bool {
        if self.closing.swap(true, Relaxed) {
            return false;
        }
        self.output.close(reason);
        true
    }

    fn enqueue_frame(&self, frame: &Frame) -> Result<(), Disconnect> {
        self.enqueue::<false, false>(frame)
    }

    fn enqueue_fanout(&self, frame: &Frame) -> Result<(), Disconnect> {
        self.enqueue::<true, false>(frame)
    }

    fn enqueue_barrier(&self, frame: &Frame) -> Result<(), Disconnect> {
        self.enqueue::<false, true>(frame)
    }

    // Select the output method at compile time. Ordinary private submissions
    // carry no multicast flag, extra branch or different frame representation.
    #[inline(always)]
    fn enqueue<const FANOUT: bool, const BARRIER: bool>(
        &self,
        frame: &Frame,
    ) -> Result<(), Disconnect> {
        let Some(packet) = self.reserve(frame)? else {
            return Ok(());
        };
        let accepted = if BARRIER {
            self.output.send_barrier(packet)
        } else if FANOUT {
            self.output.send_fanout(packet)
        } else {
            self.output.send(packet)
        };
        if !accepted {
            self.overloaded();
        }
        Ok(())
    }

    #[inline(always)]
    fn reserve<'a>(&'a self, frame: &'a Frame) -> Result<Option<Submission<'a>>, Disconnect> {
        if self.closing.load(Relaxed) {
            return Ok(None);
        }
        self.charge(frame)
    }

    // Retirement notifications remain chargeable after the source has closed.
    fn charge<'a>(&'a self, frame: &'a Frame) -> Result<Option<Submission<'a>>, Disconnect> {
        let size = frame.len();
        if self
            .budget
            .used
            .fetch_update(Relaxed, Relaxed, |used| {
                used.checked_add(size)
                    .filter(|&next| next <= self.budget.limit)
            })
            .is_err()
        {
            self.overloaded();
            return Ok(None);
        }
        if self
            .budget
            .metrics
            .queued_bytes
            .fetch_update(Relaxed, Relaxed, |used| {
                used.checked_add(size)
                    .filter(|&next| next <= self.budget.global_limit)
            })
            .is_err()
        {
            self.budget.used.fetch_sub(size, Relaxed);
            self.budget.metrics.global_pressure.fetch_add(1, Relaxed);
            return Err(Disconnect::OVERLOADED);
        }
        Ok(Some(Submission {
            frame,
            budget: Some(&self.budget),
        }))
    }

    fn overloaded(&self) {
        if self.close(Disconnect::OVERLOADED) {
            self.budget.metrics.slow_consumers.fetch_add(1, Relaxed);
        }
    }
}

pub(crate) struct Registration {
    pub peer: Arc<Peer>,
    relay: Arc<Relay>,
}

impl Drop for Registration {
    fn drop(&mut self) {
        self.relay.remove(&self.peer);
    }
}

#[cfg(test)]
pub(crate) struct Subscription {
    pub registration: Registration,
    pub outgoing: mpsc::Receiver<Envelope>,
    pub close: watch::Receiver<Option<Disconnect>>,
}

#[derive(Serialize)]
pub struct ChannelInfo {
    pub name: Option<String>,
    pub name_base64: String,
    pub members: usize,
    pub variables: usize,
    pub variable_bytes: usize,
}

/// Registry names are bounded to 16 bytes. Inline inspection names hold small
/// metadata without per-name allocation or reference-count traffic.
#[derive(Clone, Copy)]
struct SnapshotName {
    bytes: [u8; 16],
    len: u8,
}

impl SnapshotName {
    fn new(name: &[u8]) -> Self {
        let mut bytes = [0; 16];
        bytes[..name.len()].copy_from_slice(name);
        Self {
            bytes,
            len: name.len() as u8,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }

    fn base64(&self) -> [u8; 24] {
        let mut encoded = [0; 24];
        STANDARD
            .encode_slice(self.as_bytes(), &mut encoded)
            .expect("16-byte channel names fit in 24 base64 bytes");
        encoded
    }
}

/// One coherent view owns only small names and numeric metadata. It keeps no
/// registry guard, peer or variable value alive while inspection formats output.
#[derive(Clone, Copy)]
pub(crate) struct ChannelSnapshot {
    name: SnapshotName,
    members: usize,
    variables: usize,
    variable_bytes: usize,
}

impl Serialize for ChannelSnapshot {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;

        let name = self.name.as_bytes();
        let base64 = self.name.base64();
        let encoded_len = name.len().div_ceil(3) * 4;
        let encoded = std::str::from_utf8(&base64[..encoded_len]).expect("base64 is ASCII");
        let mut row = serializer.serialize_struct("ChannelInfo", 5)?;
        row.serialize_field("name", &std::str::from_utf8(name).ok())?;
        row.serialize_field("name_base64", encoded)?;
        row.serialize_field("members", &self.members)?;
        row.serialize_field("variables", &self.variables)?;
        row.serialize_field("variable_bytes", &self.variable_bytes)?;
        row.end()
    }
}

impl Relay {
    pub fn new(config: Config) -> Result<Arc<Self>> {
        config.validate()?;
        // Verify entropy availability before listening, never silently use predictable IDs.
        let _ = random_id()?;
        let mut channels = HashMap::new();
        channels.insert(Bytes::from_static(GLOBAL_CHANNEL), Channel::default());
        let read_budget = (config.total_read_buffer_bytes > 0).then(|| {
            tokio_tungstenite::tungstenite::protocol::ReadBudget::new(
                config.total_read_buffer_bytes,
            )
        });
        Ok(Arc::new(Self {
            ip_limit: crate::admission::IpLimit::new(config.max_connections_per_ip),
            config,
            metrics: Arc::new(Metrics {
                read_budget,
                ..Metrics::default()
            }),
            admission: AtomicU8::new(Admission::Open as u8),
            retirements: Mutex::new(VecDeque::new()),
            retirement_pending: AtomicBool::new(false),
            state: RwLock::new(Registry {
                users: HashMap::new(),
                channels,
                listeners: Default::default(),
                next_key: 1,
                sessions: 0,
            }),
        }))
    }

    pub fn counts(&self) -> (usize, usize) {
        let state = self.state.read();
        (state.sessions, state.channels.len())
    }

    /// Manual drain and terminal shutdown both refuse new admission.
    pub fn is_draining(&self) -> bool {
        self.admission.load(Relaxed) != Admission::Open as u8
    }

    pub fn is_stopping(&self) -> bool {
        self.admission.load(Relaxed) == Admission::Stopping as u8
    }

    /// Set a reversible drain. Returns false when resuming a terminal shutdown;
    /// draining an already stopping relay remains an idempotent success.
    #[must_use]
    pub fn set_draining(&self, draining: bool) -> bool {
        self.admission
            .fetch_update(Relaxed, Relaxed, |state| {
                if state == Admission::Stopping as u8 {
                    draining.then_some(state)
                } else {
                    Some(if draining {
                        Admission::Draining as u8
                    } else {
                        Admission::Open as u8
                    })
                }
            })
            .is_ok()
    }

    pub(crate) fn begin_shutdown(&self) {
        self.admission.store(Admission::Stopping as u8, Relaxed);
        // Terminal shutdown sends no departure notices. Release listener views
        // once instead of searching them for every retiring global session.
        // Drop the registry guard before taking the retirement queue lock.
        self.state.write().listeners = Default::default();
        let mut pending = self.retirements.lock();
        pending.clear();
        self.retirement_pending.store(false, Relaxed);
    }

    fn channel_snapshot(&self) -> Vec<ChannelSnapshot> {
        let state = self.state.read();
        state
            .channels
            .iter()
            .map(|(name, channel)| ChannelSnapshot {
                name: SnapshotName::new(name),
                members: channel.members.len(),
                variables: channel.variables.len(),
                variable_bytes: channel.variable_bytes,
            })
            .collect()
    }

    fn channel_members_snapshot(&self) -> Vec<(SnapshotName, usize)> {
        let state = self.state.read();
        state
            .channels
            .iter()
            .map(|(name, channel)| (SnapshotName::new(name), channel.members.len()))
            .collect()
    }

    pub fn channels(&self) -> Vec<ChannelInfo> {
        let mut channels: Vec<_> = self
            .channel_snapshot()
            .into_iter()
            .map(|channel| ChannelInfo {
                name: std::str::from_utf8(channel.name.as_bytes())
                    .ok()
                    .map(str::to_owned),
                name_base64: STANDARD.encode(channel.name.as_bytes()),
                members: channel.members,
                variables: channel.variables,
                variable_bytes: channel.variable_bytes,
            })
            .collect();
        channels.sort_unstable_by(|a, b| a.name_base64.cmp(&b.name_base64));
        channels
    }

    pub(crate) fn channel_listing(&self) -> Vec<ChannelSnapshot> {
        let mut channels = self.channel_snapshot();
        // Base64 contains no zero bytes. Zero padding therefore preserves text
        // ordering, including empty names and keys that prefix another key.
        // Cache one bounded key per row while sorting, without another vector
        // of full metadata rows. Serialization encodes into a stack buffer.
        channels.sort_by_cached_key(|channel| channel.name.base64());
        channels
    }

    pub fn disconnect(&self, id: u64) -> bool {
        self.state.read().users.get(&id).is_some_and(|p| {
            p.close(Disconnect {
                code: 4000,
                reason: "Administrative Disconnect",
            })
        })
    }

    #[cfg(test)]
    pub(crate) fn subscribe(self: &Arc<Self>, channel: &[u8]) -> Result<Subscription, Disconnect> {
        let (outgoing, receiver) = mpsc::channel(self.config.queue_messages);
        let (close, close_receiver) = watch::channel(None);
        let registration =
            self.subscribe_with_output(channel, Arc::new(AsyncOutput { outgoing, close }))?;
        Ok(Subscription {
            registration,
            outgoing: receiver,
            close: close_receiver,
        })
    }

    pub(crate) fn subscribe_with_output(
        self: &Arc<Self>,
        channel: &[u8],
        output: Arc<dyn PeerOutput>,
    ) -> Result<Registration, Disconnect> {
        if channel.len() > 16 {
            return Err(Disconnect::CHANNEL_LENGTH);
        }
        let mut state = self.state.write();
        let existing = state.channels.get_key_value(channel);
        if self.is_draining()
            || state.sessions >= self.config.max_connections
            || (existing.is_none() && state.channels.len() >= self.config.max_channels)
            || existing.is_some_and(|(_, c)| c.members.len() >= self.config.max_channel_members)
        {
            self.metrics.rejected.fetch_add(1, Relaxed);
            return Err(Disconnect::OVERLOADED);
        }
        // Copy a bounded name once per channel. Peers in an existing channel
        // share its canonical storage instead of allocating another name.
        let channel =
            existing.map_or_else(|| Bytes::copy_from_slice(channel), |(name, _)| name.clone());
        let id = loop {
            let id = random_id().map_err(|_| Disconnect::OVERLOADED)?;
            if id != 0 && id != BROADCAST && !state.users.contains_key(&id) {
                break id;
            }
        };
        let key = state.next_key;
        state.next_key = key.checked_add(1).ok_or(Disconnect::OVERLOADED)?;
        let peer = Arc::new(Peer {
            key,
            id: AtomicU64::new(id),
            channel,
            authenticated: AtomicBool::new(false),
            listener: AtomicU8::new(0),
            closing: AtomicBool::new(false),
            output,
            budget: Arc::new(Budget {
                used: AtomicUsize::new(0),
                limit: self.config.queue_bytes,
                global_limit: self.config.total_queue_bytes,
                metrics: self.metrics.clone(),
            }),
        });
        // Publish only after the welcome packet is first in this session's FIFO.
        peer.enqueue_frame(&Frame::integer(id))?;
        state.users.insert(id, peer.clone());
        let channel = state.channels.entry(peer.channel.clone()).or_default();
        channel.insert(peer.clone());
        state.sessions += 1;
        self.metrics.joined.fetch_add(1, Relaxed);
        Ok(Registration {
            peer,
            relay: self.clone(),
        })
    }

    #[cfg(test)]
    pub(crate) fn receive(&self, peer: &Peer, bytes: Bytes) -> Result<(), Disconnect> {
        let capacity = self.config.read_buffer_bytes.max(bytes.len());
        self.receive_with_storage(peer, bytes, capacity).map(|_| ())
    }

    pub(crate) fn receive_batch(&self) -> ReceiveBatch<'_> {
        ReceiveBatch {
            relay: self,
            totals: InputTotals::default(),
        }
    }

    #[cfg(test)]
    pub(crate) fn receive_with_storage(
        &self,
        peer: &Peer,
        bytes: Bytes,
        storage_capacity: usize,
    ) -> Result<usize, Disconnect> {
        self.receive_batch().receive(peer, bytes, storage_capacity)
    }

    fn receive_with_totals<F>(
        &self,
        peer: &Peer,
        bytes: Bytes,
        storage_capacity: usize,
        totals: &mut InputTotals,
        room: F,
    ) -> Result<usize, Disconnect>
    where
        F: FnOnce(&Peer, Arc<Vec<Arc<Peer>>>, Frame) -> Result<usize, Disconnect>,
    {
        if peer.closing.load(Relaxed) {
            return Ok(1);
        }
        totals.messages = totals.messages.wrapping_add(1);
        totals.bytes = totals.bytes.wrapping_add(bytes.len() as u64);
        let (target, value) = protocol::parse(&bytes)?;
        if target == 0 {
            return self.control(peer, value).map(|_| 1);
        }
        let state = self.state.read();
        if peer.closing.load(Relaxed) {
            return Ok(1);
        }
        let private_recipient = if target == BROADCAST {
            if peer.channel.as_ref() == GLOBAL_CHANNEL
                && self.config.disable_global_broadcast
                && !peer.authenticated.load(Relaxed)
            {
                return Err(Disconnect {
                    code: 1008,
                    reason: "Global Broadcast Requires Authentication",
                });
            }
            None
        } else {
            match state.users.get(&target) {
                Some(recipient) => Some(recipient),
                None => return Ok(1),
            }
        };
        let value_start = bytes.len() - value.len();
        let compact = storage_capacity > self.config.read_buffer_bytes
            && bytes.len().saturating_mul(8) < storage_capacity
            && !bytes.is_unique();
        let frame = Frame::routed(peer.id.load(Relaxed), bytes, value_start, compact);
        let counter = if compact {
            &mut totals.copied
        } else {
            &mut totals.reused
        };
        *counter = counter.wrapping_add(frame.payload().len() as u64);
        if let Some(recipient) = private_recipient
            && state.listeners(PRIVATE_MESSAGE).is_empty()
        {
            // Lookup and sender identity form one registry snapshot. The
            // retained target owns its lifetime through the socket write.
            let recipient = recipient.clone();
            drop(state);
            return recipient.enqueue_frame(&frame).map(|_| 1);
        }
        if target == BROADCAST
            && peer.channel.as_ref() != GLOBAL_CHANNEL
            && state.listeners(CHANNEL_MESSAGE).is_empty()
        {
            let Some(channel) = state.channels.get(&peer.channel) else {
                return Ok(1);
            };
            let recipients = channel
                .broadcast
                .get_or_init(|| Arc::new(channel.members.values().cloned().collect()))
                .clone();
            drop(state);
            return room(peer, recipients, frame);
        }
        // Charge every fanout path by recipient visits, including global
        // observers. One complete message remains atomic with respect to this
        // source's FIFO; the connection yields before its next buffered frame.
        // Counts conservatively include the sender or duplicate primary target.
        let work = if target == BROADCAST {
            if peer.channel.as_ref() == GLOBAL_CHANNEL {
                state.users.len()
            } else {
                state
                    .channels
                    .get(&peer.channel)
                    .map_or(0, |c| c.members.len())
                    .saturating_add(state.listeners(CHANNEL_MESSAGE).len())
            }
        } else {
            state.listeners(PRIVATE_MESSAGE).len().saturating_add(1)
        };
        if target == BROADCAST {
            if peer.channel.as_ref() == GLOBAL_CHANNEL {
                for recipient in state.users.values() {
                    if recipient.key != peer.key {
                        recipient.enqueue_fanout(&frame)?;
                    }
                }
            } else {
                if let Some(channel) = state.channels.get(&peer.channel) {
                    for recipient in channel.members.values() {
                        if recipient.key != peer.key {
                            recipient.enqueue_fanout(&frame)?;
                        }
                    }
                }
                for recipient in state.listeners(CHANNEL_MESSAGE) {
                    recipient.enqueue_fanout(&frame)?;
                }
            }
        } else if let Some(recipient) = private_recipient {
            recipient.enqueue_fanout(&frame)?;
            for listener in state.listeners(PRIVATE_MESSAGE) {
                if listener.key != recipient.key {
                    listener.enqueue_fanout(&frame)?;
                }
            }
        }
        Ok(work.max(1))
    }

    fn control(&self, peer: &Peer, value: &[u8]) -> Result<(), Disconnect> {
        match protocol::command(value)? {
            Command::Authenticate(password) => {
                if protocol::secret_matches(self.config.auth_password.as_deref(), password) {
                    peer.authenticated.store(true, Relaxed);
                }
                peer.enqueue_frame(&Frame::reply_bool(0, peer.authenticated.load(Relaxed)))?;
            }
            Command::Listen(flags) => {
                if !peer.authenticated.load(Relaxed) {
                    return self.unauthorized();
                }
                if peer.channel.as_ref() == GLOBAL_CHANNEL {
                    let mut state = self.state.write();
                    if peer.closing.load(Relaxed) {
                        return Ok(());
                    }
                    if peer.listener.load(Relaxed) & DISCONNECT_MESSAGE != 0
                        && flags & DISCONNECT_MESSAGE == 0
                    {
                        // Older deferred notices must precede revocation's ACK.
                        // Match the registry -> retirement lock order used by
                        // removal; retirement output never reacquires registry.
                        self.flush_global_retirements();
                    }
                    state.set_listener_flags(peer, flags & 7);
                } else {
                    peer.listener.store(flags & 7, Relaxed);
                }
                peer.enqueue_frame(&Frame::reply_uint(1, u64::from(flags & 7)))?;
            }
            Command::Channels => {
                if !peer.authenticated.load(Relaxed) {
                    return self.unauthorized();
                }
                let mut channels = self.channel_members_snapshot();
                channels.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                // Names fit one-byte CBOR length headers; each entry adds an
                // array header and its full-width member count. Size detached
                // metadata after releasing the registry guard, then encode once.
                let capacity = channels
                    .iter()
                    .try_fold(
                        protocol::uint_len(channels.len() as u64),
                        |size, (name, members)| {
                            size.checked_add(
                                name.as_bytes().len() + 2 + protocol::uint_len(*members as u64),
                            )
                        },
                    )
                    .filter(|&size| size <= isize::MAX as usize)
                    .ok_or(Disconnect::OVERLOADED)?;
                let mut e = Encoder::new(Vec::with_capacity(capacity));
                e.array(channels.len() as u64).expect("Vec encoding");
                for (name, members) in channels {
                    e.array(2)
                        .and_then(|e| e.bytes(name.as_bytes()))
                        .and_then(|e| e.u64(members as u64))
                        .expect("Vec encoding");
                }
                peer.enqueue_frame(&Frame::reply(2, e.into_writer().into()))?;
            }
            Command::Assign(id) => {
                if !peer.authenticated.load(Relaxed) {
                    return self.unauthorized();
                }
                if id == 0 || id == BROADCAST {
                    return Err(Disconnect::RESERVED);
                }
                let mut state = self.state.write();
                let old = peer.id.load(Relaxed);
                // Takeover may have happened after receive's initial closing check.
                // The map entry must still belong to this immutable session key.
                if peer.closing.load(Relaxed)
                    || state
                        .users
                        .get(&old)
                        .is_none_or(|owner| owner.key != peer.key)
                {
                    return Ok(());
                }
                if old != id {
                    let owner = state.users.remove(&old).ok_or(Disconnect::PROTOCOL)?;
                    if let Some(previous) = state.users.insert(id, owner) {
                        previous.close(Disconnect::TAKEN);
                        shrink_sparse_map(&mut state.users);
                    }
                    peer.id.store(id, Relaxed);
                }
                peer.enqueue_frame(&Frame::reply_uint(3, id))?;
            }
            Command::SetVariable { key, value } => {
                let mut state = self.state.write();
                if peer.closing.load(Relaxed) {
                    return Ok(());
                }
                let channel = state
                    .channels
                    .get_mut(&peer.channel)
                    .ok_or(Disconnect::PROTOCOL)?;
                let previous_size = channel
                    .variables
                    .get(key)
                    .map_or(0, |v| key.len() + v.len());
                let next = channel.variable_bytes - previous_size + key.len() + value.len();
                let total = self.metrics.variable_bytes.load(Relaxed) - previous_size
                    + key.len()
                    + value.len();
                if next > self.config.max_variable_bytes
                    || total > self.config.total_variable_bytes
                    || (previous_size == 0 && channel.variables.len() >= self.config.max_variables)
                {
                    return Err(Disconnect::OVERLOADED);
                }
                // Copy the retained value, so a tiny variable cannot pin a large input frame.
                channel
                    .variables
                    .insert(Bytes::copy_from_slice(key), Bytes::copy_from_slice(value));
                channel.variable_bytes = next;
                self.metrics.variable_bytes.store(total, Relaxed);
                peer.enqueue_frame(&Frame::reply_bool(4, true))?;
            }
            Command::GetVariable(key) => {
                let value = self
                    .state
                    .read()
                    .channels
                    .get(&peer.channel)
                    .and_then(|c| c.variables.get(key))
                    .cloned();
                peer.enqueue_frame(&Frame::variable(value))?;
            }
        }
        Ok(())
    }

    fn unauthorized(&self) -> Result<(), Disconnect> {
        Err(Disconnect {
            code: 1008,
            reason: "Authentication Required",
        })
    }

    pub(crate) fn has_retirements(&self) -> bool {
        self.retirement_pending.load(Relaxed)
    }

    pub(crate) fn progress_retirements(&self) {
        // Serialize the retirement sequence, but never make another worker wait
        // for this turn. No registry lock is acquired during output submission.
        let Some(mut pending) = self.retirements.try_lock() else {
            return;
        };
        if let Some(work) = pending.front_mut() {
            let _ = work.resume(FANOUT_QUANTUM);
            if !work.active() {
                pending.pop_front();
            }
        }
        if pending.is_empty() && pending.capacity() > 32 {
            *pending = VecDeque::new();
        }
        self.retirement_pending.store(!pending.is_empty(), Relaxed);
    }

    fn flush_global_retirements(&self) {
        if !self.has_retirements() {
            return;
        }
        let mut pending = self.retirements.lock();
        // Every retirement view belongs to one channel: the departing peer's
        // room, or global disconnect observers. Only global views can precede
        // this observer's ACK. Removing them in order cannot reorder delivery
        // to ordinary rooms, whose disjoint views stay queued for bounded turns.
        pending.retain_mut(|work| {
            let global = work
                .recipients
                .as_ref()
                .expect("live retirement view")
                .first()
                .is_some_and(|peer| peer.channel.as_ref() == GLOBAL_CHANNEL);
            if global {
                let _ = work.resume_with::<true, true>(usize::MAX);
            }
            !global
        });
        if pending.is_empty() && pending.capacity() > 32 {
            *pending = VecDeque::new();
        }
        self.retirement_pending.store(!pending.is_empty(), Relaxed);
    }

    fn flush_retirements(&self) {
        if !self.has_retirements() {
            return;
        }
        // Budget refusal may require synchronous best-effort delivery. Finish
        // preceding retirements first so this fallback cannot overtake them.
        let mut pending = self.retirements.lock();
        for mut work in pending.drain(..) {
            let _ = work.resume(usize::MAX);
        }
        if pending.capacity() > 32 {
            *pending = VecDeque::new();
        }
        self.retirement_pending.store(false, Relaxed);
    }

    fn defer_retirement(
        &self,
        source: &Peer,
        recipients: Arc<Vec<Arc<Peer>>>,
        frame: &Frame,
    ) -> bool {
        if !source.output.supports_retirement() {
            return false;
        }
        let mut work = Box::<RoomFanout>::default();
        if work
            .begin(
                source,
                recipients,
                frame.clone(),
                self.config.write_timeout(),
                true,
            )
            .is_err()
        {
            return false;
        }
        // Registry removal serializes publication. One queue preserves that
        // sequence across workers, including causally ordered client closes.
        let mut pending = self.retirements.lock();
        if self.is_stopping() {
            return true;
        }
        let wake = pending.is_empty();
        pending.push_back(work);
        self.retirement_pending.store(true, Relaxed);
        drop(pending);
        if wake {
            source.output.wake_retirement();
        }
        true
    }

    fn remove(&self, peer: &Peer) {
        let mut state = self.state.write();
        let Some(channel) = state.channels.get_mut(&peer.channel) else {
            return;
        };
        if !channel.remove(peer.key) {
            return;
        }
        peer.closing.store(true, Relaxed);
        let id = peer.id.load(Relaxed);
        let notify = !self.is_stopping();
        let frame = notify.then(|| Frame::envelope(BROADCAST, protocol::uint(id)));
        if let Some(frame) = &frame {
            let mut deferred = false;
            if !channel.members.is_empty()
                && (channel.members.len() > FANOUT_QUANTUM || self.has_retirements())
            {
                let recipients = channel
                    .broadcast
                    .get_or_init(|| Arc::new(channel.members.values().cloned().collect()))
                    .clone();
                deferred = self.defer_retirement(peer, recipients, frame);
            }
            if !deferred && !channel.members.is_empty() {
                self.flush_retirements();
                for recipient in channel.members.values() {
                    let _ = recipient.enqueue_fanout(frame);
                }
            }
        }
        if channel.members.is_empty() && peer.channel.as_ref() != GLOBAL_CHANNEL {
            self.metrics
                .variable_bytes
                .fetch_sub(channel.variable_bytes, Relaxed);
            state.channels.remove(&peer.channel);
            shrink_sparse_map(&mut state.channels);
        } else {
            // Empty ordinary rooms drop the complete map; retained rooms and
            // the persistent global room can release spare membership slots.
            shrink_sparse_map(&mut channel.members);
        }
        if peer.channel.as_ref() == GLOBAL_CHANNEL {
            state.set_listener_flags(peer, 0);
        }
        if notify && peer.channel.as_ref() != GLOBAL_CHANNEL {
            let frame = frame.as_ref().expect("notifications enabled");
            let listeners = state.listeners(DISCONNECT_MESSAGE);
            let deferred = !listeners.is_empty()
                && (listeners.len() > FANOUT_QUANTUM || self.has_retirements())
                && self.defer_retirement(peer, Arc::new(listeners.to_vec()), frame);
            if !deferred && !listeners.is_empty() {
                self.flush_retirements();
                for recipient in listeners {
                    let _ = recipient.enqueue_fanout(frame);
                }
            }
        }
        // A displaced session must never erase the new owner of its previous ID.
        if state
            .users
            .get(&id)
            .is_some_and(|owner| owner.key == peer.key)
        {
            state.users.remove(&id);
            shrink_sparse_map(&mut state.users);
        }
        state.sessions -= 1;
        self.metrics.disconnected.fetch_add(1, Relaxed);
    }
}

fn random_id() -> Result<u64> {
    let mut bytes = [0; 8];
    getrandom::fill(&mut bytes).context("operating system entropy unavailable")?;
    Ok(u64::from_ne_bytes(bytes))
}

#[cfg(test)]
mod broadcast_tests;
#[cfg(test)]
mod channel_tests;
#[cfg(test)]
mod listener_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_batch_defers_only_cumulative_statistics_and_publishes_once() {
        let relay = Relay::new(config()).unwrap();
        let mut sender = relay.subscribe(b"batch").unwrap();
        let mut recipient = relay.subscribe(b"batch").unwrap();
        sender.outgoing.try_recv().unwrap();
        recipient.outgoing.try_recv().unwrap();
        let bytes = protocol::envelope(recipient.registration.peer.id.load(Relaxed), &[0xf5]);
        let mut batch = relay.receive_batch();
        batch
            .receive(&sender.registration.peer, bytes.clone(), 4096)
            .unwrap();
        let held = recipient.outgoing.try_recv().unwrap();
        assert_eq!(relay.metrics.incoming_messages.load(Relaxed), 0);
        assert_eq!(relay.metrics.routing_reused_bytes.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
        assert_eq!(
            recipient.registration.peer.budget.used.load(Relaxed),
            held.frame.len()
        );
        assert_eq!(
            batch.receive(&sender.registration.peer, Bytes::from_static(b"\xff"), 4096),
            Err(Disconnect::PROTOCOL)
        );
        batch.publish();
        batch.publish();
        assert_eq!(relay.metrics.incoming_messages.load(Relaxed), 2);
        assert_eq!(
            relay.metrics.incoming_bytes.load(Relaxed),
            bytes.len() as u64 + 1
        );
        assert_eq!(relay.metrics.routing_reused_bytes.load(Relaxed), 1);
        batch
            .receive(&sender.registration.peer, bytes.clone(), 4096)
            .unwrap();
        drop(batch);
        assert_eq!(relay.metrics.incoming_messages.load(Relaxed), 3);
        assert_eq!(
            relay.metrics.incoming_bytes.load(Relaxed),
            bytes.len() as u64 * 2 + 1
        );
        assert_eq!(relay.metrics.routing_reused_bytes.load(Relaxed), 2);
        drop((sender, recipient));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
        drop(held);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn input_batches_keep_instance_identity_and_publish_during_unwind() {
        let first = Relay::new(config()).unwrap();
        let second = Relay::new(config()).unwrap();
        let mut one = first.subscribe(b"batch").unwrap();
        let mut two = second.subscribe(b"batch").unwrap();
        one.outgoing.try_recv().unwrap();
        two.outgoing.try_recv().unwrap();
        assert!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut a = first.receive_batch();
                let mut b = second.receive_batch();
                a.receive(
                    &one.registration.peer,
                    protocol::envelope(one.registration.peer.id.load(Relaxed), &[0xf5]),
                    4096,
                )
                .unwrap();
                b.receive(
                    &two.registration.peer,
                    protocol::envelope(two.registration.peer.id.load(Relaxed), &[0xf5]),
                    4096,
                )
                .unwrap();
                b.publish();
                assert_eq!(first.metrics.incoming_messages.load(Relaxed), 0);
                assert_eq!(second.metrics.incoming_messages.load(Relaxed), 1);
                panic!("interrupted ready batch");
            }))
            .is_err()
        );
        assert_eq!(first.metrics.incoming_messages.load(Relaxed), 1);
        assert_eq!(second.metrics.incoming_messages.load(Relaxed), 1);
        drop((one, two));
        assert_eq!(first.metrics.queued_bytes.load(Relaxed), 0);
        assert_eq!(second.metrics.queued_bytes.load(Relaxed), 0);
    }

    fn config() -> Config {
        Config {
            auth_password: Some("test-password".into()),
            ..Config::default()
        }
    }

    #[test]
    fn variable_reply_keeps_stored_bytes_after_replacement_and_channel_retirement() {
        let relay = Relay::new(config()).unwrap();
        let mut sub = relay.subscribe(b"variables").unwrap();
        sub.outgoing.try_recv().unwrap();
        let value = protocol::byte_string(&vec![0xa5; 32768]);
        let mut input = b"\x82\x44body".to_vec();
        input.extend_from_slice(&value);
        relay
            .receive(&sub.registration.peer, protocol::request(4, &input))
            .unwrap();
        sub.outgoing.try_recv().unwrap();
        let pointer = relay.state.read().channels[b"variables".as_slice()].variables
            [b"body".as_slice()]
        .as_ptr();
        relay
            .receive(&sub.registration.peer, protocol::request(5, b"\x44body"))
            .unwrap();
        let held = sub.outgoing.try_recv().unwrap();
        assert_eq!(held.frame.payload().as_ptr(), pointer);
        assert_eq!(held.frame.payload(), value.as_ref());
        assert_eq!(held.frame.prefix(), b"\x82\x00\x82\x18\xc8\x82\xf5");
        relay
            .receive(
                &sub.registration.peer,
                protocol::request(4, b"\x82\x44body\xf5"),
            )
            .unwrap();
        sub.outgoing.try_recv().unwrap();
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 5);
        drop(sub);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
        assert_eq!(held.frame.payload().as_ptr(), pointer);
        assert_eq!(held.frame.payload(), value.as_ref());
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
        drop(held);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn fanout_shares_the_received_value_and_keeps_it_alive_after_receive_returns() {
        let relay = Relay::new(config()).unwrap();
        let mut sender = relay.subscribe(b"a").unwrap();
        let mut first = relay.subscribe(b"a").unwrap();
        let mut second = relay.subscribe(b"a").unwrap();
        for subscription in [&mut sender, &mut first, &mut second] {
            subscription.outgoing.try_recv().unwrap();
        }
        let value = protocol::byte_string(&[0x5a; 2048]);
        let input = protocol::envelope(BROADCAST, &value);
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        relay.receive(&sender.registration.peer, input).unwrap();
        let first_frame = first.outgoing.try_recv().unwrap();
        let second_frame = second.outgoing.try_recv().unwrap();
        assert_eq!(first_frame.frame.payload().as_ptr(), pointer);
        assert_eq!(second_frame.frame.payload().as_ptr(), pointer);
        assert_eq!(first_frame.frame.payload(), value.as_ref());
        assert_eq!(
            relay.metrics.routing_reused_bytes.load(Relaxed),
            value.len() as u64
        );
        assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
        drop(sender);
        drop(first);
        drop(second);
        // In-flight envelopes still own their byte reservations and receive storage.
        assert_eq!(first_frame.frame.payload(), value.as_ref());
        assert!(relay.metrics.queued_bytes.load(Relaxed) > 0);
        drop(first_frame);
        drop(second_frame);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn tiny_views_of_a_large_current_allocation_are_compacted() {
        let relay = Relay::new(config()).unwrap();
        let mut sender = relay.subscribe(b"a").unwrap();
        let mut recipient = relay.subscribe(b"a").unwrap();
        sender.outgoing.try_recv().unwrap();
        recipient.outgoing.try_recv().unwrap();
        let small = protocol::envelope(recipient.registration.peer.id.load(Relaxed), &[0xf6]);
        let mut read_slab = bytes::BytesMut::with_capacity(65536);
        read_slab.extend_from_slice(&small);
        let input = read_slab.split().freeze();
        let backing = input.clone();
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        relay
            .receive_with_storage(&sender.registration.peer, input, 65536)
            .unwrap();
        let packet = recipient.outgoing.try_recv().unwrap();
        assert_ne!(packet.frame.payload().as_ptr(), pointer);
        assert_eq!(packet.frame.payload(), [0xf6]);
        assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 1);
        assert!(!backing.is_empty());
        drop(packet);
        drop(sender);
        drop(recipient);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn slow_consumer_is_closed_and_every_byte_reservation_is_released() {
        let relay = Relay::new(Config {
            queue_messages: 1,
            ..config()
        })
        .unwrap();
        let sender = relay.subscribe(b"a").unwrap();
        let recipient = relay.subscribe(b"a").unwrap();
        // Recipient queue still holds its welcome; the next routed frame exceeds its message budget.
        relay
            .receive(
                &sender.registration.peer,
                protocol::envelope(BROADCAST, &[1]),
            )
            .unwrap();
        assert_eq!(*recipient.close.borrow(), Some(Disconnect::OVERLOADED));
        assert_eq!(relay.metrics.slow_consumers.load(Relaxed), 1);
        drop(recipient);
        drop(sender);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn takeover_cleanup_cannot_remove_new_id_owner() {
        let relay = Relay::new(config()).unwrap();
        let mut first = relay.subscribe(b"a").unwrap();
        let mut next = relay.subscribe(b"a").unwrap();
        first.outgoing.try_recv().unwrap();
        next.outgoing.try_recv().unwrap();
        let id = first.registration.peer.id.load(Relaxed);
        let new_peer = &next.registration.peer;
        relay
            .receive(
                new_peer,
                protocol::request(0, &protocol::text("test-password")),
            )
            .unwrap();
        relay
            .receive(new_peer, protocol::request(3, &protocol::uint(id)))
            .unwrap();
        assert_eq!(*first.close.borrow(), Some(Disconnect::TAKEN));
        drop(first);
        assert!(
            relay
                .state
                .read()
                .users
                .get(&id)
                .is_some_and(|p| p.key == new_peer.key)
        );
        drop(next);
        assert_eq!(relay.counts(), (0, 1));
    }

    #[test]
    fn sparse_id_registry_rehash_preserves_owners_and_in_flight_payloads() {
        let relay = Relay::new(config()).unwrap();
        let mut old = relay.subscribe(b"old").unwrap();
        let mut owner = relay.subscribe(b"owner").unwrap();
        let mut source = relay.subscribe(b"source").unwrap();
        for subscription in [&mut old, &mut owner, &mut source] {
            subscription.outgoing.try_recv().unwrap();
        }
        let id = old.registration.peer.id.load(Relaxed);
        let source_id = source.registration.peer.id.load(Relaxed);
        relay
            .receive(
                &owner.registration.peer,
                protocol::request(0, &protocol::text("test-password")),
            )
            .unwrap();
        relay
            .receive(
                &owner.registration.peer,
                protocol::request(3, &protocol::uint(id)),
            )
            .unwrap();
        assert_eq!(*old.close.borrow(), Some(Disconnect::TAKEN));
        owner.outgoing.try_recv().unwrap();
        owner.outgoing.try_recv().unwrap();
        let fillers: Vec<_> = (0..1024)
            .map(|i| {
                let mut subscription = relay.subscribe(format!("burst-{i}").as_bytes()).unwrap();
                subscription.outgoing.try_recv().unwrap();
                subscription
            })
            .collect();
        let peak_capacity = relay.state.read().users.capacity();
        assert!(peak_capacity >= 1026);
        let value = protocol::byte_string(&[0x5a; 8192]);
        let input = protocol::envelope(id, &value);
        let payload_pointer = protocol::parse(&input).unwrap().1.as_ptr();
        relay.receive(&source.registration.peer, input).unwrap();
        let packet = owner.outgoing.try_recv().unwrap();
        assert_eq!(packet.frame.payload().as_ptr(), payload_pointer);
        drop(fillers);
        {
            let state = relay.state.read();
            assert_eq!(state.users.len(), 2);
            assert!(state.users.capacity() <= 256);
            assert!(state.users.capacity() < peak_capacity / 4);
            assert!(Arc::ptr_eq(&state.users[&id], &owner.registration.peer));
            assert!(Arc::ptr_eq(
                &state.users[&source_id],
                &source.registration.peer
            ));
        }
        // The displaced owner is still registered. Its delayed cleanup cannot
        // erase the replacement after rehashing the ownership table.
        drop(old);
        assert!(Arc::ptr_eq(
            &relay.state.read().users[&id],
            &owner.registration.peer
        ));
        relay
            .receive(
                &owner.registration.peer,
                protocol::envelope(source_id, &[0x18, 42]),
            )
            .unwrap();
        let reply = source.outgoing.try_recv().unwrap();
        assert_eq!(
            [reply.frame.prefix(), reply.frame.payload()].concat(),
            protocol::envelope(id, &[0x18, 42])
        );
        drop(reply);
        drop(source);
        drop(owner);
        assert_eq!(relay.counts(), (0, 1));
        assert!(relay.state.read().users.is_empty());
        assert_eq!(packet.frame.payload().as_ptr(), payload_pointer);
        assert_eq!(packet.frame.payload(), value.as_ref());
        assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), packet.frame.len());
        drop(packet);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn takeovers_reclaim_sparse_id_slots_before_displaced_sessions_finish() {
        let relay = Relay::new(config()).unwrap();
        let mut subscriptions: Vec<_> = (0..1024)
            .map(|i| {
                let mut sub = relay.subscribe(format!("owner-{i}").as_bytes()).unwrap();
                sub.outgoing.try_recv().unwrap();
                sub
            })
            .collect();
        let id = subscriptions[0].registration.peer.id.load(Relaxed);
        assert!(relay.state.read().users.capacity() >= 1024);
        for i in 1..subscriptions.len() {
            let peer = subscriptions[i].registration.peer.clone();
            relay
                .receive(
                    &peer,
                    protocol::request(0, &protocol::text("test-password")),
                )
                .unwrap();
            subscriptions[i].outgoing.try_recv().unwrap();
            relay
                .receive(&peer, protocol::request(3, &protocol::uint(id)))
                .unwrap();
            subscriptions[i].outgoing.try_recv().unwrap();
            assert_eq!(
                *subscriptions[i - 1].close.borrow(),
                Some(Disconnect::TAKEN)
            );
        }
        {
            let state = relay.state.read();
            assert_eq!(state.sessions, 1024);
            assert_eq!(state.users.len(), 1);
            assert!(state.users.capacity() <= 256);
            assert!(Arc::ptr_eq(
                &state.users[&id],
                &subscriptions.last().unwrap().registration.peer
            ));
        }
        // All displaced registrations still exist until this point.
        drop(subscriptions);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn sparse_membership_keeps_peers_variables_and_published_payloads_alive() {
        for room in [b"kept-room".as_slice(), GLOBAL_CHANNEL] {
            let relay = Relay::new(config()).unwrap();
            let mut source = relay.subscribe(room).unwrap();
            let mut target = relay.subscribe(room).unwrap();
            source.outgoing.try_recv().unwrap();
            target.outgoing.try_recv().unwrap();
            let source_id = source.registration.peer.id.load(Relaxed);
            let target_id = target.registration.peer.id.load(Relaxed);
            let mut argument = Encoder::new(Vec::new());
            argument
                .array(2)
                .unwrap()
                .bytes(b"kept")
                .unwrap()
                .bool(true)
                .unwrap();
            relay
                .receive(
                    &source.registration.peer,
                    protocol::request(4, &argument.into_writer()),
                )
                .unwrap();
            source.outgoing.try_recv().unwrap();
            let variable_pointer =
                relay.state.read().channels[room].variables[b"kept".as_slice()].as_ptr();
            let fillers: Vec<_> = (0..1024)
                .map(|_| {
                    let mut sub = relay.subscribe(room).unwrap();
                    sub.outgoing.try_recv().unwrap();
                    sub
                })
                .collect();
            let peak_capacity = relay.state.read().channels[room].members.capacity();
            assert!(peak_capacity >= 1026);
            // Exercise real administrative close signals. Registrations remain
            // until cleanup, while healthy peers keep their output queues.
            for sub in &fillers {
                assert!(relay.disconnect(sub.registration.peer.id.load(Relaxed)));
            }
            let value = protocol::byte_string(&[0x5a; 8192]);
            let input = protocol::envelope(target_id, &value);
            let pointer = protocol::parse(&input).unwrap().1.as_ptr();
            relay.receive(&source.registration.peer, input).unwrap();
            let held = target.outgoing.try_recv().unwrap();
            assert_eq!(held.frame.payload().as_ptr(), pointer);
            for sub in fillers {
                let id = sub.registration.peer.id.load(Relaxed);
                drop(sub);
                let expected = protocol::envelope(BROADCAST, &protocol::uint(id));
                for survivor in [&mut source, &mut target] {
                    let departure = survivor.outgoing.try_recv().unwrap();
                    assert_eq!(departure.frame.to_vec(), expected.as_ref());
                }
            }
            {
                let state = relay.state.read();
                let channel = &state.channels[room];
                assert_eq!(channel.members.len(), 2);
                assert!(channel.members.capacity() <= 256);
                assert!(channel.members.capacity() < peak_capacity / 4);
                assert!(Arc::ptr_eq(
                    &channel.members[&source.registration.peer.key],
                    &source.registration.peer
                ));
                assert!(Arc::ptr_eq(
                    &channel.members[&target.registration.peer.key],
                    &target.registration.peer
                ));
                assert_eq!(
                    channel.variables[b"kept".as_slice()].as_ptr(),
                    variable_pointer
                );
                assert_eq!(channel.variables[b"kept".as_slice()].as_ref(), &[0xf5]);
            }
            assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 5);
            relay
                .receive(
                    &source.registration.peer,
                    protocol::envelope(target_id, &[0x18, 42]),
                )
                .unwrap();
            let after = target.outgoing.try_recv().unwrap();
            assert_eq!(
                [after.frame.prefix(), after.frame.payload()].concat(),
                protocol::envelope(source_id, &[0x18, 42])
            );
            drop(after);
            drop(source);
            drop(target);
            assert_eq!(relay.counts(), (0, 1));
            assert_eq!(
                relay.metrics.variable_bytes.load(Relaxed),
                if room == GLOBAL_CHANNEL { 5 } else { 0 }
            );
            assert_eq!(held.frame.payload().as_ptr(), pointer);
            assert_eq!(held.frame.payload(), value.as_ref());
            assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
            assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
            drop(held);
            assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        }
    }

    #[test]
    fn sparse_channel_registry_preserves_names_variables_and_published_storage() {
        let relay = Relay::new(config()).unwrap();
        let mut source = relay.subscribe(b"\xff\0").unwrap();
        let mut target = relay.subscribe(b"kept").unwrap();
        let mut global = relay.subscribe(GLOBAL_CHANNEL).unwrap();
        for sub in [&mut source, &mut target, &mut global] {
            sub.outgoing.try_recv().unwrap();
            relay
                .receive(
                    &sub.registration.peer,
                    protocol::request(4, b"\x82\x44kept\x18\x2a"),
                )
                .unwrap();
            sub.outgoing.try_recv().unwrap();
        }
        relay
            .receive(
                &source.registration.peer,
                protocol::request(0, &protocol::text("test-password")),
            )
            .unwrap();
        source.outgoing.try_recv().unwrap();
        let expected =
            b"\x82\x00\x82\x02\x83\x82\x44kept\x01\x82\x48re_globl\x01\x82\x42\xff\0\x01";
        let names = [b"\xff\0".as_slice(), b"kept", GLOBAL_CHANNEL];
        let pointers: Vec<_> = {
            let state = relay.state.read();
            names
                .iter()
                .map(|name| {
                    let (key, channel) = state.channels.get_key_value(*name).unwrap();
                    (key.as_ptr(), channel.variables[b"kept".as_slice()].as_ptr())
                })
                .collect()
        };
        let fillers: Vec<_> = (0..1024)
            .map(|i| {
                let mut sub = relay.subscribe(format!("room-{i}").as_bytes()).unwrap();
                sub.outgoing.try_recv().unwrap();
                sub
            })
            .collect();
        let peak_capacity = relay.state.read().channels.capacity();
        assert!(peak_capacity >= 1027);
        let value = protocol::byte_string(&[0x5a; 8192]);
        let input = protocol::envelope(target.registration.peer.id.load(Relaxed), &value);
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        relay.receive(&source.registration.peer, input).unwrap();
        let held = target.outgoing.try_recv().unwrap();
        drop(fillers);
        {
            let state = relay.state.read();
            assert_eq!(state.channels.len(), 3);
            assert!(state.channels.capacity() <= 256);
            assert!(state.channels.capacity() < peak_capacity / 4);
            for ((name, (key_pointer, value_pointer)), sub) in
                names.iter().zip(&pointers).zip([&source, &target, &global])
            {
                let (key, channel) = state.channels.get_key_value(*name).unwrap();
                assert_eq!(key.as_ptr(), *key_pointer);
                assert_eq!(
                    channel.variables[b"kept".as_slice()].as_ptr(),
                    *value_pointer
                );
                assert!(Arc::ptr_eq(
                    &channel.members[&sub.registration.peer.key],
                    &sub.registration.peer
                ));
            }
        }
        // The two inspection interfaces intentionally sort by different keys:
        // raw channel bytes on CBOR, base64 text on the management listener.
        relay
            .receive(&source.registration.peer, protocol::request(2, &[0xf6]))
            .unwrap();
        assert_eq!(source.outgoing.try_recv().unwrap().frame.to_vec(), expected);
        assert_eq!(
            serde_json::to_value(relay.channels()).unwrap(),
            serde_json::json!([
                {"name":null,"name_base64":"/wA=","members":1,"variables":1,"variable_bytes":6},
                {"name":"kept","name_base64":"a2VwdA==","members":1,"variables":1,"variable_bytes":6},
                {"name":"re_globl","name_base64":"cmVfZ2xvYmw=","members":1,"variables":1,"variable_bytes":6}
            ])
        );
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 18);
        drop(global);
        drop(source);
        drop(target);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 6);
        let mut rejoined = relay.subscribe(b"kept").unwrap();
        rejoined.outgoing.try_recv().unwrap();
        relay
            .receive(
                &rejoined.registration.peer,
                protocol::request(5, b"\x44kept"),
            )
            .unwrap();
        assert_eq!(
            rejoined.outgoing.try_recv().unwrap().frame.to_vec(),
            b"\x82\x00\x82\x18\xc8\x82\xf4\xf6"
        );
        drop(rejoined);
        let mut rejoined = relay.subscribe(GLOBAL_CHANNEL).unwrap();
        rejoined.outgoing.try_recv().unwrap();
        relay
            .receive(
                &rejoined.registration.peer,
                protocol::request(5, b"\x44kept"),
            )
            .unwrap();
        assert_eq!(
            rejoined.outgoing.try_recv().unwrap().frame.to_vec(),
            b"\x82\x00\x82\x18\xc8\x82\xf5\x18\x2a"
        );
        drop(rejoined);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(held.frame.payload().as_ptr(), pointer);
        assert_eq!(held.frame.payload(), value.as_ref());
        assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
        drop(held);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn channel_snapshots_own_metadata_and_leave_the_registry_writable() {
        let relay = Relay::new(config()).unwrap();
        let name = b"\xff\0".as_slice();
        let mut source = relay.subscribe(name).unwrap();
        let mut target = relay.subscribe(name).unwrap();
        source.outgoing.try_recv().unwrap();
        target.outgoing.try_recv().unwrap();
        relay
            .receive(
                &source.registration.peer,
                protocol::request(4, b"\x82\x45ready\xf5"),
            )
            .unwrap();
        source.outgoing.try_recv().unwrap();
        let name_pointer = relay
            .state
            .read()
            .channels
            .get_key_value(name)
            .unwrap()
            .0
            .as_ptr();
        let peer = Arc::downgrade(&source.registration.peer);
        let value = protocol::byte_string(&[0x5a; 8192]);
        let input = protocol::envelope(target.registration.peer.id.load(Relaxed), &value);
        let body_pointer = protocol::parse(&input).unwrap().1.as_ptr();
        relay.receive(&source.registration.peer, input).unwrap();
        let held = target.outgoing.try_recv().unwrap();

        let snapshot = relay.channel_snapshot();
        assert!(
            relay.state.try_write().is_some(),
            "snapshot retained the guard"
        );
        let captured = snapshot.iter().find(|c| c.name.as_bytes() == name).unwrap();
        assert_ne!(captured.name.as_bytes().as_ptr(), name_pointer);
        let members = relay.channel_members_snapshot();
        assert!(relay.state.try_write().is_some());
        assert_eq!(
            members.iter().find(|c| c.0.as_bytes() == name).unwrap().1,
            2
        );
        assert_eq!(
            (
                captured.members,
                captured.variables,
                captured.variable_bytes
            ),
            (2, 1, 6)
        );
        let mut joined = relay.subscribe(name).unwrap();
        joined.outgoing.try_recv().unwrap();
        relay
            .receive(
                &joined.registration.peer,
                protocol::request(4, b"\x82\x45ready\x18\x2a"),
            )
            .unwrap();
        joined.outgoing.try_recv().unwrap();
        assert_eq!(
            relay
                .channels()
                .iter()
                .find(|c| c.name.is_none())
                .unwrap()
                .members,
            3
        );
        assert_eq!(
            (
                captured.members,
                captured.variables,
                captured.variable_bytes
            ),
            (2, 1, 6)
        );
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 7);
        drop(source);
        drop(target);
        drop(joined);
        assert!(peer.upgrade().is_none(), "snapshot retained a peer");
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
        assert_eq!(captured.name.as_bytes(), name);
        assert_eq!(
            members.iter().find(|c| c.0.as_bytes() == name).unwrap().1,
            2
        );
        assert_eq!(captured.members, 2);
        assert_eq!(held.frame.payload().as_ptr(), body_pointer);
        assert_eq!(held.frame.payload(), value.as_ref());
        assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
        drop(held);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn global_queue_pressure_refuses_producer_without_closing_unrelated_recipient() {
        let relay = Relay::new(Config {
            max_message_bytes: 32,
            queue_bytes: 64,
            total_queue_bytes: 64,
            ..config()
        })
        .unwrap();
        let mut sender = relay.subscribe(b"a").unwrap();
        let mut busy = relay.subscribe(b"b").unwrap();
        let mut empty = relay.subscribe(b"c").unwrap();
        sender.outgoing.try_recv().unwrap();
        busy.outgoing.try_recv().unwrap();
        empty.outgoing.try_recv().unwrap();
        let payload = protocol::byte_string(&[0; 20]);
        let busy_id = busy.registration.peer.id.load(Relaxed);
        let empty_id = empty.registration.peer.id.load(Relaxed);
        for _ in 0..2 {
            relay
                .receive(
                    &sender.registration.peer,
                    protocol::envelope(busy_id, &payload),
                )
                .unwrap();
        }
        assert_eq!(
            relay.receive(
                &sender.registration.peer,
                protocol::envelope(empty_id, &payload)
            ),
            Err(Disconnect::OVERLOADED)
        );
        assert!(!empty.registration.peer.closing.load(Relaxed));
        assert!(!busy.registration.peer.closing.load(Relaxed));
        assert!(relay.metrics.queued_bytes.load(Relaxed) <= 64);
        drop(sender);
        drop(busy);
        drop(empty);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn global_variable_budget_is_released_by_channel_cleanup() {
        let relay = Relay::new(Config {
            max_variable_bytes: 16,
            total_variable_bytes: 16,
            ..config()
        })
        .unwrap();
        let first = relay.subscribe(b"first").unwrap();
        let second = relay.subscribe(b"second").unwrap();
        let mut e = Encoder::new(Vec::new());
        e.array(2)
            .unwrap()
            .str("x")
            .unwrap()
            .bytes(&[0; 8])
            .unwrap();
        let request = protocol::request(4, &e.into_writer());
        relay
            .receive(&first.registration.peer, request.clone())
            .unwrap();
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 10);
        assert_eq!(
            relay.receive(&second.registration.peer, request.clone()),
            Err(Disconnect::OVERLOADED)
        );
        drop(first);
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
        relay.receive(&second.registration.peer, request).unwrap();
        drop(second);
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
    }

    #[test]
    fn synchronous_submission_borrows_owners_and_releases_charged_bytes() {
        struct BorrowProbe;
        impl PeerOutput for BorrowProbe {
            fn send(&self, packet: Submission<'_>) -> bool {
                let budget = packet.budget.expect("charged submission");
                assert_eq!(
                    Arc::strong_count(budget),
                    1,
                    "synchronous budget was cloned"
                );
                assert_eq!(budget.used.load(Relaxed), packet.frame.len());
                assert_eq!(
                    budget.metrics.queued_bytes.load(Relaxed),
                    packet.frame.len()
                );
                if !packet.frame.payload().is_empty() {
                    assert!(
                        packet.frame.has_unique_payload(),
                        "synchronous body was cloned"
                    );
                }
                true
            }
            fn close(&self, _: Disconnect) {}
        }
        let relay = Relay::new(config()).unwrap();
        let registration = relay
            .subscribe_with_output(b"borrow", Arc::new(BorrowProbe))
            .unwrap();
        let frame = Frame::envelope(1, Bytes::from(vec![0; 64]));
        registration.peer.enqueue_frame(&frame).unwrap();
        assert!(frame.has_unique_payload());
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }

    #[test]
    fn deferred_submission_promotes_same_body_and_holds_reservation_through_retirement() {
        let relay = Relay::new(config()).unwrap();
        let mut subscription = relay.subscribe(b"defer").unwrap();
        drop(subscription.outgoing.try_recv().unwrap());
        let frame = Frame::envelope(1, Bytes::from(vec![0; 64]));
        let pointer = frame.payload().as_ptr();
        subscription
            .registration
            .peer
            .enqueue_frame(&frame)
            .unwrap();
        let held = subscription.outgoing.try_recv().unwrap();
        assert_eq!(held.frame.payload().as_ptr(), pointer);
        assert_eq!(Arc::strong_count(&held.budget), 2);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), frame.len());
        drop(subscription);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), frame.len());
        drop(held);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        assert!(frame.has_unique_payload());
    }
}

#[cfg(test)]
mod continuation_tests;
