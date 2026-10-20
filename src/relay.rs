//! Routing registry with fine-grained locking.
//!
//! No single lock covers the whole relay. Session ownership lives in a sharded
//! map, each channel owns its own membership lock and departure queue, global
//! observers have a separate lock, and admission counters are atomics. No
//! socket write ever happens while a registry lock is held: output is either
//! submitted after the guard drops or queued for the owning worker.
use std::{
    collections::{HashMap, VecDeque},
    hash::Hash,
    sync::{
        Arc, OnceLock, Weak,
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
mod messages;
use messages::{MessageFanouts, Publication};

pub(crate) const FANOUT_QUANTUM: usize = 1024;
// Bound ordinary synchronous removal by the same visit budget as native
// retirement progress, while keeping live message fanout independently sized.
pub(crate) const RETIREMENT_QUANTUM: usize = 128;

pub(crate) const MAX_CHANNEL_PAGE_SIZE: usize = 1000;

/// Session IDs are random 64-bit values, so the top bits of a multiplicative
/// hash spread ownership lookups evenly across independent locks.
const USER_SHARDS: usize = 64;

/// An immutable recipient view shared between the cache and in-flight work.
type Recipients = Arc<Vec<Arc<Peer>>>;

#[repr(u8)]
enum Admission {
    Open,
    Draining,
    Stopping,
}

/// The relay. Every field is independently synchronized; see the module notes.
pub struct Relay {
    pub config: Config,
    pub metrics: Arc<Metrics>,
    admission: AtomicU8,
    users: Users,
    channels: RwLock<HashMap<Bytes, Arc<Channel>>>,
    /// Incremented on every membership change. A global view built against an
    /// older generation is used once and never cached.
    membership_generation: AtomicU64,
    global_view: Mutex<Option<(u64, Recipients)>>,
    listeners: RwLock<Listeners>,
    /// Bits of listener classes that currently have at least one member, so
    /// the ordinary routing path can skip the listener lock entirely.
    listener_mask: AtomicU8,
    disconnect_snapshot: Mutex<Weak<RetirementSnapshot>>,
    retirements: Retirements,
    /// Disconnect-observer notices and `re_globl` room departures share one
    /// queue: both reach `re_globl` members, and one order keeps barriers simple.
    observer_departures: Arc<DepartureQueue>,
    /// Live fanouts whose source retired before they finished.
    detached: Arc<DepartureQueue>,
    messages: Arc<MessageFanouts>,
    sessions: AtomicUsize,
    next_key: AtomicU64,
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

/// Routing errors retain attempted recipient work for native worker fairness.
/// Validation and preparation refusals count as one attempt, never a full turn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RoutingFailure {
    pub reason: Disconnect,
    pub work: usize,
}

impl RoutingFailure {
    fn visited(reason: Disconnect, work: usize) -> Self {
        Self {
            reason,
            work: work.max(1),
        }
    }

    fn after(mut self, preceding: usize) -> Self {
        self.work = self.work.saturating_add(preceding);
        self
    }
}

impl From<Disconnect> for RoutingFailure {
    fn from(reason: Disconnect) -> Self {
        Self::visited(reason, 1)
    }
}

impl ReceiveBatch<'_> {
    #[cfg(test)]
    pub(crate) fn receive(
        &mut self,
        peer: &Peer,
        bytes: Bytes,
        storage_capacity: usize,
    ) -> Result<usize, Disconnect> {
        self.relay
            .receive_with_totals(peer, bytes, storage_capacity, &mut self.totals, None)
            .map_err(|error| error.reason)
    }

    /// Native workers keep one reusable continuation slot per large publisher.
    pub(crate) fn receive_bounded(
        &mut self,
        peer: &Peer,
        bytes: Bytes,
        storage_capacity: usize,
        pending: &mut Option<Box<RoomFanout>>,
        quantum: usize,
    ) -> Result<usize, RoutingFailure> {
        self.relay.receive_with_totals(
            peer,
            bytes,
            storage_capacity,
            &mut self.totals,
            Some((pending, quantum)),
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

// ---------------------------------------------------------------------------
// Session ownership: sharded ID -> peer map.

struct Users {
    shards: Vec<RwLock<HashMap<u64, Arc<Peer>>>>,
}

impl Users {
    fn new() -> Self {
        Self {
            shards: (0..USER_SHARDS)
                .map(|_| RwLock::new(HashMap::new()))
                .collect(),
        }
    }

    fn index(id: u64) -> usize {
        (id.wrapping_mul(0x9E37_79B9_7F4A_7C15) >> (64 - USER_SHARDS.trailing_zeros())) as usize
    }

    fn shard(&self, id: u64) -> &RwLock<HashMap<u64, Arc<Peer>>> {
        &self.shards[Self::index(id)]
    }

    fn get(&self, id: u64) -> Option<Arc<Peer>> {
        self.shard(id).read().get(&id).cloned()
    }

    fn try_insert(&self, id: u64, peer: &Arc<Peer>) -> bool {
        let mut shard = self.shard(id).write();
        if shard.contains_key(&id) {
            return false;
        }
        shard.insert(id, peer.clone());
        true
    }

    /// Remove `id` only while `key` still owns it. A displaced session must
    /// never erase the new owner of its previous ID.
    fn remove_owned(&self, id: u64, key: u64) {
        let mut shard = self.shard(id).write();
        if shard.get(&id).is_some_and(|owner| owner.key == key) {
            shard.remove(&id);
            shrink_sparse_map(&mut shard);
        }
    }

    /// Move ownership of `old` to `new` atomically with respect to routing.
    /// Returns the displaced owner of `new`, or `None` when `peer` no longer
    /// owns `old`.
    fn reassign(&self, peer: &Peer, old: u64, new: u64) -> Option<Option<Arc<Peer>>> {
        let (a, b) = (Self::index(old), Self::index(new));
        let (mut first, mut second);
        let (from, to) = if a == b {
            first = self.shards[a].write();
            let shard = &mut *first;
            if shard.get(&old).is_none_or(|owner| owner.key != peer.key) {
                return None;
            }
            let owner = shard.remove(&old).expect("verified owner");
            let previous = shard.insert(new, owner);
            shrink_sparse_map(shard);
            return Some(previous);
        } else if a < b {
            first = self.shards[a].write();
            second = self.shards[b].write();
            (&mut *first, &mut *second)
        } else {
            second = self.shards[b].write();
            first = self.shards[a].write();
            (&mut *first, &mut *second)
        };
        if from.get(&old).is_none_or(|owner| owner.key != peer.key) {
            return None;
        }
        let owner = from.remove(&old).expect("verified owner");
        shrink_sparse_map(from);
        Some(to.insert(new, owner))
    }

    #[cfg(test)]
    fn capacity(&self) -> usize {
        self.shards
            .iter()
            .map(|shard| shard.read().capacity())
            .sum()
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.shards.iter().map(|shard| shard.read().len()).sum()
    }
}

// ---------------------------------------------------------------------------
// Channels.

/// One room. Membership, variables and the cached recipient view share one
/// lock; departure jobs have their own queue so a large room's notifications
/// never delay another room's.
pub(crate) struct Channel {
    pub(crate) name: Bytes,
    state: RwLock<ChannelState>,
    /// Admission reservations, taken before the welcome is sent and released
    /// on removal, so a refused join never receives a welcome.
    reserved: AtomicUsize,
    departures: Arc<DepartureQueue>,
}

#[derive(Default)]
struct ChannelState {
    members: HashMap<u64, Arc<Peer>>,
    // Membership metadata only. Uncaptured views update in place; a membership
    // change invalidates a captured view while its routing owners finish.
    broadcast: OnceLock<Arc<Vec<Arc<Peer>>>>,
    variables: HashMap<Bytes, Bytes>,
    variable_bytes: usize,
    /// Set once the empty room leaves the index. A joiner that raced the
    /// retirement creates a fresh room instead of resurrecting this one.
    retired: bool,
}

impl Channel {
    fn new(name: Bytes) -> Arc<Self> {
        Arc::new(Self {
            name,
            state: RwLock::new(ChannelState::default()),
            reserved: AtomicUsize::new(0),
            departures: Arc::new(DepartureQueue::default()),
        })
    }

    #[cfg(test)]
    pub(crate) fn cached_view(&self) -> Option<Arc<Vec<Arc<Peer>>>> {
        self.state.read().broadcast.get().cloned()
    }

    #[cfg(test)]
    pub(crate) fn members_len(&self) -> usize {
        self.state.read().members.len()
    }

    #[cfg(test)]
    pub(crate) fn members_capacity(&self) -> usize {
        self.state.read().members.capacity()
    }

    #[cfg(test)]
    pub(crate) fn member(&self, key: u64) -> Option<Arc<Peer>> {
        self.state.read().members.get(&key).cloned()
    }

    #[cfg(test)]
    pub(crate) fn variable(&self, key: &[u8]) -> Option<Bytes> {
        self.state.read().variables.get(key).cloned()
    }

    #[cfg(test)]
    pub(crate) fn departure_queue(&self) -> &Arc<DepartureQueue> {
        &self.departures
    }
}

impl ChannelState {
    fn insert(&mut self, peer: Arc<Peer>) {
        if let Some(view) = self.broadcast.get_mut().and_then(Arc::get_mut) {
            // The write lock excludes new captures. A unique view has no
            // routing owner, so retain its allocation across churn.
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

    fn view(&self) -> Arc<Vec<Arc<Peer>>> {
        self.broadcast
            .get_or_init(|| Arc::new(self.members.values().cloned().collect()))
            .clone()
    }
}

// ---------------------------------------------------------------------------
// Global observers.

#[derive(Default)]
struct Listeners {
    // Only authenticated global members opting into each class belong here.
    // Routing visits eligible listeners, independent of the idle population.
    lists: [Vec<Arc<Peer>>; 3],
    message_snapshots: [OnceLock<Arc<Vec<Arc<Peer>>>>; 2],
}

impl Listeners {
    fn of(&self, flag: u8) -> &[Arc<Peer>] {
        &self.lists[flag.trailing_zeros() as usize]
    }

    fn mask(&self) -> u8 {
        self.lists
            .iter()
            .enumerate()
            .filter(|(_, list)| !list.is_empty())
            .fold(0, |mask, (bit, _)| mask | (1 << bit))
    }

    /// Returns whether the disconnect class changed.
    fn set_flags(&mut self, peer: &Peer, flags: u8, member: Option<&Arc<Peer>>) -> bool {
        let previous = peer.listener.load(Relaxed);
        peer.listener.store(flags, Relaxed);
        for (bit, list) in self.lists.iter_mut().enumerate() {
            let mask = 1 << bit;
            if (previous ^ flags) & mask == 0 {
                continue;
            }
            if bit < 2 {
                self.message_snapshots[bit].take();
            }
            if flags & mask != 0 {
                if let Some(member) = member {
                    list.push(member.clone());
                }
            } else if let Some(position) = list.iter().position(|p| p.key == peer.key) {
                list.swap_remove(position);
                if list.capacity() > 256 && list.len() <= list.capacity() / 8 {
                    list.shrink_to(list.len() * 2);
                }
            }
        }
        (previous ^ flags) & DISCONNECT_MESSAGE != 0
    }

    fn message_snapshot(&self, flag: u8) -> Arc<Vec<Arc<Peer>>> {
        self.message_snapshots[flag.trailing_zeros() as usize]
            .get_or_init(|| Arc::new(self.of(flag).to_vec()))
            .clone()
    }
}

// ---------------------------------------------------------------------------
// Deferred departure work.

/// FIFO of bounded jobs for one recipient population. Each room has one, the
/// global observers share one, and orphaned live fanouts have one.
#[derive(Default)]
pub(crate) struct DepartureQueue {
    state: Mutex<QueueState>,
}

#[derive(Default)]
struct QueueState {
    jobs: VecDeque<Box<RoomFanout>>,
    // Never reset when the queue empties: peers retain their barrier watermark.
    sequence: u64,
    /// Whether this queue is currently listed for worker progress.
    queued: bool,
}

impl DepartureQueue {
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.state.lock().jobs.len()
    }

    #[cfg(test)]
    pub(crate) fn sequence(&self) -> u64 {
        self.state.lock().sequence
    }

    #[cfg(test)]
    pub(crate) fn set_sequence(&self, sequence: u64) {
        self.state.lock().sequence = sequence;
    }

    #[cfg(test)]
    pub(crate) fn with_jobs<T>(&self, f: impl FnOnce(&VecDeque<Box<RoomFanout>>) -> T) -> T {
        f(&self.state.lock().jobs)
    }
}

/// Round-robin list of queues with pending work. Workers only ever `try_lock`
/// it, so a busy worker never waits here.
#[derive(Default)]
struct Retirements {
    active: Mutex<VecDeque<Arc<DepartureQueue>>>,
    pending: AtomicBool,
}

/// Shared disconnect membership is charged once while any departure retains it.
/// The relay holds only a weak cache; the last job releases both handles and
/// credit. Jobs and recipient output retain their independent reservations.
struct RetirementSnapshot {
    members: Vec<Arc<Peer>>,
    metrics: Arc<Metrics>,
    bytes: usize,
}

impl Drop for RetirementSnapshot {
    fn drop(&mut self) {
        // Release the actual handle buffer before making its credit reusable.
        drop(std::mem::take(&mut self.members));
        self.metrics.queued_bytes.fetch_sub(self.bytes, Relaxed);
    }
}

/// How one departure notice reaches the room it leaves.
enum Departure {
    Direct(Vec<Arc<Peer>>),
    Deferred(Arc<Vec<Arc<Peer>>>),
}

enum FanoutRecipients {
    Room(Arc<Vec<Arc<Peer>>>),
    Disconnect(Arc<RetirementSnapshot>),
    Message(MessageRecipients),
}

struct MessageRecipients {
    room: Option<Arc<Vec<Arc<Peer>>>>,
    observers: Arc<Vec<Arc<Peer>>>,
    excluded_observer: u64,
}

fn membership_bytes(capacity: usize) -> Option<usize> {
    capacity
        .checked_mul(std::mem::size_of::<Arc<Peer>>())
        .and_then(|bytes| {
            bytes.checked_add(
                std::mem::size_of::<Vec<Arc<Peer>>>() + 2 * std::mem::size_of::<usize>(),
            )
        })
}

impl MessageRecipients {
    fn room(&self) -> &[Arc<Peer>] {
        self.room.as_ref().map_or(&[], |room| room.as_slice())
    }

    fn storage_bytes(&self) -> Option<usize> {
        let room = self
            .room
            .as_ref()
            .map_or(Some(0), |room| membership_bytes(room.capacity()))?;
        room.checked_add(membership_bytes(self.observers.capacity())?)
    }
}

impl FanoutRecipients {
    fn single_slice(&self) -> &[Arc<Peer>] {
        match self {
            Self::Room(members) => members,
            Self::Disconnect(snapshot) => &snapshot.members,
            Self::Message(_) => unreachable!("message publications have two recipient segments"),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Message(message) => message.room().len().saturating_add(message.observers.len()),
            _ => self.single_slice().len(),
        }
    }
}

/// One immutable membership snapshot and one charged shared body. Publisher
/// slots reuse their box; departure jobs belong to a departure queue. Empty
/// slots retain no payload or peer ownership.
#[derive(Default)]
pub(crate) struct RoomFanout {
    recipients: Option<FanoutRecipients>,
    packet: Option<Envelope>,
    cursor: usize,
    source: u64,
    snapshot_bytes: usize,
    // Zero denotes a live publisher; positive values order departure jobs.
    retirement_sequence: u64,
    /// Jobs in the observer queue honour each observer's revocation watermark.
    checks_barrier: bool,
    /// A refused recipient never aborts a job that outlives its source.
    tolerant: bool,
    // Reused only by sources publishing to a large observer audience. The
    // indexed job has no Publication of its own, so ownership is acyclic.
    observed: Option<Publication>,
}

impl RoomFanout {
    fn begin(
        &mut self,
        source: &Peer,
        recipients: FanoutRecipients,
        frame: Frame,
        retirement_sequence: u64,
    ) -> Result<(), Disconnect> {
        debug_assert!(!self.active());
        let packet = source.charge_strict(&frame)?.into_owned();
        let job_bytes = if retirement_sequence != 0 {
            std::mem::size_of::<Self>() + 2 * std::mem::size_of::<Box<Self>>()
        } else {
            0
        };
        // Room views remain conservatively charged per publisher. Disconnect
        // snapshots own one shared charge, so jobs reserve only their metadata.
        let snapshot_bytes = match &recipients {
            FanoutRecipients::Room(members) => {
                membership_bytes(members.capacity()).and_then(|n| n.checked_add(job_bytes))
            }
            FanoutRecipients::Disconnect(_) => Some(job_bytes),
            FanoutRecipients::Message(message) => message
                .storage_bytes()
                .and_then(|n| n.checked_add(job_bytes)),
        }
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
        self.retirement_sequence = retirement_sequence;
        self.checks_barrier = false;
        self.tolerant = retirement_sequence != 0;
        self.snapshot_bytes = snapshot_bytes;
        self.packet = Some(packet);
        self.recipients = Some(recipients);
        self.cursor = 0;
        self.source = source.key;
        Ok(())
    }

    pub(crate) fn active(&self) -> bool {
        self.packet.is_some() || self.observed.as_ref().is_some_and(Publication::active)
    }

    pub(crate) fn clear(&mut self) {
        if let Some(observed) = &mut self.observed {
            observed.clear();
        }
        self.clear_local();
    }

    fn clear_local(&mut self) {
        // Captured views can be the final owners after index invalidation.
        // Drop their storage before returning the corresponding queue credit.
        self.recipients = None;
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
    }

    /// Mark a live publication as outliving its source. Remaining recipients
    /// are still visited; refusals no longer have a source to report to.
    fn detach(&mut self) {
        self.tolerant = true;
        if let Some(observed) = &mut self.observed {
            observed.detach();
        }
    }

    pub(crate) fn resume(&mut self, quantum: usize) -> Result<(), Disconnect> {
        self.resume_routing(quantum).map_err(|error| error.reason)
    }

    fn resume_routing(&mut self, quantum: usize) -> Result<(), RoutingFailure> {
        let visits = self.recipients.as_ref().map_or(0, |view| {
            quantum.min(view.len().saturating_sub(self.cursor))
        });
        if self.packet.is_some() {
            self.resume_with(visits, None)?;
        }
        if self.packet.is_none()
            && let Some(observed) = &mut self.observed
        {
            observed
                .resume(quantum - visits)
                .map_err(|error| error.after(visits))?;
        }
        Ok(())
    }

    fn resume_with(
        &mut self,
        quantum: usize,
        message_sequence: Option<u64>,
    ) -> Result<(), RoutingFailure> {
        let recipients = self.recipients.as_ref().expect("live fanout snapshot");
        let frame = &self.packet.as_ref().expect("charged fanout body").frame;
        let end = self.cursor.saturating_add(quantum).min(recipients.len());
        let tolerant = self.tolerant;
        if let FanoutRecipients::Message(message) = recipients {
            let room = message.room();
            let room_end = end.min(room.len());
            if self.cursor < room_end {
                Self::submit(
                    &room[self.cursor..room_end],
                    self.source,
                    frame,
                    None,
                    tolerant,
                )?;
            }
            let observer_start = self.cursor.saturating_sub(room.len());
            let observer_end = end.saturating_sub(room.len());
            if observer_start < observer_end {
                Self::submit(
                    &message.observers[observer_start..observer_end],
                    message.excluded_observer,
                    frame,
                    Some((
                        BarrierKind::Message,
                        message_sequence.expect("observer jobs have a publication sequence"),
                    )),
                    tolerant,
                )
                .map_err(|error| error.after(room_end.saturating_sub(self.cursor)))?;
            }
        } else {
            let barrier = self
                .checks_barrier
                .then_some((BarrierKind::Retirement, self.retirement_sequence));
            Self::submit(
                &recipients.single_slice()[self.cursor..end],
                self.source,
                frame,
                barrier,
                tolerant,
            )?;
        }
        self.cursor = end;
        if end == recipients.len() {
            self.clear_local();
        }
        Ok(())
    }

    fn submit(
        recipients: &[Arc<Peer>],
        excluded: u64,
        frame: &Frame,
        barrier: Option<(BarrierKind, u64)>,
        tolerant: bool,
    ) -> Result<(), RoutingFailure> {
        for (index, recipient) in recipients.iter().enumerate() {
            if recipient.key == excluded {
                continue;
            }
            if let Some((kind, sequence)) = barrier {
                let watermark = match kind {
                    BarrierKind::Retirement => &recipient.retirement_barrier,
                    BarrierKind::Message => &recipient.message_barrier,
                };
                if watermark.load(Relaxed) >= sequence {
                    continue;
                }
            }
            let result = recipient.enqueue_fanout(frame);
            if !tolerant {
                result.map_err(|reason| RoutingFailure::visited(reason, index + 1))?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum BarrierKind {
    Retirement,
    Message,
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
) -> Result<usize, RoutingFailure> {
    let work = recipients.len().max(1);
    for (index, recipient) in recipients.iter().enumerate() {
        if recipient.key != source.key {
            recipient
                .enqueue_fanout(&frame)
                .map_err(|reason| RoutingFailure::visited(reason, index + 1))?;
        }
    }
    Ok(work)
}

// ---------------------------------------------------------------------------
// Peers and output accounting.

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

pub(crate) struct Peer {
    key: u64,
    pub id: AtomicU64,
    /// Canonical name storage shared with the room.
    pub(crate) channel: Bytes,
    room: Arc<Channel>,
    authenticated: AtomicBool,
    listener: AtomicU8,
    closing: AtomicBool,
    // Watermarks let a barrier submit this observer's pending notices without
    // advancing everyone else's cursors, and prevent replay afterwards.
    retirement_barrier: AtomicU64,
    message_barrier: AtomicU64,
    output: Arc<dyn PeerOutput>,
    budget: Arc<Budget>,
}

impl Peer {
    pub(crate) fn close(&self, reason: Disconnect) -> bool {
        if self.closing.swap(true, Relaxed) {
            return false;
        }
        self.output.close(reason);
        true
    }

    fn is_global(&self) -> bool {
        self.channel.as_ref() == GLOBAL_CHANNEL
    }

    /// Output addressed to this peer by routing: dropped under pressure.
    fn enqueue_frame(&self, frame: &Frame) -> Result<(), Disconnect> {
        self.enqueue::<false, false, false>(frame)
    }

    fn enqueue_fanout(&self, frame: &Frame) -> Result<(), Disconnect> {
        self.enqueue::<true, false, false>(frame)
    }

    fn enqueue_barrier(&self, frame: &Frame) -> Result<(), Disconnect> {
        self.enqueue::<false, true, true>(frame)
    }

    /// A reply to this peer's own command. A peer that cannot even receive
    /// its reply is closed rather than left waiting for one.
    fn enqueue_reply(&self, frame: &Frame) -> Result<(), Disconnect> {
        self.enqueue::<false, false, true>(frame)
    }

    /// The welcome is submitted before the peer is published anywhere, and
    /// with no registry lock held, so it may be written inline like a reply.
    fn enqueue_welcome(&self, frame: &Frame) -> Result<(), Disconnect> {
        self.enqueue::<false, false, true>(frame)
    }

    // Select the output method at compile time. Ordinary private submissions
    // carry no multicast flag, extra branch or different frame representation.
    #[inline(always)]
    fn enqueue<const FANOUT: bool, const BARRIER: bool, const REPLY: bool>(
        &self,
        frame: &Frame,
    ) -> Result<(), Disconnect> {
        let Some(packet) = self.reserve(frame, REPLY)? else {
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
    fn reserve<'a>(
        &'a self,
        frame: &'a Frame,
        reply: bool,
    ) -> Result<Option<Submission<'a>>, Disconnect> {
        if self.closing.load(Relaxed) {
            return Ok(None);
        }
        self.charge(frame, reply)
    }

    /// Reserve output credit for one frame addressed to this peer.
    ///
    /// Per-session exhaustion closes this peer: it is the slow consumer. Global
    /// exhaustion never punishes the sender; the frame is dropped, the pressure
    /// counter advances, and this peer is closed only when it holds a large
    /// share of its own allowance or the frame is its own command reply.
    fn charge<'a>(
        &'a self,
        frame: &'a Frame,
        reply: bool,
    ) -> Result<Option<Submission<'a>>, Disconnect> {
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
            let held = self.budget.used.fetch_sub(size, Relaxed) - size;
            self.budget.metrics.global_pressure.fetch_add(1, Relaxed);
            if reply || held >= self.budget.limit / 4 {
                self.overloaded();
            }
            return Ok(None);
        }
        Ok(Some(Submission {
            frame,
            budget: Some(&self.budget),
        }))
    }

    /// Staging reservations for deferred work report refusal to the caller,
    /// which then falls back to immediate delivery. Closed sources can still
    /// stage their own departure notice.
    fn charge_strict<'a>(&'a self, frame: &'a Frame) -> Result<Submission<'a>, Disconnect> {
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
            return Err(Disconnect::OVERLOADED);
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
        Ok(Submission {
            frame,
            budget: Some(&self.budget),
        })
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

impl Registration {
    /// Hand an unfinished publication to the relay before this session leaves.
    pub(crate) fn detach_fanout(&self, work: Box<RoomFanout>) {
        self.relay.detach_fanout(&self.peer, work);
    }
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

// ---------------------------------------------------------------------------
// Inspection types.

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

impl ChannelSnapshot {
    fn of(channel: &Channel) -> Self {
        let state = channel.state.read();
        Self {
            name: SnapshotName::new(&channel.name),
            members: state.members.len(),
            variables: state.variables.len(),
            variable_bytes: state.variable_bytes,
        }
    }
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

// ---------------------------------------------------------------------------
// The relay.

impl Relay {
    pub fn new(config: Config) -> Result<Arc<Self>> {
        config.validate()?;
        // Verify entropy availability before listening, never silently use predictable IDs.
        let _ = random_id()?;
        let mut channels = HashMap::new();
        let global = Bytes::from_static(GLOBAL_CHANNEL);
        channels.insert(global.clone(), Channel::new(global));
        let read_budget = (config.total_read_buffer_bytes > 0)
            .then(|| tungstenite::protocol::ReadBudget::new(config.total_read_buffer_bytes));
        Ok(Arc::new(Self {
            ip_limit: crate::admission::IpLimit::new(config.max_connections_per_ip),
            config,
            metrics: Arc::new(Metrics {
                read_budget,
                ..Metrics::default()
            }),
            admission: AtomicU8::new(Admission::Open as u8),
            users: Users::new(),
            channels: RwLock::new(channels),
            membership_generation: AtomicU64::new(0),
            global_view: Mutex::new(None),
            listeners: RwLock::new(Listeners::default()),
            listener_mask: AtomicU8::new(0),
            disconnect_snapshot: Mutex::new(Weak::new()),
            retirements: Retirements::default(),
            observer_departures: Arc::new(DepartureQueue::default()),
            detached: Arc::new(DepartureQueue::default()),
            messages: Arc::new(MessageFanouts::default()),
            sessions: AtomicUsize::new(0),
            next_key: AtomicU64::new(1),
        }))
    }

    pub fn counts(&self) -> (usize, usize) {
        (self.sessions.load(Relaxed), self.channels.read().len())
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
        {
            let mut listeners = self.listeners.write();
            *listeners = Listeners::default();
            self.listener_mask.store(0, Relaxed);
        }
        *self.disconnect_snapshot.lock() = Weak::new();
        *self.global_view.lock() = None;
        // Lock order is queue state, then the active list. Take the list
        // first on its own, then clear each queue without holding the list.
        let queues: Vec<Arc<DepartureQueue>> = {
            let mut active = self.retirements.active.lock();
            self.retirements.pending.store(false, Relaxed);
            active.drain(..).collect()
        };
        for queue in queues {
            let mut state = queue.state.lock();
            state.jobs.clear();
            state.queued = false;
        }
    }

    fn channel_snapshot(&self) -> Vec<ChannelSnapshot> {
        let channels: Vec<Arc<Channel>> = self.channels.read().values().cloned().collect();
        channels.iter().map(|c| ChannelSnapshot::of(c)).collect()
    }

    fn channel_members_snapshot(&self) -> Vec<(SnapshotName, usize)> {
        let channels: Vec<Arc<Channel>> = self.channels.read().values().cloned().collect();
        channels
            .iter()
            .map(|c| (SnapshotName::new(&c.name), c.state.read().members.len()))
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
        channels.sort_by_cached_key(|channel| channel.name.base64());
        channels
    }

    pub fn disconnect(&self, id: u64) -> bool {
        self.users.get(id).is_some_and(|p| {
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

    /// Find a live room or create it. The index read lock is the fast path;
    /// creation takes the write lock and re-checks under it.
    fn room(&self, name: &[u8]) -> Result<Arc<Channel>, Disconnect> {
        if let Some(channel) = self.channels.read().get(name) {
            return Ok(channel.clone());
        }
        let mut channels = self.channels.write();
        if let Some(channel) = channels.get(name) {
            return Ok(channel.clone());
        }
        if channels.len() >= self.config.max_channels {
            return Err(Disconnect::OVERLOADED);
        }
        // Copy a bounded name once per channel. Peers in an existing channel
        // share its canonical storage instead of allocating another name.
        let name = Bytes::copy_from_slice(name);
        let channel = Channel::new(name.clone());
        channels.insert(name, channel.clone());
        Ok(channel)
    }

    pub(crate) fn subscribe_with_output(
        self: &Arc<Self>,
        channel: &[u8],
        output: Arc<dyn PeerOutput>,
    ) -> Result<Registration, Disconnect> {
        if channel.len() > 16 {
            return Err(Disconnect::CHANNEL_LENGTH);
        }
        let refused = |relay: &Relay| {
            relay.metrics.rejected.fetch_add(1, Relaxed);
            Disconnect::OVERLOADED
        };
        if self.is_draining()
            || self
                .sessions
                .fetch_update(Relaxed, Relaxed, |sessions| {
                    (sessions < self.config.max_connections).then_some(sessions + 1)
                })
                .is_err()
        {
            return Err(refused(self));
        }
        let mut lease = SessionLease {
            relay: self,
            room: None,
            id: None,
        };
        // Reserve room capacity before anything is sent, so a refused join
        // never receives a welcome. A room retired by a concurrent last
        // departure is replaced by a fresh one.
        let room = loop {
            let room = match self.room(channel) {
                Ok(room) => room,
                Err(reason) => {
                    self.metrics.rejected.fetch_add(1, Relaxed);
                    return Err(reason);
                }
            };
            if room
                .reserved
                .fetch_update(Relaxed, Relaxed, |count| {
                    (count < self.config.max_channel_members).then_some(count + 1)
                })
                .is_err()
            {
                return Err(refused(self));
            }
            if room.state.read().retired {
                room.reserved.fetch_sub(1, Relaxed);
                continue;
            }
            break room;
        };
        lease.room = Some(room.clone());
        let key = self.next_key.fetch_add(1, Relaxed);
        let (id, peer) = loop {
            let id = random_id().map_err(|_| Disconnect::OVERLOADED)?;
            if id == 0 || id == BROADCAST {
                continue;
            }
            let peer = Arc::new(Peer {
                key,
                id: AtomicU64::new(id),
                channel: room.name.clone(),
                room: room.clone(),
                authenticated: AtomicBool::new(false),
                listener: AtomicU8::new(0),
                closing: AtomicBool::new(false),
                retirement_barrier: AtomicU64::new(0),
                message_barrier: AtomicU64::new(0),
                output: output.clone(),
                budget: Arc::new(Budget {
                    used: AtomicUsize::new(0),
                    limit: self.config.queue_bytes,
                    global_limit: self.config.total_queue_bytes,
                    metrics: self.metrics.clone(),
                }),
            });
            if self.users.try_insert(id, &peer) {
                break (id, peer);
            }
        };
        lease.id = Some((id, key));
        // The welcome is first in this session's FIFO: nothing can address the
        // peer through its room before the insert below. No lock is held here,
        // so the inline socket write cannot delay any other session.
        peer.enqueue_welcome(&Frame::integer(id))?;
        {
            let mut state = room.state.write();
            if state.retired {
                // Lost a race with the last departure after the welcome went
                // out. Start over against the replacement room; the welcome
                // carries the same ID either way.
                drop(state);
                lease.room = None;
                room.reserved.fetch_sub(1, Relaxed);
                self.users.remove_owned(id, key);
                lease.id = None;
                self.sessions.fetch_sub(1, Relaxed);
                return self.subscribe_with_output(channel, output);
            }
            state.insert(peer.clone());
        }
        self.membership_changed();
        self.metrics.joined.fetch_add(1, Relaxed);
        lease.room = None;
        lease.id = None;
        std::mem::forget(lease);
        Ok(Registration {
            peer,
            relay: self.clone(),
        })
    }

    fn membership_changed(&self) {
        self.membership_generation.fetch_add(1, Relaxed);
        *self.global_view.lock() = None;
    }

    /// Every current room member. Cached until the next membership change; a
    /// view built during a change is used once and not cached.
    fn global_recipients(&self) -> Arc<Vec<Arc<Peer>>> {
        let generation = self.membership_generation.load(Relaxed);
        if let Some((cached, view)) = &*self.global_view.lock()
            && *cached == generation
        {
            return view.clone();
        }
        let channels: Vec<Arc<Channel>> = self.channels.read().values().cloned().collect();
        let mut members = Vec::new();
        for channel in &channels {
            members.extend(
                channel
                    .state
                    .read()
                    .members
                    .values()
                    .filter(|peer| !peer.closing.load(Relaxed))
                    .cloned(),
            );
        }
        let view = Arc::new(members);
        if self.membership_generation.load(Relaxed) == generation {
            let mut cache = self.global_view.lock();
            if cache
                .as_ref()
                .is_none_or(|(cached, _)| *cached < generation)
            {
                *cache = Some((generation, view.clone()));
            }
        }
        view
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

    fn receive_with_totals(
        &self,
        peer: &Peer,
        bytes: Bytes,
        storage_capacity: usize,
        totals: &mut InputTotals,
        native: Option<(&mut Option<Box<RoomFanout>>, usize)>,
    ) -> Result<usize, RoutingFailure> {
        if peer.closing.load(Relaxed) {
            return Ok(1);
        }
        totals.messages = totals.messages.wrapping_add(1);
        totals.bytes = totals.bytes.wrapping_add(bytes.len() as u64);
        let (target, value) = protocol::parse(&bytes)?;
        if target == 0 {
            return self.control(peer, value).map(|_| 1).map_err(Into::into);
        }
        let private_recipient = if target == BROADCAST {
            if peer.is_global()
                && self.config.disable_global_broadcast
                && !peer.authenticated.load(Relaxed)
            {
                return Err(Disconnect {
                    code: 1008,
                    reason: "Global Broadcast Requires Authentication",
                }
                .into());
            }
            None
        } else {
            match self.users.get(target) {
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
        let mask = self.listener_mask.load(Relaxed);
        if let Some(recipient) = &private_recipient
            && mask & PRIVATE_MESSAGE == 0
        {
            // The ownership lookup above was atomic with reassignment. The
            // retained target owns its lifetime through the socket write.
            return recipient
                .enqueue_frame(&frame)
                .map(|_| 1)
                .map_err(Into::into);
        }
        if target == BROADCAST && peer.is_global() {
            return self.deliver_room(peer, self.global_recipients(), frame, native);
        }
        if target == BROADCAST && mask & CHANNEL_MESSAGE == 0 {
            let recipients = peer.room.state.read().view();
            return self.deliver_room(peer, recipients, frame, native);
        }
        // Observers are present. Hold the listener lock through capture so a
        // concurrent revocation sees either the registered job or none.
        let listeners = self.listeners.read();
        let flag = if target == BROADCAST {
            CHANNEL_MESSAGE
        } else {
            PRIVATE_MESSAGE
        };
        let room = (target == BROADCAST).then(|| peer.room.state.read().view());
        let observers = listeners.of(flag);
        // Charge every fanout path by recipient visits, including global
        // observers. One complete message remains atomic with respect to this
        // source's FIFO; the connection yields before its next buffered frame.
        // Counts conservatively include the sender or duplicate primary target.
        let work = room
            .as_ref()
            .map_or(1, |room| room.len())
            .saturating_add(observers.len());
        if let Some((pending, quantum)) = native
            && work > quantum
        {
            // Preserve the primary private delivery before observer work,
            // including when a large captured view cannot obtain staging credit.
            if let Some(recipient) = &private_recipient {
                recipient.enqueue_fanout(&frame)?;
            }
            let snapshot = listeners.message_snapshot(flag);
            let room = room.as_ref().filter(|room| room.len() > 1).cloned();
            let publisher = pending.get_or_insert_with(|| Box::new(RoomFanout::default()));
            let observed = publisher
                .observed
                .get_or_insert_with(|| Publication::new(self.messages.clone()));
            let direct_visits = usize::from(private_recipient.is_some());
            if observed
                .begin(
                    peer,
                    MessageRecipients {
                        room: room.clone(),
                        observers: snapshot,
                        excluded_observer: private_recipient.as_ref().map_or(0, |p| p.key),
                    },
                    frame.clone(),
                )
                .is_ok()
            {
                drop(listeners);
                publisher
                    .resume_routing(quantum.saturating_sub(direct_visits))
                    .map_err(|error| error.after(direct_visits))?;
                return Ok(quantum.max(direct_visits));
            }
            // Staging credit was refused: deliver immediately instead. The
            // primary recipient was already served above.
            publisher.clear();
            let mut visited = direct_visits;
            if let Some(room) = &room {
                for recipient in room.iter() {
                    visited += 1;
                    if recipient.key != peer.key {
                        recipient
                            .enqueue_fanout(&frame)
                            .map_err(|reason| RoutingFailure::visited(reason, visited))?;
                    }
                }
            }
            let excluded = private_recipient.as_ref().map_or(0, |p| p.key);
            for recipient in observers {
                visited += 1;
                if recipient.key != excluded {
                    recipient
                        .enqueue_fanout(&frame)
                        .map_err(|reason| RoutingFailure::visited(reason, visited))?;
                }
            }
            return Ok(work.max(1));
        }
        let mut visited = 0;
        if let Some(room) = &room {
            for recipient in room.iter() {
                visited += 1;
                if recipient.key != peer.key {
                    recipient
                        .enqueue_fanout(&frame)
                        .map_err(|reason| RoutingFailure::visited(reason, visited))?;
                }
            }
            for recipient in observers {
                visited += 1;
                recipient
                    .enqueue_fanout(&frame)
                    .map_err(|reason| RoutingFailure::visited(reason, visited))?;
            }
        } else if let Some(recipient) = &private_recipient {
            visited = 1;
            recipient.enqueue_fanout(&frame)?;
            for listener in observers {
                visited += 1;
                if listener.key != recipient.key {
                    listener
                        .enqueue_fanout(&frame)
                        .map_err(|reason| RoutingFailure::visited(reason, visited))?;
                }
            }
        }
        Ok(work.max(1))
    }

    fn deliver_room(
        &self,
        source: &Peer,
        recipients: Arc<Vec<Arc<Peer>>>,
        frame: Frame,
        native: Option<(&mut Option<Box<RoomFanout>>, usize)>,
    ) -> Result<usize, RoutingFailure> {
        if let Some((pending, quantum)) = native
            && recipients.len() > quantum
        {
            let work = pending.get_or_insert_with(|| Box::new(RoomFanout::default()));
            if work
                .begin(
                    source,
                    FanoutRecipients::Room(recipients.clone()),
                    frame.clone(),
                    0,
                )
                .is_ok()
            {
                work.resume_routing(quantum)?;
                return Ok(quantum);
            }
            // Refused staging credit: one longer turn beats a lost message.
        }
        deliver_room(source, recipients, frame)
    }

    /// A publisher's connection ended with an unfinished fanout. The remaining
    /// recipients still receive the message through a departure queue, ahead
    /// of the source's own departure notice.
    pub(crate) fn detach_fanout(&self, source: &Peer, mut work: Box<RoomFanout>) {
        if !work.active() || self.is_stopping() {
            return;
        }
        work.detach();
        let queue = match &work.recipients {
            Some(FanoutRecipients::Room(_)) if !source.is_global() => &source.room.departures,
            _ => &self.detached,
        };
        self.push_job(queue, work, &*source.output);
    }

    fn control(&self, peer: &Peer, value: &[u8]) -> Result<(), Disconnect> {
        match protocol::command(value)? {
            Command::Authenticate(password) => {
                if protocol::secret_matches(self.config.auth_password.as_deref(), password) {
                    peer.authenticated.store(true, Relaxed);
                }
                peer.enqueue_reply(&Frame::reply_bool(0, peer.authenticated.load(Relaxed)))?;
            }
            Command::Listen(flags) => {
                if !peer.authenticated.load(Relaxed) {
                    return self.unauthorized();
                }
                let mut barrier = false;
                let mut notices = Vec::new();
                if peer.is_global() {
                    let member = peer.room.state.read().members.get(&peer.key).cloned();
                    let mut listeners = self.listeners.write();
                    if peer.closing.load(Relaxed) {
                        return Ok(());
                    }
                    if peer.listener.load(Relaxed) & !flags & (CHANNEL_MESSAGE | PRIVATE_MESSAGE)
                        != 0
                    {
                        notices.extend(self.messages.flush(peer));
                        barrier = true;
                    }
                    if peer.listener.load(Relaxed) & DISCONNECT_MESSAGE != 0
                        && flags & DISCONNECT_MESSAGE == 0
                    {
                        // Older deferred notices must precede revocation's ACK.
                        notices.extend(self.flush_observer_retirements(peer));
                        barrier = true;
                    }
                    if listeners.set_flags(peer, flags & 7, member.as_ref()) {
                        *self.disconnect_snapshot.lock() = Weak::new();
                    }
                    self.listener_mask.store(listeners.mask(), Relaxed);
                } else {
                    peer.listener.store(flags & 7, Relaxed);
                }
                // Every registry lock is released now. The watermarks stored
                // above keep deferred jobs from replaying these notices, and
                // nothing can reach this observer's FIFO before its ACK.
                for notice in &notices {
                    let _ = peer.enqueue_barrier(notice);
                }
                let reply = Frame::reply_uint(1, u64::from(flags & 7));
                if barrier {
                    peer.enqueue_barrier(&reply)?;
                } else {
                    peer.enqueue_reply(&reply)?;
                }
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
                peer.enqueue_reply(&Frame::reply(2, e.into_writer().into()))?;
            }
            Command::Assign(id) => {
                if !peer.authenticated.load(Relaxed) {
                    return self.unauthorized();
                }
                if id == 0 || id == BROADCAST {
                    return Err(Disconnect::RESERVED);
                }
                if peer.closing.load(Relaxed) {
                    return Ok(());
                }
                let old = peer.id.load(Relaxed);
                if old != id {
                    // Takeover may have happened after receive's initial
                    // closing check. The map entry must still belong to this
                    // immutable session key.
                    let Some(displaced) = self.users.reassign(peer, old, id) else {
                        return Ok(());
                    };
                    peer.id.store(id, Relaxed);
                    if let Some(previous) = displaced {
                        previous.close(Disconnect::TAKEN);
                        self.membership_changed();
                    }
                }
                peer.enqueue_reply(&Frame::reply_uint(3, id))?;
            }
            Command::SetVariable { key, value } => {
                if peer.closing.load(Relaxed) {
                    return Ok(());
                }
                {
                    let mut state = peer.room.state.write();
                    if state.retired {
                        return Err(Disconnect::PROTOCOL);
                    }
                    let previous_size = state.variables.get(key).map_or(0, |v| key.len() + v.len());
                    let added = key.len() + value.len();
                    let next = state.variable_bytes - previous_size + added;
                    if next > self.config.max_variable_bytes
                        || (previous_size == 0
                            && state.variables.len() >= self.config.max_variables)
                        || self
                            .metrics
                            .variable_bytes
                            .fetch_update(Relaxed, Relaxed, |total| {
                                (total - previous_size + added <= self.config.total_variable_bytes)
                                    .then_some(total - previous_size + added)
                            })
                            .is_err()
                    {
                        return Err(Disconnect::OVERLOADED);
                    }
                    // Copy the retained value, so a tiny variable cannot pin a large input frame.
                    state
                        .variables
                        .insert(Bytes::copy_from_slice(key), Bytes::copy_from_slice(value));
                    state.variable_bytes = next;
                }
                peer.enqueue_reply(&Frame::reply_bool(4, true))?;
            }
            Command::GetVariable(key) => {
                let value = peer.room.state.read().variables.get(key).cloned();
                peer.enqueue_reply(&Frame::variable(value))?;
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
        self.retirements.pending.load(Relaxed)
    }

    /// Advance one queue by one bounded turn. Queues rotate so a large job in
    /// one room cannot delay another room's notices. Never blocks a worker.
    pub(crate) fn progress_retirements(&self, quantum: usize) {
        let queue = {
            let Some(mut active) = self.retirements.active.try_lock() else {
                return;
            };
            let Some(queue) = active.pop_front() else {
                self.retirements.pending.store(false, Relaxed);
                return;
            };
            self.retirements.pending.store(!active.is_empty(), Relaxed);
            queue
        };
        let Some(mut state) = queue.state.try_lock() else {
            self.requeue(queue);
            return;
        };
        if let Some(work) = state.jobs.front_mut() {
            let _ = work.resume(quantum);
            if !work.active() {
                state.jobs.pop_front();
            }
        }
        if state.jobs.is_empty() {
            state.queued = false;
            if state.jobs.capacity() > 32 {
                state.jobs = VecDeque::new();
            }
        } else {
            drop(state);
            self.requeue(queue);
        }
    }

    fn requeue(&self, queue: Arc<DepartureQueue>) {
        let mut active = self.retirements.active.lock();
        active.push_back(queue);
        self.retirements.pending.store(true, Relaxed);
    }

    /// Append a job and make sure its queue is scheduled. Returns whether a
    /// worker needs waking: only an idle retirement system does.
    fn push_job(
        &self,
        queue: &Arc<DepartureQueue>,
        work: Box<RoomFanout>,
        output: &dyn PeerOutput,
    ) {
        let mut state = queue.state.lock();
        state.jobs.push_back(work);
        if !state.queued {
            state.queued = true;
            let mut active = self.retirements.active.lock();
            let wake = active.is_empty();
            active.push_back(queue.clone());
            self.retirements.pending.store(true, Relaxed);
            drop(active);
            drop(state);
            if wake {
                output.wake_retirement();
            }
        }
    }

    /// Collect this observer's still-pending captured notices and record a
    /// watermark so deferred jobs cannot replay them after re-subscription.
    /// The caller submits the notices once every lock is released, ahead of
    /// the revocation acknowledgement. Only the observer queue is scanned; it
    /// holds every job that can reach a `re_globl` member.
    fn flush_observer_retirements(&self, observer: &Peer) -> Vec<Frame> {
        let mut notices = Vec::new();
        let pending = self.observer_departures.state.lock();
        let previous = observer.retirement_barrier.load(Relaxed);
        // Adjacent jobs often share an immutable view; reuse its lookup within
        // this barrier, but check each job's own cursor.
        let mut searched: Option<(&[Arc<Peer>], Option<usize>)> = None;
        for work in &pending.jobs {
            let Some(recipients) = work.recipients.as_ref() else {
                continue;
            };
            if work.retirement_sequence <= previous {
                continue;
            }
            let view = match recipients {
                FanoutRecipients::Message(_) => continue,
                other => other.single_slice(),
            };
            let position = match searched {
                Some((previous_view, position)) if std::ptr::eq(previous_view, view) => position,
                _ => {
                    let position = view
                        .iter()
                        .position(|peer| std::ptr::eq(Arc::as_ptr(peer), observer));
                    searched = Some((view, position));
                    position
                }
            };
            if position.is_some_and(|position| position >= work.cursor) {
                notices.push(
                    work.packet
                        .as_ref()
                        .expect("charged retirement body")
                        .frame
                        .clone(),
                );
            }
        }
        // Deferred visits skip notices already submitted by this barrier, even
        // if the observer re-enables the bit before those jobs finish.
        observer.retirement_barrier.store(pending.sequence, Relaxed);
        notices
    }

    /// Budget refusal fallback: finish this queue's older jobs synchronously
    /// so a direct notice cannot overtake them. Other queues keep their turns.
    fn flush_queue(&self, queue: &Arc<DepartureQueue>) {
        let mut state = queue.state.lock();
        for work in state.jobs.iter_mut() {
            let _ = work.resume(usize::MAX);
        }
        state.jobs.clear();
        if state.jobs.capacity() > 32 {
            state.jobs = VecDeque::new();
        }
        if state.queued {
            state.queued = false;
            let mut active = self.retirements.active.lock();
            active.retain(|listed| !Arc::ptr_eq(listed, queue));
            self.retirements.pending.store(!active.is_empty(), Relaxed);
        }
    }

    /// Queue a departure notice as a bounded job. Returns false when staging
    /// credit or numbering is exhausted: the caller then finishes this queue
    /// synchronously and delivers directly, preserving order.
    fn defer_departure(
        &self,
        queue: &Arc<DepartureQueue>,
        source: &Peer,
        recipients: FanoutRecipients,
        frame: &Frame,
        checks_barrier: bool,
    ) -> bool {
        let mut state = queue.state.lock();
        if self.is_stopping() {
            return true;
        }
        // Exhaustion falls back to ordered synchronous delivery rather than
        // wrapping and making older observer watermarks suppress new notices.
        let Some(sequence) = state.sequence.checked_add(1) else {
            return false;
        };
        let mut work = Box::<RoomFanout>::default();
        if work
            .begin(source, recipients, frame.clone(), sequence)
            .is_err()
        {
            return false;
        }
        work.checks_barrier = checks_barrier;
        state.sequence = sequence;
        state.jobs.push_back(work);
        let wake = if state.queued {
            false
        } else {
            state.queued = true;
            let mut active = self.retirements.active.lock();
            let wake = active.is_empty();
            active.push_back(queue.clone());
            self.retirements.pending.store(true, Relaxed);
            wake
        };
        drop(state);
        if wake {
            source.output.wake_retirement();
        }
        true
    }

    fn disconnect_recipients(&self, listeners: &Listeners) -> Option<Arc<RetirementSnapshot>> {
        let mut cache = self.disconnect_snapshot.lock();
        if let Some(snapshot) = cache.upgrade() {
            return Some(snapshot);
        }
        let members = listeners.of(DISCONNECT_MESSAGE).to_vec();
        let bytes = members
            .capacity()
            .checked_mul(std::mem::size_of::<Arc<Peer>>())
            .and_then(|n| {
                n.checked_add(
                    std::mem::size_of::<RetirementSnapshot>() + 2 * std::mem::size_of::<usize>(),
                )
            })?;
        if self
            .metrics
            .queued_bytes
            .fetch_update(Relaxed, Relaxed, |used| {
                used.checked_add(bytes)
                    .filter(|&next| next <= self.config.total_queue_bytes)
            })
            .is_err()
        {
            self.metrics.global_pressure.fetch_add(1, Relaxed);
            return None;
        }
        let snapshot = Arc::new(RetirementSnapshot {
            members,
            bytes,
            metrics: self.metrics.clone(),
        });
        *cache = Arc::downgrade(&snapshot);
        Some(snapshot)
    }

    fn remove(&self, peer: &Peer) {
        let room = &peer.room;
        let stopping = self.is_stopping();
        // Global room departures share the observer queue; see its notes.
        let queue = if peer.is_global() {
            &self.observer_departures
        } else {
            &room.departures
        };
        let remaining = {
            let mut state = room.state.write();
            if stopping {
                // Terminal removals do not need a future routing index. Drop its
                // cache once; captured fanouts keep their own immutable owners.
                state.broadcast.take();
            }
            if !state.remove(peer.key) {
                return;
            }
            peer.closing.store(true, Relaxed);
            if state.members.is_empty() || stopping {
                None
            } else if peer.output.supports_retirement()
                && (state.members.len() > RETIREMENT_QUANTUM || !queue.state.lock().jobs.is_empty())
            {
                // Large, or ordered behind older work: capture the shared view.
                Some(Departure::Deferred(state.view()))
            } else {
                // Small and nothing older ahead: a short copy, no cached view.
                Some(Departure::Direct(state.members.values().cloned().collect()))
            }
        };
        room.reserved.fetch_sub(1, Relaxed);
        self.membership_changed();
        let id = peer.id.load(Relaxed);
        let frame = (!stopping).then(|| Frame::envelope(BROADCAST, protocol::uint(id)));
        match (&frame, remaining) {
            (Some(frame), Some(Departure::Direct(recipients))) => {
                for recipient in &recipients {
                    let _ = recipient.enqueue_fanout(frame);
                }
            }
            (Some(frame), Some(Departure::Deferred(recipients))) => {
                if !self.defer_departure(
                    queue,
                    peer,
                    FanoutRecipients::Room(recipients.clone()),
                    frame,
                    peer.is_global(),
                ) {
                    self.flush_queue(queue);
                    for recipient in recipients.iter() {
                        let _ = recipient.enqueue_fanout(frame);
                    }
                }
            }
            _ => {}
        }
        // An empty ordinary room leaves the index. Re-check under both locks:
        // a joiner may have arrived between the membership update and here.
        if !peer.is_global() {
            let empty = room.state.read().members.is_empty();
            if empty {
                let mut channels = self.channels.write();
                let mut state = room.state.write();
                if state.members.is_empty()
                    && !state.retired
                    && channels
                        .get(&room.name)
                        .is_some_and(|current| Arc::ptr_eq(current, room))
                {
                    state.retired = true;
                    self.metrics
                        .variable_bytes
                        .fetch_sub(state.variable_bytes, Relaxed);
                    channels.remove(&room.name);
                    shrink_sparse_map(&mut channels);
                }
            } else {
                shrink_sparse_map(&mut room.state.write().members);
            }
        } else {
            shrink_sparse_map(&mut room.state.write().members);
            let member_flags = peer.listener.load(Relaxed);
            if member_flags != 0 {
                let mut listeners = self.listeners.write();
                if listeners.set_flags(peer, 0, None) {
                    *self.disconnect_snapshot.lock() = Weak::new();
                }
                self.listener_mask.store(listeners.mask(), Relaxed);
            }
        }
        if let Some(frame) = &frame
            && !peer.is_global()
            && self.listener_mask.load(Relaxed) & DISCONNECT_MESSAGE != 0
        {
            // Hold the listener lock through capture and queueing, so a
            // concurrent revocation observes the job or never sees this notice.
            let listeners = self.listeners.read();
            let observers = listeners.of(DISCONNECT_MESSAGE);
            if !observers.is_empty() {
                let queue = &self.observer_departures;
                let deferred = peer.output.supports_retirement()
                    && (observers.len() > RETIREMENT_QUANTUM
                        || !queue.state.lock().jobs.is_empty());
                let queued = deferred
                    && self
                        .disconnect_recipients(&listeners)
                        .is_some_and(|snapshot| {
                            self.defer_departure(
                                queue,
                                peer,
                                FanoutRecipients::Disconnect(snapshot),
                                frame,
                                true,
                            )
                        });
                if !queued {
                    if deferred {
                        self.flush_queue(queue);
                    }
                    for recipient in observers {
                        let _ = recipient.enqueue_fanout(frame);
                    }
                }
            }
        }
        self.users.remove_owned(id, peer.key);
        self.sessions.fetch_sub(1, Relaxed);
        self.metrics.disconnected.fetch_add(1, Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn channel(&self, name: &[u8]) -> Option<Arc<Channel>> {
        self.channels.read().get(name).cloned()
    }

    #[cfg(test)]
    pub(crate) fn user(&self, id: u64) -> Option<Arc<Peer>> {
        self.users.get(id)
    }

    #[cfg(test)]
    pub(crate) fn cached_global_view(&self) -> Option<Arc<Vec<Arc<Peer>>>> {
        self.global_view
            .lock()
            .as_ref()
            .map(|(_, view)| view.clone())
    }

    #[cfg(test)]
    pub(crate) fn listeners_of(&self, flag: u8) -> Vec<Arc<Peer>> {
        self.listeners.read().of(flag).to_vec()
    }

    #[cfg(test)]
    pub(crate) fn listener_capacities(&self) -> [usize; 3] {
        let listeners = self.listeners.read();
        [0, 1, 2].map(|i| listeners.lists[i].capacity())
    }

    #[cfg(test)]
    pub(crate) fn message_snapshot_cached(&self, index: usize) -> Option<Arc<Vec<Arc<Peer>>>> {
        self.listeners.read().message_snapshots[index]
            .get()
            .cloned()
    }

    #[cfg(test)]
    pub(crate) fn disconnect_snapshot_cached(&self) -> bool {
        self.disconnect_snapshot.lock().upgrade().is_some()
    }

    #[cfg(test)]
    pub(crate) fn observer_queue(&self) -> &Arc<DepartureQueue> {
        &self.observer_departures
    }

    #[cfg(test)]
    pub(crate) fn detached_queue(&self) -> &Arc<DepartureQueue> {
        &self.detached
    }

    /// Whether no registry lock is currently held by anyone. Output paths
    /// must run with this true.
    #[cfg(test)]
    pub(crate) fn registry_unlocked(&self) -> bool {
        self.channels.try_write().is_some()
            && self.listeners.try_write().is_some()
            && self
                .channels
                .read()
                .values()
                .all(|channel| channel.state.try_write().is_some())
    }

    #[cfg(test)]
    pub(crate) fn listeners_locked(&self) -> bool {
        self.listeners.try_read().is_none()
    }

    #[cfg(test)]
    pub(crate) fn users_capacity(&self) -> usize {
        self.users.capacity()
    }

    #[cfg(test)]
    pub(crate) fn users_len(&self) -> usize {
        self.users.len()
    }

    #[cfg(test)]
    pub(crate) fn channels_capacity(&self) -> usize {
        self.channels.read().capacity()
    }
}

/// Undo partial admission when a join fails after reserving capacity.
struct SessionLease<'a> {
    relay: &'a Relay,
    room: Option<Arc<Channel>>,
    id: Option<(u64, u64)>,
}

impl Drop for SessionLease<'_> {
    fn drop(&mut self) {
        if let Some(room) = &self.room {
            room.reserved.fetch_sub(1, Relaxed);
        }
        if let Some((id, key)) = self.id {
            self.relay.users.remove_owned(id, key);
        }
        self.relay.sessions.fetch_sub(1, Relaxed);
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
mod tests;

#[cfg(test)]
mod continuation_tests;
