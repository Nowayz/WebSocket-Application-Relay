use std::{
    collections::HashMap,
    hash::Hash,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering::Relaxed},
    },
};

use anyhow::{Context, Result};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use minicbor::Encoder;
use parking_lot::RwLock;
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

/// The registry lock protects membership and ID ownership. Output submission is
/// nonblocking; the transport writes immediately or retains a shared frame.
pub struct Relay {
    pub config: Config,
    pub metrics: Arc<Metrics>,
    pub draining: AtomicBool,
    stopping: AtomicBool,
    state: RwLock<Registry>,
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
    variables: HashMap<Bytes, Bytes>,
    variable_bytes: usize,
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

/// A transport owns ordering and backpressure. Sending never waits for socket
/// readiness, and close only signals the owning I/O loop.
pub(crate) trait PeerOutput: Send + Sync {
    fn send(&self, packet: Envelope) -> bool;
    fn close(&self, reason: Disconnect);
}

#[cfg(test)]
struct AsyncOutput {
    outgoing: mpsc::Sender<Envelope>,
    close: watch::Sender<Option<Disconnect>>,
}

#[cfg(test)]
impl PeerOutput for AsyncOutput {
    fn send(&self, packet: Envelope) -> bool {
        self.outgoing.try_send(packet).is_ok()
    }

    fn close(&self, reason: Disconnect) {
        self.close.send_replace(Some(reason));
    }
}

impl Drop for Envelope {
    fn drop(&mut self) {
        self.budget.used.fetch_sub(self.frame.len(), Relaxed);
        self.budget
            .metrics
            .queued_bytes
            .fetch_sub(self.frame.len(), Relaxed);
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

    fn enqueue(&self, bytes: &Bytes) -> Result<(), Disconnect> {
        self.enqueue_frame(&Frame::encoded(bytes.clone()))
    }

    fn enqueue_frame(&self, frame: &Frame) -> Result<(), Disconnect> {
        if self.closing.load(Relaxed) {
            return Ok(());
        }
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
            return Ok(());
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
        let packet = Envelope {
            frame: frame.clone(),
            budget: self.budget.clone(),
        };
        if !self.output.send(packet) {
            self.overloaded();
        }
        Ok(())
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
}

/// One coherent view owns only small names and numeric metadata. It keeps no
/// registry guard, peer or variable value alive while inspection formats output.
struct ChannelSnapshot {
    name: SnapshotName,
    members: usize,
    variables: usize,
    variable_bytes: usize,
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
            config,
            metrics: Arc::new(Metrics {
                read_budget,
                ..Metrics::default()
            }),
            draining: AtomicBool::new(false),
            stopping: AtomicBool::new(false),
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

    pub(crate) fn begin_shutdown(&self) {
        self.draining.store(true, Relaxed);
        self.stopping.store(true, Relaxed);
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

    pub fn disconnect(&self, id: u64) -> bool {
        self.state.read().users.get(&id).is_some_and(|p| {
            p.close(Disconnect {
                code: 4000,
                reason: "Administrative Disconnect",
            })
        })
    }

    #[cfg(test)]
    fn subscribe(self: &Arc<Self>, channel: &[u8]) -> Result<Subscription, Disconnect> {
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
        if self.draining.load(Relaxed)
            || state.sessions >= self.config.max_connections
            || (!state.channels.contains_key(channel)
                && state.channels.len() >= self.config.max_channels)
            || state
                .channels
                .get(channel)
                .is_some_and(|c| c.members.len() >= self.config.max_channel_members)
        {
            self.metrics.rejected.fetch_add(1, Relaxed);
            return Err(Disconnect::OVERLOADED);
        }
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
            channel: Bytes::copy_from_slice(channel),
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
        peer.enqueue(&protocol::uint(id))?;
        state.users.insert(id, peer.clone());
        state
            .channels
            .entry(peer.channel.clone())
            .or_default()
            .members
            .insert(key, peer.clone());
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
        self.receive_with_storage(peer, bytes, capacity)
    }

    pub(crate) fn receive_with_storage(
        &self,
        peer: &Peer,
        bytes: Bytes,
        storage_capacity: usize,
    ) -> Result<(), Disconnect> {
        if peer.closing.load(Relaxed) {
            return Ok(());
        }
        self.metrics.incoming_messages.fetch_add(1, Relaxed);
        self.metrics
            .incoming_bytes
            .fetch_add(bytes.len() as u64, Relaxed);
        let (target, value) = protocol::parse(&bytes)?;
        if target == 0 {
            return self.control(peer, value);
        }
        let state = self.state.read();
        if peer.closing.load(Relaxed) {
            return Ok(());
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
                None => return Ok(()),
            }
        };
        let value_start = bytes.len() - value.len();
        let compact = storage_capacity > self.config.read_buffer_bytes
            && bytes.len().saturating_mul(8) < storage_capacity
            && !bytes.is_unique();
        let frame = Frame::routed(peer.id.load(Relaxed), bytes, value_start, compact);
        let counter = if compact {
            &self.metrics.routing_copied_bytes
        } else {
            &self.metrics.routing_reused_bytes
        };
        counter.fetch_add(frame.payload().len() as u64, Relaxed);
        if let Some(recipient) = private_recipient
            && state.listeners(PRIVATE_MESSAGE).is_empty()
        {
            // Lookup and sender identity form one registry snapshot. The
            // retained target owns its lifetime through the socket write.
            let recipient = recipient.clone();
            drop(state);
            return recipient.enqueue_frame(&frame);
        }
        if target == BROADCAST {
            if peer.channel.as_ref() == GLOBAL_CHANNEL {
                for recipient in state.users.values() {
                    if recipient.key != peer.key {
                        recipient.enqueue_frame(&frame)?;
                    }
                }
            } else {
                if let Some(channel) = state.channels.get(&peer.channel) {
                    for recipient in channel.members.values() {
                        if recipient.key != peer.key {
                            recipient.enqueue_frame(&frame)?;
                        }
                    }
                }
                for recipient in state.listeners(CHANNEL_MESSAGE) {
                    recipient.enqueue_frame(&frame)?;
                }
            }
        } else if let Some(recipient) = private_recipient {
            recipient.enqueue_frame(&frame)?;
            for listener in state.listeners(PRIVATE_MESSAGE) {
                if listener.key != recipient.key {
                    listener.enqueue_frame(&frame)?;
                }
            }
        }
        Ok(())
    }

    fn control(&self, peer: &Peer, value: &[u8]) -> Result<(), Disconnect> {
        match protocol::command(value)? {
            Command::Authenticate(password) => {
                if protocol::secret_matches(self.config.auth_password.as_deref(), password) {
                    peer.authenticated.store(true, Relaxed);
                }
                peer.enqueue(&protocol::reply(
                    0,
                    &[if peer.authenticated.load(Relaxed) {
                        0xf5
                    } else {
                        0xf4
                    }],
                ))?;
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
                    state.set_listener_flags(peer, flags & 7);
                } else {
                    peer.listener.store(flags & 7, Relaxed);
                }
                peer.enqueue(&protocol::reply(1, &protocol::uint(u64::from(flags & 7))))?;
            }
            Command::Channels => {
                if !peer.authenticated.load(Relaxed) {
                    return self.unauthorized();
                }
                let mut channels = self.channel_members_snapshot();
                channels.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                let mut e = Encoder::new(Vec::new());
                e.array(channels.len() as u64).expect("Vec encoding");
                for (name, members) in channels {
                    e.array(2)
                        .and_then(|e| e.bytes(name.as_bytes()))
                        .and_then(|e| e.u64(members as u64))
                        .expect("Vec encoding");
                }
                peer.enqueue(&protocol::reply(2, &e.into_writer()))?;
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
                peer.enqueue(&protocol::reply(3, &protocol::uint(id)))?;
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
                peer.enqueue(&protocol::reply(4, &[0xf5]))?;
            }
            Command::GetVariable(key) => {
                let state = self.state.read();
                let value = state
                    .channels
                    .get(&peer.channel)
                    .and_then(|c| c.variables.get(key));
                let mut found = Vec::with_capacity(3 + value.map_or(1, Bytes::len));
                found.extend_from_slice(&[0x82, if value.is_some() { 0xf5 } else { 0xf4 }]);
                found.extend_from_slice(value.map_or(&[0xf6][..], Bytes::as_ref));
                peer.enqueue(&protocol::reply(200, &found))?;
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

    fn remove(&self, peer: &Peer) {
        let mut state = self.state.write();
        let Some(channel) = state.channels.get_mut(&peer.channel) else {
            return;
        };
        if channel.members.remove(&peer.key).is_none() {
            return;
        }
        peer.closing.store(true, Relaxed);
        let id = peer.id.load(Relaxed);
        let notify = !self.stopping.load(Relaxed);
        let frame = notify.then(|| protocol::envelope(BROADCAST, &protocol::uint(id)));
        if let Some(frame) = &frame {
            for recipient in channel.members.values() {
                let _ = recipient.enqueue(frame);
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
            for recipient in state.listeners(DISCONNECT_MESSAGE) {
                let _ = recipient.enqueue(frame.as_ref().expect("notifications enabled"));
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
mod listener_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            auth_password: Some("test-password".into()),
            ..Config::default()
        }
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
                    assert_eq!(departure.frame.payload(), expected.as_ref());
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
        assert_eq!(
            source.outgoing.try_recv().unwrap().frame.payload(),
            expected
        );
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
            rejoined.outgoing.try_recv().unwrap().frame.payload(),
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
            rejoined.outgoing.try_recv().unwrap().frame.payload(),
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
}
