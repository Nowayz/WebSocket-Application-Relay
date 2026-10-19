//! Socket output has one serialized owner at a time, including writes from a
//! different relay worker. Private traffic writes borrowed slices immediately.
//! Fanout is queued to its owner for bounded zero-copy vectored batches.
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
    relay::{Envelope, PeerOutput, Submission},
};

pub(super) const WRITE_QUANTUM: usize = 256 * 1024;
const RETAIN_QUEUE_SLOTS: usize = 32;

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

impl State {
    pub fn deadline(&self) -> Option<Instant> {
        match (
            self.queue.front().map(|p| p.deadline),
            self.control_deadline,
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        }
    }

    // Called for registered sockets with idle tracking enabled. Pending output
    // postpones idle expiry, without extending its separate write deadline.
    pub fn idle_expired(&mut self, now: Instant, timeout: Duration) -> bool {
        if self.activity + timeout > now {
            return false;
        }
        if self.deadline().is_none() {
            return true;
        }
        self.activity = now;
        false
    }
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
            let mut pending = self.signal.pending.lock();
            let wake = pending.is_empty();
            pending.push_back(self.token);
            drop(pending);
            if wake {
                let _ = self.signal.waker.wake();
            }
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

    fn delivered(&self, frame: &Frame, state: &mut State) {
        self.metrics.deliveries.fetch_add(1, Relaxed);
        self.metrics
            .delivered_bytes
            .fetch_add(frame.len() as u64, Relaxed);
        if self.track_activity {
            state.activity = Instant::now();
        }
    }

    pub fn deadline(&self) -> Option<Instant> {
        self.state.lock().deadline()
    }

    /// Resume bounded application output. `true` means this socket consumed its
    /// quantum while still ready and must be explicitly scheduled again.
    pub fn flush(&self) -> io::Result<bool> {
        let mut state = self.state.lock();
        if state.control_pending {
            return Ok(false);
        }
        self.flush_locked(&mut state)
    }

    /// A pending application-output wake need not enter the input decoder.
    /// Check terminal/control/deadline policy under the same lock as the write;
    /// uncertain cases return to the complete connection state machine.
    pub fn flush_output_only(
        &self,
        now: Instant,
        idle_timeout: Duration,
    ) -> Option<io::Result<bool>> {
        let mut state = self.state.lock();
        if state.fatal
            || state.requested.is_some()
            || state.control_pending
            || state.control_deadline.is_some_and(|d| d <= now)
            || state.queue.front().is_some_and(|p| p.deadline <= now)
            || (!idle_timeout.is_zero() && state.activity + idle_timeout <= now)
        {
            return None;
        }
        Some(self.flush_locked(&mut state))
    }

    fn flush_locked(&self, state: &mut State) -> io::Result<bool> {
        let mut remaining = WRITE_QUANTUM;
        while !state.queue.is_empty() {
            let State { stream, queue, .. } = &mut *state;
            match write_batch(&mut SocketWriter(stream), queue, remaining) {
                Ok((written, delivered, bytes)) => {
                    remaining -= written;
                    if delivered != 0 {
                        self.metrics.deliveries.fetch_add(delivered, Relaxed);
                        self.metrics.delivered_bytes.fetch_add(bytes, Relaxed);
                        if self.track_activity {
                            state.activity = Instant::now();
                        }
                    }
                    if remaining == 0 {
                        let queued = !state.queue.is_empty();
                        self.writable(state, queued)?;
                        return Ok(queued);
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    self.writable(state, true)?;
                    return Ok(false);
                }
                Err(error) => return Err(error),
            }
        }
        self.writable(state, false)?;
        Ok(false)
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
    #[inline(always)]
    fn submit<const DEFER: bool, const BARRIER: bool>(&self, packet: Submission<'_>) -> bool {
        let mut state = self.state.lock();
        if state.closed || state.fatal || state.requested.is_some() {
            return true;
        }
        if BARRIER
            && state.queue.len() >= self.queue_limit
            && !state.control_pending
            && state
                .queue
                .front()
                .is_some_and(|p| p.deadline > Instant::now())
            && self.flush_locked(&mut state).is_err()
        {
            state.fatal = true;
            self.metrics.write_errors.fetch_add(1, Relaxed);
            self.notify();
            return true;
        }
        if state.queue.len() >= self.queue_limit {
            return false;
        }
        let mut offset = 0;
        if !DEFER && state.queue.is_empty() && !state.control_pending {
            match write_frame(
                &mut SocketWriter(&state.stream),
                packet.frame,
                &mut offset,
                WRITE_QUANTUM,
            ) {
                Ok(_) if offset == wire_len(packet.frame) => {
                    self.delivered(packet.frame, &mut state);
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
            packet: packet.into_owned(),
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
        if !DEFER && self.writable(&mut state, true).is_err() {
            state.fatal = true;
        }
        if first {
            self.notify();
        }
        true
    }
}

impl PeerOutput for Socket {
    fn supports_retirement(&self) -> bool {
        true
    }
    fn wake_retirement(&self) {
        let _ = self.signal.waker.wake();
    }

    fn send(&self, packet: Submission<'_>) -> bool {
        self.submit::<false, false>(packet)
    }

    fn send_barrier(&self, packet: Submission<'_>) -> bool {
        self.submit::<false, true>(packet)
    }

    fn send_fanout(&self, packet: Submission<'_>) -> bool {
        self.submit::<true, false>(packet)
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

// Body-bearing frames have at most ten prefix bytes: 10 WebSocket + 10 CBOR.
// Longer inline scalar prefixes have no body and need only a two-byte header.
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

// Avoid the large batching stack for the common single-entry deferred queue.
#[inline(never)]
fn write_batch(
    stream: &mut impl Write,
    queue: &mut VecDeque<Pending>,
    quantum: usize,
) -> io::Result<(usize, u64, u64)> {
    if queue.len() == 1 {
        let pending = queue.front_mut().expect("single queued frame");
        let written = write_frame(stream, &pending.packet.frame, &mut pending.offset, quantum)?;
        if pending.offset == wire_len(&pending.packet.frame) {
            let bytes = pending.packet.frame.len() as u64;
            queue.pop_front();
            return Ok((written, 1, bytes));
        }
        return Ok((written, 0, 0));
    }
    write_many(stream, queue, quantum)
}

// At most 16 frames / 32 iovecs and WRITE_QUANTUM bytes per call. Headers
// live on the stack, bodies remain borrowed from the charged queue entries.
// A short write retires only complete frames and advances one partial frame.
// Keep the batching stack and code outside the ordinary empty-queue/private path.
#[inline(never)]
fn write_many(
    stream: &mut impl Write,
    queue: &mut VecDeque<Pending>,
    quantum: usize,
) -> io::Result<(usize, u64, u64)> {
    const FRAMES: usize = 16;
    let mut headers = [[0; 20]; FRAMES];
    let mut lengths = [0; FRAMES];
    for (i, pending) in queue.iter().take(FRAMES).enumerate() {
        lengths[i] = header(&pending.packet.frame, &mut headers[i]);
    }
    let written = {
        let mut slices: [IoSlice<'_>; FRAMES * 2] = std::array::from_fn(|_| IoSlice::new(&[]));
        let mut count = 0;
        let mut remaining = quantum;
        for (i, pending) in queue.iter().take(FRAMES).enumerate() {
            let head_start = pending.offset.min(lengths[i]);
            let head_end = lengths[i].min(head_start + remaining);
            let head = &headers[i][head_start..head_end];
            if !head.is_empty() {
                slices[count] = IoSlice::new(head);
                count += 1;
                remaining -= head.len();
            }
            let body = pending.packet.frame.payload();
            let start = pending.offset.saturating_sub(lengths[i]);
            let end = body.len().min(start + remaining);
            if end > start {
                slices[count] = IoSlice::new(&body[start..end]);
                count += 1;
                remaining -= end - start;
            }
            if remaining == 0 {
                break;
            }
        }
        loop {
            match stream.write_vectored(&slices[..count]) {
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                result => break result?,
            }
        }
    };
    if written == 0 {
        return Err(io::ErrorKind::WriteZero.into());
    }
    let mut remaining = written;
    let mut delivered = 0;
    let mut bytes = 0;
    while remaining != 0 {
        let pending = queue.front_mut().expect("write bounded by queued slices");
        let size = wire_len(&pending.packet.frame) - pending.offset;
        let used = remaining.min(size);
        pending.offset += used;
        remaining -= used;
        if used == size {
            bytes += pending.packet.frame.len() as u64;
            delivered += 1;
            queue.pop_front();
        }
    }
    Ok((written, delivered, bytes))
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
    fn stack_headers_fit_inline_controls_and_body_length_boundaries() {
        fn check(frame: Frame, expected: bytes::Bytes) -> (usize, u8) {
            let mut head = [0; 20];
            let head_len = header(&frame, &mut head);
            assert!(head_len <= head.len());
            let mut wire = Vec::new();
            let mut offset = 0;
            while offset < wire_len(&frame) {
                write_frame(&mut wire, &frame, &mut offset, WRITE_QUANTUM).unwrap();
            }
            assert_eq!(offset, wire.len());
            let width = wire[1];
            let mut client = WebSocket::from_raw_socket(io::Cursor::new(wire), Role::Client, None);
            assert_eq!(client.read().unwrap(), Message::Binary(expected));
            (head_len, width)
        }

        for value in [0, 23, 24, 255, 256, 65535, 65536, u32::MAX as u64, u64::MAX] {
            check(Frame::integer(value), protocol::uint(value));
            for op in [0, 23, 24, 200, 255] {
                check(
                    Frame::reply_uint(op, value),
                    protocol::reply(op, &protocol::uint(value)),
                );
            }
        }
        assert_eq!(Frame::reply_uint(255, u64::MAX).prefix().len(), 14);
        let mut widths = [false; 3];
        let mut maximum_header = 0;
        for size in [
            0, 23, 24, 113, 114, 116, 117, 118, 119, 65522, 65523, 65524, 65525, 65526, 65527,
            65528,
        ] {
            let value = protocol::byte_string(&vec![0x5a; size]);
            for (frame, expected) in [
                (
                    Frame::envelope(u64::MAX - 1, value.clone()),
                    protocol::envelope(u64::MAX - 1, &value),
                ),
                (
                    Frame::reply(255, value.clone()),
                    protocol::reply(255, &value),
                ),
                (
                    Frame::variable(Some(value.clone())),
                    protocol::reply(200, &[b"\x82\xf5".as_slice(), value.as_ref()].concat()),
                ),
            ] {
                let (head_len, width) = check(frame, expected);
                maximum_header = maximum_header.max(head_len);
                widths[match width {
                    0..=125 => 0,
                    126 => 1,
                    127 => 2,
                    _ => unreachable!(),
                }] = true;
            }
        }
        assert_eq!(widths, [true; 3]);
        assert_eq!(maximum_header, 20);
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
    fn output_only_flush_defers_control_close_and_expired_policy_to_the_decoder() {
        let (_relay, socket, _reader, _poll) = connected();
        let now = Instant::now();
        socket.state.lock().fatal = true;
        assert!(socket.flush_output_only(now, Duration::ZERO).is_none());
        socket.state.lock().fatal = false;
        socket.state.lock().requested = Some(Disconnect::OVERLOADED);
        assert!(socket.flush_output_only(now, Duration::ZERO).is_none());
        socket.state.lock().requested = None;
        socket.state.lock().control_pending = true;
        assert!(socket.flush_output_only(now, Duration::ZERO).is_none());
        socket.state.lock().control_pending = false;
        socket.state.lock().control_deadline = Some(now);
        assert!(socket.flush_output_only(now, Duration::ZERO).is_none());
        socket.state.lock().control_deadline = None;
        socket.state.lock().activity = now - Duration::from_secs(2);
        assert!(
            socket
                .flush_output_only(now, Duration::from_secs(1))
                .is_none()
        );
        assert!(
            !socket
                .flush_output_only(now, Duration::ZERO)
                .unwrap()
                .unwrap()
        );
    }

    #[test]
    fn multicast_waits_for_its_owner_while_private_output_stays_immediate() {
        let (relay, socket, raw_reader, _poll) = connected();
        let receiver = relay
            .subscribe_with_output(b"owner", socket.clone())
            .unwrap();
        struct Source;
        impl PeerOutput for Source {
            fn send(&self, _packet: Submission<'_>) -> bool {
                true
            }
            fn close(&self, _reason: Disconnect) {}
        }
        let sender = relay
            .subscribe_with_output(b"owner", Arc::new(Source))
            .unwrap();
        let id = receiver.peer.id.load(Relaxed);
        let sender_id = sender.peer.id.load(Relaxed);
        let mut reader = WebSocket::from_raw_socket(raw_reader, Role::Client, None);
        assert_eq!(reader.read().unwrap(), Message::Binary(protocol::uint(id)));
        let value = protocol::byte_string(&[0x5a; 1024]);
        let input = protocol::envelope(protocol::BROADCAST, &value);
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        let now = Instant::now();
        let idle_timeout = Duration::from_secs(1);
        socket.state.lock().activity = now - idle_timeout;
        let stale_output_pending = socket.deadline().is_some();
        assert!(!stale_output_pending);
        // Admit output on another thread between the former output/activity reads.
        let writer_relay = relay.clone();
        let writer = sender.peer.clone();
        std::thread::spawn(move || writer_relay.receive(&writer, input))
            .join()
            .unwrap()
            .unwrap();
        {
            let mut state = socket.state.lock();
            assert_eq!(state.queue.len(), 1);
            assert_eq!(state.queue[0].offset, 0);
            assert_eq!(state.queue[0].packet.frame.payload().as_ptr(), pointer);
            assert!(!stale_output_pending && state.activity + idle_timeout <= now);
            let write_deadline = state.deadline();
            assert!(!state.idle_expired(now, idle_timeout));
            assert_eq!(state.activity, now);
            assert_eq!(state.deadline(), write_deadline);
        }
        assert!(relay.metrics.queued_bytes.load(Relaxed) > 0);
        assert!(
            !socket
                .flush_output_only(Instant::now(), Duration::ZERO)
                .unwrap()
                .unwrap()
        );
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        assert_eq!(
            reader.read().unwrap(),
            Message::Binary(protocol::envelope(sender_id, &value))
        );
        relay
            .receive(&sender.peer, protocol::envelope(id, &value))
            .unwrap();
        assert!(socket.state.lock().queue.is_empty());
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
        assert_eq!(
            reader.read().unwrap(),
            Message::Binary(protocol::envelope(sender_id, &value))
        );
    }

    #[test]
    fn idle_expiry_preserves_exact_boundary_and_pending_control_deadline() {
        let (_relay, socket, _reader, _poll) = connected();
        let now = Instant::now();
        let timeout = Duration::from_secs(1);
        let mut state = socket.state.lock();
        state.activity = now;
        assert!(!state.idle_expired(now, timeout));
        state.activity = now - timeout;
        assert!(state.idle_expired(now, timeout));
        assert_eq!(state.activity, now - timeout);
        let write_deadline = now + Duration::from_secs(5);
        state.control_pending = true;
        state.control_deadline = Some(write_deadline);
        assert!(!state.idle_expired(now, timeout));
        assert_eq!(state.activity, now);
        assert_eq!(state.deadline(), Some(write_deadline));
    }

    #[test]
    fn batched_output_preserves_fifo_bodies_and_charges_across_short_writes() {
        for max_write in [1, 3, 17, 1037, 32768] {
            let (relay, socket, raw_reader, _poll) = connected();
            let registration = relay
                .subscribe_with_output(b"batch", socket.clone())
                .unwrap();
            let id = registration.peer.id.load(Relaxed);
            let mut reader = WebSocket::from_raw_socket(raw_reader, Role::Client, None);
            assert_eq!(reader.read().unwrap(), Message::Binary(protocol::uint(id)));
            socket.state.lock().control_pending = true;
            let values: Vec<_> = (0..20)
                .map(|i| {
                    protocol::byte_string(&vec![i as u8; [0, 1, 115, 126, 1024, 65536][i % 6]])
                })
                .collect();
            for value in &values {
                relay
                    .receive(&registration.peer, protocol::envelope(id, value))
                    .unwrap();
            }
            let mut queue = std::mem::take(&mut socket.state.lock().queue);
            let pointers: Vec<_> = queue
                .iter()
                .map(|p| p.packet.frame.payload().as_ptr())
                .collect();
            let mut probe = Probe {
                wire: Vec::new(),
                pointer: pointers[0] as usize,
                borrowed: false,
                max_write,
                blocked: false,
            };
            let mut completed = 0;
            while !queue.is_empty() {
                assert_eq!(
                    relay.metrics.queued_bytes.load(Relaxed),
                    queue.iter().map(|p| p.packet.frame.len()).sum::<usize>()
                );
                assert_eq!(
                    queue.front().unwrap().packet.frame.payload().as_ptr(),
                    pointers[completed]
                );
                match write_batch(&mut probe, &mut queue, 4093) {
                    Ok((n, done, _)) => {
                        assert!(n <= 4093);
                        completed += done as usize;
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("{e}"),
                }
            }
            socket.state.lock().queue = queue;
            assert_eq!(completed, values.len());
            assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
            assert!(probe.borrowed);
            let mut decoded =
                WebSocket::from_raw_socket(io::Cursor::new(probe.wire), Role::Client, None);
            for value in values {
                assert_eq!(
                    decoded.read().unwrap(),
                    Message::Binary(protocol::envelope(id, &value))
                );
            }
        }
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

    #[test]
    fn registration_failure_closes_the_socket_and_releases_physical_admission() {
        let relay = Relay::new(Config::default()).unwrap();
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let mut reader = std::net::TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        reader
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let (stream, _) = listener.accept().unwrap();
        stream.set_nonblocking(true).unwrap();
        let mut stream = TcpStream::from_std(stream);
        let poll = Poll::new().unwrap();
        let registry = Arc::new(poll.registry().try_clone().unwrap());
        // A second registration of this descriptor fails in the real poller.
        // No preload library or unsafe syscall interception is needed.
        registry
            .register(&mut stream, Token(2), Interest::READABLE)
            .unwrap();
        let signal = Arc::new(Signal {
            waker: Waker::new(&registry, Token(1)).unwrap(),
            stopping: AtomicBool::new(false),
            pending: Mutex::new(VecDeque::new()),
        });
        relay.metrics.connections.fetch_add(1, Relaxed);
        let result = Socket::new(
            stream,
            Token(3),
            registry,
            signal,
            ConnectionGuard(relay.metrics.clone(), None),
            &relay,
        );
        assert!(result.is_err(), "duplicate registration must fail");
        assert_eq!(
            reader.read(&mut [0]).unwrap(),
            0,
            "failed setup must close TCP"
        );
        assert_eq!(relay.metrics.connections.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }
}
