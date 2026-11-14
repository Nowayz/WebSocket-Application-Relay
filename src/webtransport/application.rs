//! Explicit established-session data; pin ownership remains in the async shell.
use super::{Output, Pending, QueuedOutput, StreamOutput, frame_chunks, poll_output_chunks, queue};
use crate::{
    Relay,
    protocol::Disconnect,
    relay::{Envelope, FANOUT_QUANTUM, Registration, RoutingFailure},
    server::RateLimiter,
};
use anyhow::Result;
use bytes::Bytes;
use parking_lot::Mutex;
use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, atomic::Ordering::Relaxed},
    task::{Context, Poll},
    time::{Duration, Instant},
};
use tokio::io::{AsyncRead, ReadBuf};

pub(super) fn poll_budget_available(cx: &mut Context<'_>) -> Poll<()> {
    if tokio::task::coop::has_budget_remaining() {
        return Poll::Ready(());
    }
    match tokio::task::coop::poll_proceed(cx) {
        Poll::Pending => Poll::Pending,
        Poll::Ready(restore) => {
            drop(restore);
            Poll::Ready(())
        }
    }
}

#[derive(Default)]
pub(super) struct RecordReader {
    header: [u8; 4],
    header_read: usize,
    size: Option<usize>,
    body: RecordBody,
    body_read: usize,
    lease: Option<tungstenite::protocol::ReadLease>,
}

enum RecordBody {
    Direct(Vec<u8>),
    Arena(crate::receive_arena::ArenaBuffer),
}

impl Default for RecordBody {
    fn default() -> Self {
        Self::Direct(Vec::new())
    }
}

impl RecordBody {
    fn len(&self) -> usize {
        match self {
            Self::Direct(v) => v.len(),
            Self::Arena(v) => v.as_ref().len(),
        }
    }

    fn publish(self) -> Bytes {
        match self {
            Self::Direct(v) => Bytes::from(v),
            Self::Arena(v) => v.publish(),
        }
    }

    fn as_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Direct(v) => v,
            Self::Arena(v) => v.as_mut(),
        }
    }
}

impl RecordReader {
    pub(super) fn poll<S: AsyncRead + Unpin>(
        &mut self,
        stream: &mut S,
        maximum: usize,
        budget: Option<&Arc<tungstenite::protocol::ReadBudget>>,
        arena: Option<&crate::receive_arena::ReceiveArena>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<Bytes>> {
        while self.header_read < self.header.len() {
            let mut read = ReadBuf::new(&mut self.header[self.header_read..]);
            match Pin::new(&mut *stream).poll_read(cx, &mut read) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error.into())),
                Poll::Ready(Ok(())) => {}
            }
            if read.filled().is_empty() {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "early eof",
                )
                .into()));
            }
            self.header_read += read.filled().len();
        }
        let size = if let Some(size) = self.size {
            size
        } else {
            let size = u32::from_be_bytes(self.header) as usize;
            if size > maximum {
                return Poll::Ready(Err(anyhow::anyhow!("invalid record length")));
            }
            self.lease = match budget
                .map(|budget| budget.try_lease(size.saturating_mul(2)))
                .transpose()
            {
                Ok(lease) => lease,
                Err(error) => return Poll::Ready(Err(error.into())),
            };
            self.body = arena.and_then(|a| a.allocate(size)).map_or_else(
                || RecordBody::Direct(Vec::with_capacity(size.min(4096))),
                RecordBody::Arena,
            );
            self.body_read = 0;
            self.size = Some(size);
            size
        };
        while self.body_read < size {
            if self.body_read == self.body.len() {
                let count = (size - self.body.len()).min(4096);
                let start = self.body.len();
                if let RecordBody::Direct(body) = &mut self.body {
                    if start + count > body.capacity() {
                        let capacity = body.capacity().saturating_mul(2).min(size);
                        body.reserve_exact(capacity.max(start + count) - start);
                    }
                    body.resize(start + count, 0);
                }
            }
            let mut read = ReadBuf::new(&mut self.body.as_mut()[self.body_read..]);
            match Pin::new(&mut *stream).poll_read(cx, &mut read) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error.into())),
                Poll::Ready(Ok(())) => {}
            }
            if read.filled().is_empty() {
                return Poll::Ready(Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "early eof",
                )
                .into()));
            }
            self.body_read += read.filled().len();
        }
        let bytes = std::mem::take(&mut self.body).publish();
        self.lease = None;
        self.header_read = 0;
        self.body_read = 0;
        self.size = None;
        Poll::Ready(Ok(bytes))
    }
}

struct RouteRun<'a> {
    pending: Pending<'a>,
    result: Result<usize, RoutingFailure>,
}

impl RouteRun<'_> {
    fn finish(self) -> Result<()> {
        self.result.map(|_| ()).map_err(|error| {
            self.pending.registration.peer.close(error.reason);
            anyhow::anyhow!(error.reason.reason)
        })
    }
}

enum InputPhase<'a> {
    Read,
    Route(RouteRun<'a>),
    QuantumYield,
}

pub(super) struct InputState<'a> {
    phase: InputPhase<'a>,
    remaining: usize,
    yielding: bool,
}

impl<'a> InputState<'a> {
    pub(super) fn new(quantum: usize) -> Self {
        Self {
            phase: InputPhase::Read,
            remaining: quantum,
            yielding: false,
        }
    }

    pub(super) fn begin(
        &mut self,
        relay: &Relay,
        registration: &'a Registration,
        activity: &Option<Arc<Mutex<Instant>>>,
        limiter: &mut RateLimiter,
        bytes: Bytes,
        unreliable: bool,
    ) -> Result<()> {
        assert!(matches!(self.phase, InputPhase::Read));
        if let Some(activity) = activity {
            *activity.lock() = Instant::now();
        }
        if !limiter.allow(bytes.len(), Instant::now()) {
            relay.metrics.rate_limited.fetch_add(1, Relaxed);
            registration.peer.close(Disconnect::RATE);
            anyhow::bail!("rate exceeded");
        }
        let mut pending = Pending {
            registration,
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
        self.phase = InputPhase::Route(RouteRun { pending, result });
        Ok(())
    }

    pub(super) fn poll_ready(
        &mut self,
        cx: &mut Context<'_>,
        quantum: usize,
        mut poll_yield: impl FnMut(&mut Context<'_>, bool) -> Poll<()>,
    ) -> Poll<Result<()>> {
        loop {
            if matches!(self.phase, InputPhase::Read) {
                return Poll::Ready(Ok(()));
            }
            if matches!(self.phase, InputPhase::QuantumYield) {
                let reset = !self.yielding;
                self.yielding = true;
                if poll_yield(cx, reset).is_pending() {
                    return Poll::Pending;
                }
                self.yielding = false;
                self.remaining = quantum;
                self.phase = InputPhase::Read;
                continue;
            }
            let active = match &self.phase {
                InputPhase::Route(run) => {
                    run.pending.work.as_ref().is_some_and(|work| work.active())
                }
                _ => unreachable!(),
            };
            if self.yielding || active {
                let reset = !self.yielding;
                self.yielding = true;
                if poll_yield(cx, reset).is_pending() {
                    return Poll::Pending;
                }
                self.yielding = false;
                let InputPhase::Route(run) = &mut self.phase else {
                    unreachable!()
                };
                if let Err(error) = run
                    .pending
                    .work
                    .as_mut()
                    .expect("yielded fanout")
                    .resume(FANOUT_QUANTUM)
                {
                    return Poll::Ready(Err(anyhow::anyhow!(error.reason)));
                }
                continue;
            }
            let InputPhase::Route(run) = std::mem::replace(&mut self.phase, InputPhase::Read)
            else {
                unreachable!()
            };
            if let Err(error) = run.finish() {
                return Poll::Ready(Err(error));
            }
            self.remaining -= 1;
            if self.remaining == 0 {
                self.phase = InputPhase::QuantumYield;
            }
        }
    }
}

pub(super) struct WriteAttempt<'a> {
    chunks: Option<[Bytes; 2]>,
    packet: Option<Envelope>,
    output: &'a Output,
    offset: usize,
}

impl<'a> WriteAttempt<'a> {
    fn new(output: &'a Output, packet: Envelope, offset: usize) -> Self {
        Self {
            chunks: None,
            packet: Some(packet),
            output,
            offset,
        }
    }
}

impl Future for WriteAttempt<'_> {
    type Output = Result<Envelope>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        if this.chunks.is_none() {
            this.chunks = Some(frame_chunks(
                &this.packet.as_ref().expect("active packet").frame,
                this.offset,
            ));
        }
        match poll_output_chunks(this.output, this.chunks.as_mut().expect("write chunks"), cx) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
            Poll::Ready(Ok(())) => Poll::Ready(Ok(this.packet.take().expect("uncompleted write"))),
        }
    }
}

enum WriterPhase {
    Waiting,
    Writing,
    Yielding(Envelope),
    Ack([Bytes; 1]),
    Done,
}

pub(super) struct WriterState<'a> {
    phase: WriterPhase,
    outgoing: queue::Receiver<'a, QueuedOutput, StreamOutput>,
    remaining: usize,
    quantum: usize,
    draining: bool,
    yielding: bool,
    selecting: bool,
}

impl<'a> WriterState<'a> {
    pub(super) fn new(
        outgoing: queue::Receiver<'a, QueuedOutput, StreamOutput>,
        quantum: usize,
    ) -> Self {
        Self {
            phase: WriterPhase::Waiting,
            outgoing,
            remaining: quantum,
            quantum,
            draining: false,
            yielding: false,
            selecting: false,
        }
    }

    pub(super) fn begin_drain(&mut self) {
        self.draining = true;
    }

    pub(super) fn poll(
        &mut self,
        cx: &mut Context<'_>,
        output: &'a Output,
        mut writing: Pin<&mut Option<tokio::time::Timeout<WriteAttempt<'a>>>>,
        mut poll_yield: impl FnMut(&mut Context<'_>, bool) -> Poll<()>,
    ) -> Poll<Result<()>> {
        loop {
            match &mut self.phase {
                WriterPhase::Waiting => {
                    if self.selecting || !self.draining {
                        self.selecting = true;
                        if poll_budget_available(cx).is_pending() {
                            return Poll::Pending;
                        }
                    }
                    if self.draining {
                        self.outgoing.close();
                    }
                    let take = |packet, owner: &mut StreamOutput| {
                        assert!(!owner.busy);
                        owner.busy = true;
                        packet
                    };
                    let (packet, was_idle) = match self.outgoing.poll_recv_with_idle_map(cx, take) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(packet) => packet,
                    };
                    self.selecting = false;
                    let Some(queued) = packet else {
                        if self.draining {
                            self.phase = WriterPhase::Ack([Bytes::from_static(&[0, 0, 0, 0])]);
                            continue;
                        }
                        self.phase = WriterPhase::Done;
                        return Poll::Ready(Ok(()));
                    };
                    if was_idle {
                        self.remaining = self.quantum;
                    }
                    let remaining = Duration::from_secs(output.relay.config.write_timeout_secs)
                        .saturating_sub(
                            queued
                                .started
                                .map_or(Duration::ZERO, |started| started.elapsed()),
                        );
                    let attempt = WriteAttempt::new(output, queued.packet, queued.offset);
                    writing
                        .as_mut()
                        .set(Some(tokio::time::timeout(remaining, attempt)));
                    self.phase = WriterPhase::Writing;
                }
                WriterPhase::Writing => {
                    let result = match writing
                        .as_mut()
                        .as_pin_mut()
                        .expect("active write")
                        .poll(cx)
                    {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(result) => result,
                    };
                    writing.as_mut().set(None);
                    let packet = match result {
                        Err(error) => {
                            output.relay.metrics.write_timeouts.fetch_add(1, Relaxed);
                            return Poll::Ready(Err(error.into()));
                        }
                        Ok(Err(error)) => {
                            output.relay.metrics.write_errors.fetch_add(1, Relaxed);
                            return Poll::Ready(Err(error));
                        }
                        Ok(Ok(packet)) => packet,
                    };
                    output.delivered(&packet.frame);
                    self.remaining -= 1;
                    if self.remaining == 0 {
                        self.phase = WriterPhase::Yielding(packet);
                    } else {
                        drop(packet);
                        self.phase = WriterPhase::Waiting;
                    }
                }
                WriterPhase::Yielding(_) => {
                    let reset = !self.yielding;
                    self.yielding = true;
                    if poll_yield(cx, reset).is_pending() {
                        return Poll::Pending;
                    }
                    self.yielding = false;
                    self.remaining = self.quantum;
                    let WriterPhase::Yielding(packet) =
                        std::mem::replace(&mut self.phase, WriterPhase::Waiting)
                    else {
                        unreachable!()
                    };
                    drop(packet);
                }
                WriterPhase::Ack(chunks) => {
                    match poll_output_chunks(output, chunks, cx) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                        Poll::Ready(Ok(())) => {}
                    }
                    self.phase = WriterPhase::Done;
                    return Poll::Ready(Ok(()));
                }
                WriterPhase::Done => panic!("polled completed writer"),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fragmented {
        bytes: Vec<u8>,
        position: usize,
        pause: bool,
    }

    impl AsyncRead for Fragmented {
        fn poll_read(
            self: Pin<&mut Self>,
            cx: &mut Context<'_>,
            read: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            let this = self.get_mut();
            if this.pause {
                this.pause = false;
                cx.waker().wake_by_ref();
                return Poll::Pending;
            }
            let count = read
                .remaining()
                .min(3)
                .min(this.bytes.len() - this.position);
            read.put_slice(&this.bytes[this.position..this.position + count]);
            this.position += count;
            this.pause = true;
            Poll::Ready(Ok(()))
        }
    }

    #[test]
    fn fragmented_record_completion_and_cancellation_preserve_the_budget_and_eof_contract() {
        for arena_enabled in [false, true] {
            let arena = crate::receive_arena::ReceiveArena::new(32768, 16384);
            let arena = arena_enabled.then_some(&arena);
            let payload: Vec<_> = (0..9000).map(|index| (index % 251) as u8).collect();
            let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
            bytes.extend_from_slice(&payload);
            bytes.extend_from_slice(&[0; 4]);
            let budget = tungstenite::protocol::ReadBudget::new(payload.len() * 2);
            let mut cx = Context::from_waker(std::task::Waker::noop());
            let mut cancelled = RecordReader::default();
            let mut stream = Fragmented {
                bytes: bytes.clone(),
                position: 0,
                pause: false,
            };
            while budget.reserved() == 0 {
                assert!(
                    cancelled
                        .poll(&mut stream, payload.len(), Some(&budget), arena, &mut cx)
                        .is_pending()
                );
            }
            assert_eq!(budget.reserved(), payload.len() * 2);
            drop(cancelled);
            assert_eq!(budget.reserved(), 0);

            let mut reader = RecordReader::default();
            let mut stream = Fragmented {
                bytes,
                position: 0,
                pause: false,
            };
            let received = loop {
                if let Poll::Ready(result) =
                    reader.poll(&mut stream, payload.len(), Some(&budget), arena, &mut cx)
                {
                    break result.unwrap();
                }
            };
            assert_eq!(received.as_ref(), payload.as_slice());
            assert_eq!(budget.reserved(), 0);
            assert_eq!(budget.peak(), payload.len() * 2);
            let close = loop {
                if let Poll::Ready(result) =
                    reader.poll(&mut stream, payload.len(), Some(&budget), arena, &mut cx)
                {
                    break result.unwrap();
                }
            };
            assert!(close.is_empty());
            let eof = loop {
                if let Poll::Ready(result) =
                    reader.poll(&mut stream, payload.len(), Some(&budget), arena, &mut cx)
                {
                    break result.unwrap_err();
                }
            };
            assert_eq!(
                eof.downcast_ref::<std::io::Error>().unwrap().kind(),
                std::io::ErrorKind::UnexpectedEof
            );
            assert_eq!(budget.reserved(), 0);
        }
    }
}
