//! Bounded decoder steps preserve protocol progress and explicitly reschedule.
use std::{
    future::Future,
    io::{self, Cursor, Read, Write},
    num::NonZeroUsize,
    pin::{Pin, pin},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering::Relaxed},
    },
    task::{Context, Poll},
};

use futures_util::{StreamExt, task::ArcWake};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{
        Error, Message, WebSocket,
        protocol::{ReadBudget, Role, WebSocketConfig},
    },
};

struct Memory {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
    reads: usize,
}

impl Memory {
    fn new(input: Vec<u8>) -> Self {
        Self {
            input: Cursor::new(input),
            output: Vec::new(),
            reads: 0,
        }
    }
}

impl Read for Memory {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        self.reads += 1;
        self.input.read(bytes)
    }
}

impl Write for Memory {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.output.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AsyncRead for Memory {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let read = Read::read(&mut *self, bytes.initialize_unfilled())?;
        bytes.advance(read);
        Poll::Ready(Ok(()))
    }
}

impl AsyncWrite for Memory {
    fn poll_write(
        mut self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(Write::write(&mut *self, bytes))
    }

    fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }

    fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
        Poll::Ready(Ok(()))
    }
}

fn frame(opcode: u8, body: &[u8]) -> Vec<u8> {
    assert!(body.len() < 126);
    let mut wire = vec![opcode, 0x80 | body.len() as u8, 0, 0, 0, 0];
    wire.extend_from_slice(body);
    wire
}

#[derive(Default)]
struct WakeCount(AtomicUsize);

impl ArcWake for WakeCount {
    fn wake_by_ref(this: &Arc<Self>) {
        this.0.fetch_add(1, Relaxed);
    }
}

#[test]
fn bounded_sync_reads_preserve_utf8_control_and_unlimited_resume() {
    let mut wire = frame(0x01, &[0xe2]);
    wire.extend(frame(0, &[]));
    wire.extend(frame(0x89, b"ping"));
    wire.extend(frame(0, &[0x82]));
    wire.extend(frame(0, &[]));
    wire.extend(frame(0x80, &[0xac, b'x']));
    let pool = ReadBudget::new(32768);
    let mut socket = WebSocket::from_raw_socket(
        Memory::new(wire),
        Role::Server,
        Some(WebSocketConfig::default().read_buffer_size(4096)),
    );
    socket
        .set_read_budget(pool.try_lease(8192).unwrap())
        .unwrap();
    let limit = NonZeroUsize::new(2).unwrap();
    assert_eq!(socket.read_with_frame_limit(limit).unwrap(), None);
    assert_eq!(
        socket.read_with_frame_limit(limit).unwrap(),
        Some(Message::Ping(b"ping".as_slice().into()))
    );
    assert_eq!(socket.read_with_frame_limit(limit).unwrap(), None);
    // Opting back into ordinary reads resumes the same incomplete codepoint.
    assert_eq!(socket.read().unwrap(), Message::text("€x"));
    assert_eq!(socket.get_ref().output, b"\x8a\x04ping");
    assert_eq!(socket.get_ref().reads, 1);
    assert_eq!(pool.rejected(), 0);
    drop(socket);
    assert_eq!(pool.reserved(), 0);
}

#[tokio::test]
async fn buffered_fragments_yield_and_wake_without_needing_an_io_event() {
    let mut wire = frame(0x02, &[1]);
    for _ in 0..5 {
        wire.extend(frame(0, &[]));
    }
    wire.extend(frame(0x80, &[2, 3]));
    let mut socket = WebSocketStream::from_raw_socket(
        Memory::new(wire),
        Role::Server,
        Some(WebSocketConfig::default().read_buffer_size(4096)),
    )
    .await;
    let pool = ReadBudget::new(32768);
    socket
        .set_read_budget(pool.try_lease(8192).unwrap())
        .unwrap();
    socket.set_read_frame_limit(NonZeroUsize::new(2));
    let counter = Arc::new(WakeCount::default());
    let waker = futures_util::task::waker(counter.clone());
    let mut context = Context::from_waker(&waker);
    for expected_wakes in 1..=3 {
        // Drop each pending future as select! would when another branch wins.
        let mut next = pin!(socket.next());
        assert!(next.as_mut().poll(&mut context).is_pending());
        assert_eq!(counter.0.load(Relaxed), expected_wakes);
    }
    assert_eq!(socket.get_ref().reads, 1);
    let Some(Ok(Message::Binary(published))) = socket.next().await else {
        panic!("assembled binary")
    };
    assert_eq!(published.as_ref(), &[1, 2, 3]);
    let pointer = published.as_ptr();
    assert_eq!(socket.reclaim_read_buffer(), Some(0));
    drop(socket);
    assert_eq!(pool.reserved(), 0);
    assert_eq!(published.as_ptr(), pointer);
    assert_eq!(published.as_ref(), &[1, 2, 3]);
}

#[tokio::test]
async fn yielding_does_not_hide_protocol_errors_or_unfuse_a_failed_stream() {
    let mut wire = frame(0x02, &[1]);
    // A new data message cannot replace unfinished assembly.
    wire.extend(frame(0x82, &[2]));
    let mut socket = WebSocketStream::from_raw_socket(Memory::new(wire), Role::Server, None).await;
    socket.set_read_frame_limit(NonZeroUsize::new(1));
    assert!(matches!(socket.next().await, Some(Err(Error::Protocol(_)))));
    assert!(socket.next().await.is_none());
}
