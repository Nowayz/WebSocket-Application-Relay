use std::{
    collections::HashMap,
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

/// The registry lock protects membership and ID ownership. It is never held across
/// await or socket I/O. Hot-path routing takes a read lock and only tries queues.
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
    next_key: u64,
    sessions: usize,
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
    largest_input: AtomicUsize,
    outgoing: mpsc::Sender<Envelope>,
    close: watch::Sender<Option<Disconnect>>,
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
    // The connection may lower this watermark only after the decoder installed
    // fresh base-size input storage. Published bodies keep their own ownership.
    pub(crate) fn reclaimed_input(&self, buffer: usize) {
        self.largest_input.store(buffer, Relaxed);
    }

    pub(crate) fn close(&self, reason: Disconnect) -> bool {
        if self.closing.swap(true, Relaxed) {
            return false;
        }
        self.close.send_replace(Some(reason));
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
        if self.outgoing.try_send(packet).is_err() {
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

    pub fn channels(&self) -> Vec<ChannelInfo> {
        let state = self.state.read();
        let mut channels: Vec<_> = state
            .channels
            .iter()
            .map(|(name, channel)| ChannelInfo {
                name: std::str::from_utf8(name).ok().map(str::to_owned),
                name_base64: STANDARD.encode(name),
                members: channel.members.len(),
                variables: channel.variables.len(),
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

    pub(crate) fn subscribe(self: &Arc<Self>, channel: &[u8]) -> Result<Subscription, Disconnect> {
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
        let (outgoing, receiver) = mpsc::channel(self.config.queue_messages);
        let (close, close_receiver) = watch::channel(None);
        let peer = Arc::new(Peer {
            key,
            id: AtomicU64::new(id),
            channel: Bytes::copy_from_slice(channel),
            authenticated: AtomicBool::new(false),
            listener: AtomicU8::new(0),
            closing: AtomicBool::new(false),
            largest_input: AtomicUsize::new(self.config.read_buffer_bytes),
            outgoing,
            close,
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
        Ok(Subscription {
            registration: Registration {
                peer,
                relay: self.clone(),
            },
            outgoing: receiver,
            close: close_receiver,
        })
    }

    pub(crate) fn receive(&self, peer: &Peer, bytes: Bytes) -> Result<(), Disconnect> {
        if peer.closing.load(Relaxed) {
            return Ok(());
        }
        self.metrics.incoming_messages.fetch_add(1, Relaxed);
        self.metrics
            .incoming_bytes
            .fetch_add(bytes.len() as u64, Relaxed);
        // Only this connection updates its high-water mark. A small view must
        // not keep a previous large receive slab alive in a slow output queue.
        let previous_largest = peer.largest_input.load(Relaxed);
        let largest = previous_largest.max(bytes.len());
        if largest != previous_largest {
            peer.largest_input.store(largest, Relaxed);
        }
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
        let compact = largest > self.config.read_buffer_bytes
            && bytes.len().saturating_mul(8) < largest
            && !bytes.is_unique();
        let frame = Frame::routed(peer.id.load(Relaxed), bytes, value_start, compact);
        let counter = if compact {
            &self.metrics.routing_copied_bytes
        } else {
            &self.metrics.routing_reused_bytes
        };
        counter.fetch_add(frame.payload().len() as u64, Relaxed);
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
                for recipient in state.channels[GLOBAL_CHANNEL].members.values() {
                    if recipient.listener.load(Relaxed) & CHANNEL_MESSAGE != 0 {
                        recipient.enqueue_frame(&frame)?;
                    }
                }
            }
        } else if let Some(recipient) = private_recipient {
            recipient.enqueue_frame(&frame)?;
            for listener in state.channels[GLOBAL_CHANNEL].members.values() {
                if listener.key != recipient.key
                    && listener.listener.load(Relaxed) & PRIVATE_MESSAGE != 0
                {
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
                peer.listener.store(flags & 7, Relaxed);
                peer.enqueue(&protocol::reply(1, &protocol::uint(u64::from(flags & 7))))?;
            }
            Command::Channels => {
                if !peer.authenticated.load(Relaxed) {
                    return self.unauthorized();
                }
                let state = self.state.read();
                let mut channels: Vec<_> = state.channels.iter().collect();
                channels.sort_unstable_by(|a, b| a.0.cmp(b.0));
                let mut e = Encoder::new(Vec::new());
                e.array(channels.len() as u64).expect("Vec encoding");
                for (name, channel) in channels {
                    e.array(2)
                        .and_then(|e| e.bytes(name))
                        .and_then(|e| e.u64(channel.members.len() as u64))
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
        }
        if notify && peer.channel.as_ref() != GLOBAL_CHANNEL {
            for recipient in state.channels[GLOBAL_CHANNEL].members.values() {
                if recipient.listener.load(Relaxed) & DISCONNECT_MESSAGE != 0 {
                    let _ = recipient.enqueue(frame.as_ref().expect("notifications enabled"));
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
    fn tiny_values_after_large_input_do_not_pin_a_shared_read_slab() {
        let relay = Relay::new(config()).unwrap();
        let mut sender = relay.subscribe(b"a").unwrap();
        let mut recipient = relay.subscribe(b"a").unwrap();
        sender.outgoing.try_recv().unwrap();
        recipient.outgoing.try_recv().unwrap();
        let unknown = (1..)
            .find(|id| !relay.state.read().users.contains_key(id))
            .unwrap();
        relay
            .receive(
                &sender.registration.peer,
                protocol::envelope(unknown, &protocol::byte_string(&[0; 65536])),
            )
            .unwrap();
        let small = protocol::envelope(recipient.registration.peer.id.load(Relaxed), &[0xf6]);
        let mut read_slab = bytes::BytesMut::with_capacity(65536);
        read_slab.extend_from_slice(&small);
        let input = read_slab.split().freeze();
        let backing = input.clone();
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        relay.receive(&sender.registration.peer, input).unwrap();
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
