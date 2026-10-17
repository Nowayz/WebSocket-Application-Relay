//! Socket output has one serialized owner at a time, including writes from a
//! different relay worker. The ordinary path writes borrowed slices immediately.
//! Only backpressure allocates queue storage or retains a message body.
use std::{
    collections::VecDeque,
    io::{self, IoSlice, Read, Write},
    sync::{
        Arc,
        atomic::{
            AtomicBool,
            Ordering::{Relaxed, Release},
        },
    },
    time::{Duration, Instant},
};

use mio::{Interest, Registry, Token, Waker, net::TcpStream};
use parking_lot::Mutex;

use super::ConnectionGuard;
use crate::{
    frame::Frame,
    metrics::Metrics,
    protocol::Disconnect,
    relay::{Envelope, PeerOutput},
};

pub(super) const WRITE_QUANTUM: usize = 256 * 1024;
const RETAIN_QUEUE_SLOTS: usize = 32;
// At most sixteen iovecs, with no waiting for a batch to fill.
const WRITE_BATCH_FRAMES: usize = 8;
const WRITE_BATCH_BYTES: usize = 4096;

pub(super) struct Signal {
    pub waker: Waker,
    pub stopping: AtomicBool,
    pub pending: Mutex<VecDeque<Token>>,
}

impl Signal {
    pub fn stop(&self) {
        self.stopping.store(true, Release);
        let _ = self.waker.wake();
    }
}

pub(super) struct Socket {
    pub state: Mutex<State>,
    registry: Arc<Registry>,
    signal: Arc<Signal>,
    pub token: Token,
    notified: AtomicBool,
    metrics: Arc<Metrics>,
    queue_limit: usize,
    write_timeout: Duration,
    track_activity: bool,
    // Drop after State (and its TCP descriptor), including when a route keeps
    // this socket alive beyond the decoder's retirement.
    _guard: ConnectionGuard,
}

pub(super) struct State {
    pub stream: TcpStream,
    queue: VecDeque<Pending>,
    pub requested: Option<Disconnect>,
    pub fatal: bool,
    pub closed: bool,
    writable: bool,
    pub control_pending: bool,
    pub control_deadline: Option<Instant>,
    pub activity: Instant,
}

struct Pending {
    packet: Envelope,
    offset: usize,
    deadline: Instant,
}

impl Socket {
    pub fn new(
        stream: TcpStream,
        token: Token,
        registry: Arc<Registry>,
        signal: Arc<Signal>,
        guard: ConnectionGuard,
        relay: &crate::Relay,
    ) -> io::Result<Arc<Self>> {
        let socket = Arc::new(Self {
            state: Mutex::new(State {
                stream,
                queue: VecDeque::new(),
                requested: None,
                fatal: false,
                closed: false,
                writable: false,
                control_pending: false,
                control_deadline: None,
                activity: Instant::now(),
            }),
            registry,
            signal,
            token,
            notified: AtomicBool::new(false),
            metrics: relay.metrics.clone(),
            queue_limit: relay.config.queue_messages,
            write_timeout: relay.config.write_timeout(),
            track_activity: relay.config.idle_timeout_secs > 0,
            _guard: guard,
        });
        socket
            .registry
            .register(&mut socket.state.lock().stream, token, Interest::READABLE)?;
        Ok(socket)
    }

    fn notify(&self) {
        if !self.notified.swap(true, Relaxed) {
            self.signal.pending.lock().push_back(self.token);
            let _ = self.signal.waker.wake();
        }
    }

    pub fn notified(&self) {
        self.notified.store(false, Relaxed);
    }

    fn writable(&self, state: &mut State, enabled: bool) -> io::Result<()> {
        // Keep common window-sized queues warm. Once a larger queue drains,
        // release its unused slots without touching any published body.
        if !enabled && state.queue.is_empty() && state.queue.capacity() > RETAIN_QUEUE_SLOTS {
            self.release_queue(state);
        }
        if state.writable != enabled {
            let interest = if enabled {
                Interest::READABLE | Interest::WRITABLE
            } else {
                Interest::READABLE
            };
            self.registry
                .reregister(&mut state.stream, self.token, interest)?;
            state.writable = enabled;
        }
        Ok(())
    }

    fn release_queue(&self, state: &mut State) {
        let capacity = std::mem::take(&mut state.queue).capacity();
        if capacity != 0 {
            self.metrics
                .output_queue_capacity_bytes
                .fetch_sub(capacity * std::mem::size_of::<Pending>(), Relaxed);
        }
    }

    fn delivered(&self, packet: &Envelope, state: &mut State) {
        self.metrics.deliveries.fetch_add(1, Relaxed);
        self.metrics
            .delivered_bytes
            .fetch_add(packet.frame.len() as u64, Relaxed);
        if self.track_activity {
            state.activity = Instant::now();
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        let state = self.state.lock();
        match (
            state.queue.front().map(|p| p.deadline),
            state.control_deadline,
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    /// Resume bounded application output. `true` means this socket consumed its
    /// quantum while still ready and must be explicitly scheduled again.
    pub fn flush(&self) -> io::Result<bool> {
        let mut state = self.state.lock();
        if state.control_pending {
            return Ok(false);
        }
        let mut remaining = WRITE_QUANTUM;
        while !state.queue.is_empty() {
            match write_pending(&mut SocketWriter(&state.stream), &state.queue, remaining) {
                Ok(written) => {
                    remaining -= written;
                    self.advance_written(&mut state, written);
                    if remaining == 0 {
                        let queued = !state.queue.is_empty();
                        self.writable(&mut state, queued)?;
                        return Ok(queued);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.writable(&mut state, true)?;
                    return Ok(false);
                }
                Err(error) => return Err(error),
            }
        }
        self.writable(&mut state, false)?;
        Ok(false)
    }

    fn advance_written(&self, state: &mut State, mut written: usize) {
        while written != 0 {
            let pending = state.queue.front_mut().expect("written bytes were queued");
            let remaining = wire_len(&pending.packet.frame) - pending.offset;
            let consumed = remaining.min(written);
            pending.offset += consumed;
            written -= consumed;
            if consumed == remaining {
                let pending = state.queue.pop_front().expect("completed queued frame");
                self.delivered(&pending.packet, state);
            }
        }
    }

    /// Cancel queued application frames before Close. A partially transmitted
    /// frame requires dropping TCP; a Close can never be appended inside it.
    pub fn end_output(&self) -> bool {
        let mut state = self.state.lock();
        state.closed = true;
        let partial = state.queue.front().is_some_and(|p| p.offset != 0);
        if !state.queue.is_empty() {
            self.metrics.aborted_writes.fetch_add(1, Relaxed);
            if !partial {
                self.metrics
                    .unstarted_writes_cancelled
                    .fetch_add(1, Relaxed);
            }
        }
        // Output is terminal, so even a small warm queue has no further use.
        self.release_queue(&mut state);
        partial
    }

    pub fn control_flushed(&self) -> io::Result<()> {
        let mut state = self.state.lock();
        state.control_pending = false;
        state.control_deadline = None;
        let queued = !state.queue.is_empty();
        self.writable(&mut state, queued)
    }
}

impl PeerOutput for Socket {
    fn send(&self, packet: Envelope) -> bool {
        let mut state = self.state.lock();
        if state.closed || state.fatal || state.requested.is_some() {
            return true;
        }
        if state.queue.len() >= self.queue_limit {
            return false;
        }
        let mut offset = 0;
        if state.queue.is_empty() && !state.control_pending {
            match write_frame(
                &mut SocketWriter(&state.stream),
                &packet.frame,
                &mut offset,
                WRITE_QUANTUM,
            ) {
                Ok(_) if offset == wire_len(&packet.frame) => {
                    self.delivered(&packet, &mut state);
                    return true;
                }
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                Err(_) => {
                    state.fatal = true;
                    self.metrics.write_errors.fetch_add(1, Relaxed);
                    self.notify();
                    return true;
                }
            }
        }
        let first = state.queue.is_empty();
        let old_capacity = state.queue.capacity();
        state.queue.push_back(Pending {
            packet,
            offset,
            deadline: Instant::now() + self.write_timeout,
        });
        let added = (state.queue.capacity() - old_capacity) * std::mem::size_of::<Pending>();
        if added != 0 {
            let allocated = self
                .metrics
                .output_queue_capacity_bytes
                .fetch_add(added, Relaxed)
                + added;
            self.metrics
                .output_queue_peak_capacity_bytes
                .fetch_max(allocated, Relaxed);
        }
        if self.writable(&mut state, true).is_err() {
            state.fatal = true;
        }
        if first {
            self.notify();
        }
        true
    }

    fn close(&self, reason: Disconnect) {
        let mut state = self.state.lock();
        if state.closed || state.requested.is_some() {
            return;
        }
        state.requested = Some(reason);
        self.notify();
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        let state = self.state.get_mut();
        let capacity = state.queue.capacity() * std::mem::size_of::<Pending>();
        if capacity != 0 {
            self.metrics
                .output_queue_capacity_bytes
                .fetch_sub(capacity, Relaxed);
        }
        let _ = self.registry.deregister(&mut state.stream);
    }
}

/// The codec owns receive/fragment/control state. This small handle serializes
/// its control writes with direct application writes on other workers.
pub(super) struct Stream {
    pub socket: Arc<Socket>,
    pub remaining_upgrade: Option<usize>,
}

impl Read for Stream {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let length = match self.remaining_upgrade {
            Some(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "HTTP upgrade exceeds 16384 bytes",
                ));
            }
            Some(remaining) => remaining.min(bytes.len()),
            None => bytes.len(),
        };
        let read = self.socket.state.lock().stream.read(&mut bytes[..length])?;
        if let Some(remaining) = &mut self.remaining_upgrade {
            *remaining -= read;
        }
        Ok(read)
    }
}

impl Write for Stream {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.socket.state.lock();
        if state.queue.front().is_some_and(|p| p.offset != 0) {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        state.control_pending = true;
        state
            .control_deadline
            .get_or_insert_with(|| Instant::now() + self.socket.write_timeout);
        match state.stream.write(bytes) {
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.socket.writable(&mut state, true)?;
                Err(error)
            }
            result => result,
        }
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn header(frame: &Frame, bytes: &mut [u8; 20]) -> usize {
    bytes[0] = 0x82;
    let length = frame.len();
    let size = match length {
        0..=125 => {
            bytes[1] = length as u8;
            2
        }
        126..=65535 => {
            bytes[1] = 126;
            bytes[2..4].copy_from_slice(&(length as u16).to_be_bytes());
            4
        }
        _ => {
            bytes[1] = 127;
            bytes[2..10].copy_from_slice(&(length as u64).to_be_bytes());
            10
        }
    };
    bytes[size..size + frame.prefix().len()].copy_from_slice(frame.prefix());
    size + frame.prefix().len()
}

fn wire_len(frame: &Frame) -> usize {
    frame.len()
        + match frame.len() {
            0..=125 => 2,
            126..=65535 => 4,
            _ => 10,
        }
}

/// Use the socket send interface rather than the general file write path.
/// `try_io` retains Mio's platform-specific WouldBlock/readiness bookkeeping.
struct SocketWriter<'a>(&'a TcpStream);

impl Write for SocketWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.try_io(|| socket2::SockRef::from(self.0).send(bytes))
    }

    fn write_vectored(&mut self, slices: &[IoSlice<'_>]) -> io::Result<usize> {
        self.0
            .try_io(|| socket2::SockRef::from(self.0).send_vectored(slices))
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write_pending(
    stream: &mut impl Write,
    queue: &VecDeque<Pending>,
    quantum: usize,
) -> io::Result<usize> {
    let pending = &queue[0];
    if queue.len() == 1 || wire_len(&pending.packet.frame) - pending.offset >= WRITE_BATCH_BYTES {
        let mut offset = pending.offset;
        return write_frame(stream, &pending.packet.frame, &mut offset, quantum);
    }
    write_batch(stream, queue, quantum.min(WRITE_BATCH_BYTES))
}

/// Combine only frames that already wait on this recipient. Headers use bounded
/// stack storage; payload slices keep their original shared owners. Offsets and
/// byte reservations advance only after the actual successful socket write.
fn write_batch(
    stream: &mut impl Write,
    queue: &VecDeque<Pending>,
    quantum: usize,
) -> io::Result<usize> {
    let mut headers = [[0; 20]; WRITE_BATCH_FRAMES];
    let mut sizes = [0; WRITE_BATCH_FRAMES];
    for (index, pending) in queue.iter().take(WRITE_BATCH_FRAMES).enumerate() {
        sizes[index] = header(&pending.packet.frame, &mut headers[index]);
    }
    let mut slices: [_; WRITE_BATCH_FRAMES * 2] = std::array::from_fn(|_| IoSlice::new(&[]));
    let mut count = 0;
    let mut remaining = quantum;
    for (index, pending) in queue.iter().take(WRITE_BATCH_FRAMES).enumerate() {
        let head = &headers[index][pending.offset.min(sizes[index])..sizes[index]];
        let head = &head[..head.len().min(remaining)];
        if !head.is_empty() {
            slices[count] = IoSlice::new(head);
            count += 1;
            remaining -= head.len();
        }
        let body = &pending.packet.frame.payload()[pending.offset.saturating_sub(sizes[index])..];
        let body = &body[..body.len().min(remaining)];
        if !body.is_empty() {
            slices[count] = IoSlice::new(body);
            count += 1;
            remaining -= body.len();
        }
        if remaining == 0 {
            break;
        }
    }
    let written = loop {
        match stream.write_vectored(&slices[..count]) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => break result?,
        }
    };
    if written == 0 {
        return Err(io::ErrorKind::WriteZero.into());
    }
    Ok(written)
}

/// The body is never assembled into a new contiguous message. Short writes
/// resume from the header/body offset and retain the original `Bytes` owner.
fn write_frame(
    stream: &mut impl Write,
    frame: &Frame,
    offset: &mut usize,
    quantum: usize,
) -> io::Result<usize> {
    let mut bytes = [0; 20];
    let size = header(frame, &mut bytes);
    let head = &bytes[(*offset).min(size)..size.min(offset.saturating_add(quantum))];
    let body_offset = offset.saturating_sub(size);
    let body = &frame.payload()[body_offset
        ..frame
            .payload()
            .len()
            .min(body_offset + quantum - head.len())];
    let slices = [IoSlice::new(head), IoSlice::new(body)];
    let written = loop {
        match stream.write_vectored(if head.is_empty() {
            &slices[1..]
        } else {
            &slices
        }) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => break result?,
        }
    };
    if written == 0 {
        return Err(io::ErrorKind::WriteZero.into());
    }
    *offset += written;
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Config, Relay, protocol};
    use mio::Poll;
    use tokio_tungstenite::tungstenite::{Error, Message, WebSocket, protocol::Role};

    struct Probe {
        wire: Vec<u8>,
        pointer: usize,
        borrowed: bool,
        max_write: usize,
        blocked: bool,
    }

    impl Write for Probe {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.write_vectored(&[IoSlice::new(bytes)])
        }
        fn write_vectored(&mut self, slices: &[IoSlice<'_>]) -> io::Result<usize> {
            // Alternate short writes and WouldBlock, including both sides of
            // the header/body boundary. Retrying must not replay any bytes.
            self.blocked = !self.blocked;
            if self.blocked {
                return Err(io::ErrorKind::WouldBlock.into());
            }
            let mut remaining = self.max_write;
            for slice in slices {
                self.borrowed |= slice.as_ptr() as usize == self.pointer;
                let n = remaining.min(slice.len());
                self.wire.extend_from_slice(&slice[..n]);
                remaining -= n;
            }
            Ok(self.max_write - remaining)
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn direct_vectored_writes_keep_bodies_borrowed_across_short_writes_and_backpressure() {
        for len in [0, 1, 115, 116, 125, 126, 65525, 65526, 65535, 65536, 65537] {
            let input = protocol::envelope(42, &vec![0x5a; len]);
            let packet = Frame::routed(u64::MAX, input, 3, false);
            let mut probe = Probe {
                wire: Vec::new(),
                pointer: packet.payload().as_ptr() as usize,
                borrowed: false,
                max_write: 3,
                blocked: false,
            };
            let mut offset = 0;
            while offset < wire_len(&packet) {
                match write_frame(&mut probe, &packet, &mut offset, 7) {
                    Ok(_) => {}
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) => panic!("{error}"),
                }
            }
            assert!(len == 0 || probe.borrowed);
            let mut client =
                WebSocket::from_raw_socket(io::Cursor::new(probe.wire), Role::Client, None);
            assert_eq!(
                client.read().unwrap(),
                Message::Binary(protocol::envelope(u64::MAX, &vec![0x5a; len]))
            );
        }
    }

    #[test]
    fn queued_batches_preserve_every_frame_offset_and_reservation_across_short_writes() {
        let lengths = [0, 1, 115, 116, 125, 126, 4096, 65536, 65537];
        for (max_write, quantum) in [(3, 7), (19, 31), (usize::MAX, WRITE_QUANTUM)] {
            let (relay, socket, reader, _poll) = connected();
            let registration = relay
                .subscribe_with_output(b"batch", socket.clone())
                .unwrap();
            let id = registration.peer.id.load(Relaxed);
            let mut reader = WebSocket::from_raw_socket(reader, Role::Client, None);
            assert_eq!(reader.read().unwrap(), Message::Binary(protocol::uint(id)));
            socket.state.lock().control_pending = true;
            for (index, length) in lengths.iter().enumerate() {
                let value = protocol::byte_string(&vec![index as u8; *length]);
                relay
                    .receive(&registration.peer, protocol::envelope(id, &value))
                    .unwrap();
            }
            let mut state = socket.state.lock();
            let retained = state.queue[0].packet.frame.clone();
            let mut probe = Probe {
                wire: Vec::new(),
                pointer: retained.payload().as_ptr() as usize,
                borrowed: false,
                max_write,
                blocked: false,
            };
            let mut writes = 0;
            while !state.queue.is_empty() {
                let before = relay.metrics.queued_bytes.load(Relaxed);
                let offset = state.queue[0].offset;
                match write_pending(&mut probe, &state.queue, quantum) {
                    Ok(written) => {
                        assert!(written <= quantum);
                        socket.advance_written(&mut state, written);
                        writes += 1;
                        let remaining: usize =
                            state.queue.iter().map(|p| p.packet.frame.len()).sum();
                        assert_eq!(
                            relay.metrics.queued_bytes.load(Relaxed),
                            remaining,
                            "partly written frames retain their complete charge"
                        );
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        assert_eq!(state.queue[0].offset, offset);
                        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), before);
                    }
                    Err(error) => panic!("{error}"),
                }
            }
            if max_write == usize::MAX {
                assert!(writes < lengths.len(), "queued small frames share writes");
            }
            assert!(probe.borrowed);
            assert_eq!(retained.payload().as_ptr() as usize, probe.pointer);
            assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
            assert_eq!(
                relay.metrics.deliveries.load(Relaxed),
                lengths.len() as u64 + 1
            );
            assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
            let mut decoded =
                WebSocket::from_raw_socket(io::Cursor::new(probe.wire), Role::Client, None);
            for (index, length) in lengths.iter().enumerate() {
                let value = protocol::byte_string(&vec![index as u8; *length]);
                assert_eq!(
                    decoded.read().unwrap(),
                    Message::Binary(protocol::envelope(id, &value))
                );
            }
        }
    }

    fn connected() -> (Arc<Relay>, Arc<Socket>, std::net::TcpStream, Poll) {
        let relay = Relay::new(Config {
            ping_interval_secs: 0,
            ..Config::default()
        })
        .unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let reader = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        let poll = Poll::new().unwrap();
        let registry = Arc::new(poll.registry().try_clone().unwrap());
        let signal = Arc::new(Signal {
            waker: Waker::new(&registry, Token(1)).unwrap(),
            stopping: AtomicBool::new(false),
            pending: Mutex::new(VecDeque::new()),
        });
        relay.metrics.connections.fetch_add(1, Relaxed);
        let socket = Socket::new(
            TcpStream::from_std(stream),
            Token(2),
            registry,
            signal,
            ConnectionGuard(relay.metrics.clone(), None),
            &relay,
        )
        .unwrap();
        (relay, socket, reader, poll)
    }

    #[test]
    fn recovered_queues_keep_small_capacity_but_release_large_empty_storage() {
        for messages in [16, 48] {
            let (relay, socket, reader, _poll) = connected();
            let registration = relay
                .subscribe_with_output(b"recover", socket.clone())
                .unwrap();
            let id = registration.peer.id.load(Relaxed);
            let mut reader = WebSocket::from_raw_socket(reader, Role::Client, None);
            assert_eq!(reader.read().unwrap(), Message::Binary(protocol::uint(id)));
            // Hold application output behind a pending control write, then
            // resume it through the same path used after a flushed Pong.
            socket.state.lock().control_pending = true;
            for i in 0..messages {
                relay
                    .receive(
                        &registration.peer,
                        protocol::envelope(id, &protocol::uint(i)),
                    )
                    .unwrap();
            }
            let before = socket.state.lock().queue.capacity();
            let retained = socket.state.lock().queue[0].packet.frame.clone();
            let pointer = retained.payload().as_ptr();
            assert_eq!(
                relay.metrics.output_queue_capacity_bytes.load(Relaxed),
                before * std::mem::size_of::<Pending>()
            );
            socket.control_flushed().unwrap();
            assert!(!socket.flush().unwrap());
            let after = socket.state.lock().queue.capacity();
            assert_eq!(
                after,
                if messages <= RETAIN_QUEUE_SLOTS as u64 {
                    before
                } else {
                    0
                }
            );
            assert_eq!(
                relay.metrics.output_queue_capacity_bytes.load(Relaxed),
                after * std::mem::size_of::<Pending>()
            );
            assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
            assert_eq!(retained.payload().as_ptr(), pointer);
            assert_eq!(retained.payload(), protocol::uint(0).as_ref());
            for i in 0..messages {
                assert_eq!(
                    reader.read().unwrap(),
                    Message::Binary(protocol::envelope(id, &protocol::uint(i)))
                );
            }
            drop(registration);
            drop(socket);
            assert_eq!(relay.metrics.output_queue_capacity_bytes.load(Relaxed), 0);
        }
    }

    #[test]
    fn an_exact_write_quantum_retires_empty_queue_storage_before_returning() {
        let (relay, socket, reader, _poll) = connected();
        socket2::SockRef::from(&socket.state.lock().stream)
            .set_send_buffer_size(1024 * 1024)
            .unwrap();
        let registration = relay
            .subscribe_with_output(b"quantum", socket.clone())
            .unwrap();
        let id = registration.peer.id.load(Relaxed);
        let mut reader = WebSocket::from_raw_socket(reader, Role::Client, None);
        assert_eq!(reader.read().unwrap(), Message::Binary(protocol::uint(id)));
        socket.state.lock().control_pending = true;
        let overhead = 1 + protocol::uint(id).len() + 3 + 4;
        let last = WRITE_QUANTUM - 48 * (4096 + overhead) - overhead;
        assert!((256..65500).contains(&last));
        let lengths = [vec![4096; 48], vec![last]].concat();
        for &len in &lengths {
            let value = protocol::byte_string(&vec![0xa5; len]);
            relay
                .receive(&registration.peer, protocol::envelope(id, &value))
                .unwrap();
        }
        assert_eq!(
            socket
                .state
                .lock()
                .queue
                .iter()
                .map(|p| wire_len(&p.packet.frame))
                .sum::<usize>(),
            WRITE_QUANTUM
        );
        socket.control_flushed().unwrap();
        assert!(!socket.flush().unwrap());
        assert_eq!(socket.state.lock().queue.capacity(), 0);
        assert_eq!(relay.metrics.output_queue_capacity_bytes.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        for len in lengths {
            assert_eq!(
                reader.read().unwrap(),
                Message::Binary(protocol::envelope(
                    id,
                    &protocol::byte_string(&vec![0xa5; len])
                ))
            );
        }
    }

    #[test]
    fn queued_output_keeps_payload_and_charge_after_membership_retires() {
        let (relay, socket, reader, _poll) = connected();
        reader
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let registration = relay
            .subscribe_with_output(b"retained", socket.clone())
            .unwrap();
        let id = registration.peer.id.load(Relaxed);
        let mut reader = WebSocket::from_raw_socket(reader, Role::Client, None);
        assert_eq!(reader.read().unwrap(), Message::Binary(protocol::uint(id)));
        reader.get_mut().set_nonblocking(true).unwrap();
        let body = protocol::byte_string(&vec![0x5a; 2 * WRITE_QUANTUM]);
        let input = protocol::envelope(id, &body);
        let charged = input.len();
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        relay.receive(&registration.peer, input).unwrap();
        {
            let state = socket.state.lock();
            assert_eq!(state.queue.len(), 1);
            assert!(state.queue[0].offset > 0);
            assert_eq!(state.queue[0].packet.frame.payload().as_ptr(), pointer);
        }
        drop(registration);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), charged);
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            socket.flush().unwrap();
            match reader.read() {
                Ok(message) => {
                    assert_eq!(message, Message::Binary(protocol::envelope(id, &body)));
                    break;
                }
                Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock => {
                    assert!(Instant::now() < deadline, "queued frame did not complete");
                    std::thread::yield_now();
                }
                other => panic!("{other:?}"),
            }
        }
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        assert_eq!(relay.metrics.deliveries.load(Relaxed), 2);
        assert_eq!(socket.state.lock().queue.len(), 0);
        assert!(!socket.end_output());
        drop(socket);
        assert_eq!(relay.metrics.connections.load(Relaxed), 0);
    }

    #[test]
    fn ordinary_output_has_no_queue_allocation_and_control_never_splits_a_partial_data_frame() {
        let (relay, socket, _reader, _poll) = connected();
        let registration = relay
            .subscribe_with_output(b"test", socket.clone())
            .unwrap();
        let id = registration.peer.id.load(Relaxed);
        relay
            .receive(&registration.peer, protocol::envelope(id, &[0xf6]))
            .unwrap();
        assert_eq!(socket.state.lock().queue.capacity(), 0);
        let body = protocol::byte_string(&vec![0x5a; 1024 * 1024]);
        relay
            .receive(&registration.peer, protocol::envelope(id, &body))
            .unwrap();
        let before = socket.state.lock().queue[0].offset;
        assert!(before > 0 && before <= WRITE_QUANTUM);
        let mut stream = Stream {
            socket: socket.clone(),
            remaining_upgrade: None,
        };
        assert_eq!(
            stream.write(b"control").unwrap_err().kind(),
            io::ErrorKind::WouldBlock
        );
        assert_eq!(socket.state.lock().queue[0].offset, before);
        assert!(relay.metrics.queued_bytes.load(Relaxed) > 0);
        assert!(
            socket.end_output(),
            "a partial frame requires dropping its transport"
        );
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        drop(registration);
        assert_eq!(relay.counts(), (0, 1));
    }
}
