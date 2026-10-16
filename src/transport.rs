//! Shared WebSocket output/close lifecycle for the CBOR relay and migration bridge.
use std::{
    future::Future,
    io::{self, IoSlice},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use bytes::Bytes;
use futures_util::{FutureExt, SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf},
    sync::watch,
    time::timeout,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Error, Message, protocol::CloseFrame},
};
use tokio_util::sync::CancellationToken;

use crate::{common::Disconnect, frame::Frame};

/// Bound the raw HTTP upgrade without parsing or copying it a second time.
/// After upgrade, reads and all vectored writes delegate directly to the socket.
pub(crate) struct HandshakeStream<S> {
    stream: S,
    remaining: Option<usize>,
}

impl<S> HandshakeStream<S> {
    pub(crate) fn new(stream: S) -> Self {
        Self {
            stream,
            remaining: Some(crate::config::MAX_HTTP_UPGRADE_BYTES),
        }
    }

    pub(crate) fn handshake_done(&mut self) {
        self.remaining = None;
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for HandshakeStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let Some(remaining) = self.remaining else {
            return Pin::new(&mut self.stream).poll_read(cx, buf);
        };
        if buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        if remaining == 0 {
            return Poll::Ready(Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP upgrade exceeds 16384 bytes",
            )));
        }
        let length = remaining.min(buf.remaining());
        let mut limited = ReadBuf::new(buf.initialize_unfilled_to(length));
        let result = Pin::new(&mut self.stream).poll_read(cx, &mut limited);
        let read = limited.filled().len();
        buf.advance(read);
        self.remaining = Some(remaining - read);
        result
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for HandshakeStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }
    fn poll_write_vectored(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write_vectored(cx, bufs)
    }
    fn is_write_vectored(&self) -> bool {
        self.stream.is_write_vectored()
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

/// Native server transport with explicit permission to write application data.
///
/// The session owns all writes sequentially. Flush the decoder's automatic
/// control replies first, then bypass its contiguous application-output buffer.
/// Receive/fragment/control state stays in tungstenite. This endpoint negotiates
/// no compression or other framing extensions. Any incomplete write must drop
/// the transport; `output` and its callers enforce that rule.
pub(crate) struct ServerSocket<S> {
    websocket: WebSocketStream<S>,
    active: bool,
    pending_control: bool,
}

impl<S> ServerSocket<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    /// Only wrap a completed server handshake without negotiated extensions.
    pub(crate) fn new(websocket: WebSocketStream<S>) -> Self {
        Self {
            websocket,
            active: true,
            pending_control: false,
        }
    }

    pub(crate) async fn next(&mut self) -> Option<Result<Message, Error>> {
        let message = self.websocket.next().await;
        if matches!(message, Some(Ok(Message::Ping(_)))) {
            self.pending_control = true;
        }
        if !matches!(&message, Some(Ok(message)) if !matches!(message, Message::Close(_))) {
            self.active = false;
        }
        message
    }

    pub(crate) async fn flush(&mut self) -> Result<(), Error> {
        self.websocket.flush().await?;
        self.pending_control = false;
        Ok(())
    }

    pub(crate) fn reclaim_read_buffer(&mut self) -> Option<usize> {
        self.websocket.reclaim_read_buffer()
    }

    pub(crate) async fn ping(&mut self, payload: Bytes) -> Result<(), Error> {
        if !self.active {
            return Err(Error::AlreadyClosed);
        }
        self.websocket.send(Message::Ping(payload)).await?;
        self.pending_control = false;
        Ok(())
    }

    /// End application output before entering the shared close-handshake code.
    pub(crate) fn closing(&mut self) -> &mut WebSocketStream<S> {
        self.active = false;
        &mut self.websocket
    }

    /// Closing path only, after all output either completed or never started.
    /// Poll one already available input item so a queued peer Close can win a
    /// simultaneous capacity decision. Other in-flight input is discarded, as
    /// it would be during `initiate_close`; incomplete reads remain in the codec.
    pub(crate) fn take_pending_close(&mut self) -> bool {
        matches!(
            self.next().now_or_never(),
            Some(Some(Ok(Message::Close(_))))
        )
    }

    /// One final, unmasked binary frame. The body stays borrowed until completion.
    pub(crate) async fn binary(&mut self, frame: &Frame) -> Result<(), Error> {
        if !self.active {
            return Err(Error::AlreadyClosed);
        }
        if self.pending_control {
            self.flush().await?;
        }
        // The two tiny headers are contiguous stack storage; only the shared
        // application body needs a second I/O slice.
        let mut header = [0u8; 20];
        header[0] = 0x82; // FIN | binary. Servers never mask application output.
        let len = frame.len();
        let header_len = match len {
            0..=125 => {
                header[1] = len as u8;
                2
            }
            126..=65535 => {
                header[1] = 126;
                header[2..4].copy_from_slice(&(len as u16).to_be_bytes());
                4
            }
            _ => {
                header[1] = 127;
                header[2..10].copy_from_slice(&(len as u64).to_be_bytes());
                10
            }
        };
        let combined_len = header_len + frame.prefix().len();
        header[header_len..combined_len].copy_from_slice(frame.prefix());
        // Exactly two slices, including short writes across their boundary.
        // A non-vectored transport may write just the first slice each time.
        // No application payload is copied or allocated here.
        let mut slices = [
            IoSlice::new(&header[..combined_len]),
            IoSlice::new(frame.payload()),
        ];
        let mut remaining = slices.as_mut_slice();
        let socket = self.websocket.get_mut();
        while !remaining.is_empty() {
            if remaining[0].is_empty() {
                remaining = &mut remaining[1..];
                continue;
            }
            let written = socket.write_vectored(remaining).await?;
            if written == 0 {
                return Err(std::io::Error::from(std::io::ErrorKind::WriteZero).into());
            }
            IoSlice::advance_slices(&mut remaining, written);
        }
        socket.flush().await?;
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteFailure {
    ClosedBeforeStart(Disconnect),
    Closed(Disconnect),
    TimedOut,
    Transport,
}

/// One terminal session signal. Keep this future pinned for the connection's
/// lifetime so unrelated data messages do not repeatedly register/drop waiters.
pub(crate) async fn close_reason(mut receiver: watch::Receiver<Option<Disconnect>>) -> Disconnect {
    match receiver.wait_for(Option::is_some).await {
        Ok(value) => value.expect("watch predicate requires a reason"),
        Err(_) => Disconnect::SHUTDOWN,
    }
}

/// After polling begins, a failed output may have written part of a frame and
/// must discard its transport. A terminal signal that wins before the first
/// poll leaves the stream untouched and permits a normal Close handshake.
/// Operations must perform their I/O only when polled.
pub(crate) async fn output(
    operation: impl Future<Output = Result<(), Error>>,
    duration: Duration,
    stopped: impl Future<Output = ()>,
    closing: impl Future<Output = Disconnect>,
) -> Result<(), WriteFailure> {
    let mut started = false;
    tokio::pin!(operation);
    let result = {
        let tracked = std::future::poll_fn(|cx| {
            started = true;
            operation.as_mut().poll(cx)
        });
        tokio::select! {
        biased;
        _ = stopped => Err(WriteFailure::Closed(Disconnect::SHUTDOWN)),
        reason = closing => Err(WriteFailure::Closed(reason)),
        result = timeout(duration, tracked) => match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(WriteFailure::Transport),
            Err(_) => Err(WriteFailure::TimedOut),
        },
        }
    };
    match result {
        Err(WriteFailure::Closed(reason)) if !started => {
            Err(WriteFailure::ClosedBeforeStart(reason))
        }
        result => result,
    }
}

/// Reading Close already queued the exact acknowledgement. Sending a fresh Close
/// here would be SendAfterClosing and would leave the acknowledgement unflushed.
pub(crate) async fn acknowledge_close<S>(ws: &mut WebSocketStream<S>, duration: Duration) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    matches!(timeout(duration, ws.flush()).await, Ok(Ok(())))
}

/// Use one deadline for sending Close and receiving its acknowledgement. An
/// already requested service shutdown skips waiting after the Close is sent.
pub(crate) async fn initiate_close<S>(
    ws: &mut WebSocketStream<S>,
    frame: Option<CloseFrame>,
    duration: Duration,
    shutdown: Option<&CancellationToken>,
) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let result = timeout(duration, async {
        ws.close(frame).await?;
        let stopped = async {
            match shutdown {
                Some(token) => token.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::select! {
            _ = stopped => Ok(()),
            result = async {
                while let Some(message) = ws.next().await {
                    match message? {
                        Message::Close(_) => { ws.flush().await?; return Ok(()); }
                        Message::Ping(_) => ws.flush().await?,
                        _ => {} // In-flight application data is discarded after admission ends.
                    }
                }
                Ok::<_, Error>(())
            } => result,
        }
    })
    .await;
    matches!(
        result,
        Ok(Ok(())) | Ok(Err(Error::ConnectionClosed | Error::AlreadyClosed))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::ReadBuf;
    use tokio::io::duplex;
    use tokio_tungstenite::tungstenite::protocol::Role;

    /// A short-writing transport proves slices reach I/O unchanged and that
    /// progress survives writes stopping inside either header or the body.
    struct Probe {
        wire: Vec<u8>,
        payload_pointer: usize,
        saw_borrowed_payload: bool,
        max_write: usize,
    }

    impl AsyncRead for Probe {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Pending
        }
    }

    impl AsyncWrite for Probe {
        fn poll_write(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            bytes: &[u8],
        ) -> Poll<io::Result<usize>> {
            let n = bytes.len().min(self.max_write);
            self.saw_borrowed_payload |= bytes.as_ptr() as usize == self.payload_pointer;
            self.wire.extend_from_slice(&bytes[..n]);
            Poll::Ready(Ok(n))
        }

        fn poll_write_vectored(
            mut self: Pin<&mut Self>,
            _: &mut Context<'_>,
            slices: &[io::IoSlice<'_>],
        ) -> Poll<io::Result<usize>> {
            let mut remaining = self.max_write;
            for slice in slices {
                self.saw_borrowed_payload |= slice.as_ptr() as usize == self.payload_pointer;
                let n = slice.len().min(remaining);
                self.wire.extend_from_slice(&slice[..n]);
                remaining -= n;
                if remaining == 0 {
                    break;
                }
            }
            Poll::Ready(Ok(self.max_write - remaining))
        }

        fn is_write_vectored(&self) -> bool {
            true
        }

        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn vectored_binary_output_preserves_buffers_and_all_length_boundaries() {
        for len in [0, 1, 115, 116, 125, 126, 65525, 65526, 65535, 65536, 65537] {
            let input = crate::protocol::envelope(42, &vec![0x5a; len]);
            let packet = Frame::routed(u64::MAX, input, 3, false);
            let probe = Probe {
                wire: Vec::new(),
                payload_pointer: packet.payload().as_ptr() as usize,
                saw_borrowed_payload: false,
                max_write: 3,
            };
            let mut ws = ServerSocket::new(
                WebSocketStream::from_raw_socket(probe, Role::Server, None).await,
            );
            ws.binary(&packet).await.unwrap();
            let probe = ws.websocket.get_ref();
            assert!(len == 0 || probe.saw_borrowed_payload);
            let mut client = tokio_tungstenite::tungstenite::WebSocket::from_raw_socket(
                io::Cursor::new(probe.wire.clone()),
                Role::Client,
                None,
            );
            let Message::Binary(output) = client.read().unwrap() else {
                panic!("binary frame required")
            };
            assert_eq!(
                output.as_ref(),
                crate::protocol::envelope(u64::MAX, &vec![0x5a; len]).as_ref()
            );
        }
    }

    #[tokio::test]
    async fn pending_control_reply_precedes_direct_data_and_close_blocks_further_data() {
        let (server, client) = duplex(1024);
        let mut server =
            ServerSocket::new(WebSocketStream::from_raw_socket(server, Role::Server, None).await);
        let mut client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        client
            .send(Message::Ping(bytes::Bytes::from_static(b"ping")))
            .await
            .unwrap();
        assert!(matches!(server.next().await, Some(Ok(Message::Ping(_)))));
        let packet = Frame::encoded(bytes::Bytes::from_static(b"body"));
        server.binary(&packet).await.unwrap();
        assert_eq!(
            client.next().await.unwrap().unwrap(),
            Message::Pong(bytes::Bytes::from_static(b"ping"))
        );
        assert_eq!(
            client.next().await.unwrap().unwrap(),
            Message::Binary(bytes::Bytes::from_static(b"body"))
        );
        client.close(None).await.unwrap();
        assert!(matches!(server.next().await, Some(Ok(Message::Close(_)))));
        assert!(server.binary(&packet).await.is_err());
        assert!(acknowledge_close(server.closing(), Duration::from_secs(1)).await);
        assert!(matches!(client.next().await, Some(Ok(Message::Close(_)))));
    }

    #[tokio::test]
    async fn policy_interrupts_a_blocked_frame_without_waiting_for_write_timeout() {
        let (stream, _unread) = duplex(64);
        let mut ws =
            ServerSocket::new(WebSocketStream::from_raw_socket(stream, Role::Server, None).await);
        let packet = Frame::encoded(vec![1; 4096].into());
        let (closing, receiver) = watch::channel(None);
        let shutdown = CancellationToken::new();
        let stopped = shutdown.cancelled();
        let reason = close_reason(receiver);
        tokio::pin!(stopped, reason);
        let write = output(
            ws.binary(&packet),
            Duration::from_secs(30),
            stopped.as_mut(),
            reason.as_mut(),
        );
        tokio::pin!(write);
        assert!(
            timeout(Duration::from_millis(20), &mut write)
                .await
                .is_err()
        );
        closing.send_replace(Some(Disconnect::OVERLOADED));
        assert_eq!(
            timeout(Duration::from_millis(200), write).await.unwrap(),
            Err(WriteFailure::Closed(Disconnect::OVERLOADED))
        );
    }

    #[tokio::test]
    async fn a_blocked_write_has_one_deadline_and_preserves_its_timeout_cause() {
        let (stream, _unread) = duplex(64);
        let mut ws = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
        let shutdown = CancellationToken::new();
        assert_eq!(
            output(
                ws.send(Message::Binary(vec![1; 4096].into())),
                Duration::from_millis(20),
                shutdown.cancelled(),
                std::future::pending()
            )
            .await,
            Err(WriteFailure::TimedOut)
        );
    }

    #[tokio::test]
    async fn signals_survive_successful_outputs_and_still_cancel_a_blocked_write() {
        let (stream, _unread) = duplex(256);
        let mut ws = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
        let (closing, receiver) = watch::channel(None);
        let shutdown = CancellationToken::new();
        let stopped = shutdown.cancelled();
        let reason = close_reason(receiver);
        tokio::pin!(stopped, reason);
        for i in 0..8 {
            assert_eq!(
                output(
                    ws.send(Message::Binary(vec![i].into())),
                    Duration::from_secs(30),
                    stopped.as_mut(),
                    reason.as_mut()
                )
                .await,
                Ok(())
            );
        }
        let write = output(
            ws.send(Message::Binary(vec![1; 4096].into())),
            Duration::from_secs(30),
            stopped.as_mut(),
            reason.as_mut(),
        );
        tokio::pin!(write);
        assert!(
            timeout(Duration::from_millis(20), &mut write)
                .await
                .is_err()
        );
        closing.send_replace(Some(Disconnect::TAKEN));
        assert_eq!(
            timeout(Duration::from_millis(200), write).await.unwrap(),
            Err(WriteFailure::Closed(Disconnect::TAKEN))
        );
    }

    #[tokio::test]
    async fn an_already_closed_signal_prevents_an_immediately_ready_output() {
        let (_closing, receiver) = watch::channel(Some(Disconnect::OVERLOADED));
        let shutdown = CancellationToken::new();
        let attempted = std::cell::Cell::new(false);
        let operation = async {
            attempted.set(true);
            Ok(())
        };
        assert_eq!(
            output(
                operation,
                Duration::from_secs(30),
                shutdown.cancelled(),
                close_reason(receiver)
            )
            .await,
            Err(WriteFailure::ClosedBeforeStart(Disconnect::OVERLOADED))
        );
        assert!(!attempted.get());
    }

    #[tokio::test]
    async fn cancellation_before_first_poll_preserves_a_queued_peer_close() {
        let (server, client) = duplex(1024);
        let mut server =
            ServerSocket::new(WebSocketStream::from_raw_socket(server, Role::Server, None).await);
        let mut client = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        client
            .close(Some(CloseFrame {
                code: 4003.into(),
                reason: "finished together".into(),
            }))
            .await
            .unwrap();
        let (_closing, receiver) = watch::channel(Some(Disconnect::OVERLOADED));
        let packet = Frame::encoded(Bytes::from_static(b"unused"));
        assert_eq!(
            output(
                server.binary(&packet),
                Duration::from_secs(1),
                std::future::pending(),
                close_reason(receiver)
            )
            .await,
            Err(WriteFailure::ClosedBeforeStart(Disconnect::OVERLOADED))
        );
        assert!(server.take_pending_close());
        assert!(acknowledge_close(server.closing(), Duration::from_secs(1)).await);
        assert!(
            matches!(client.next().await.unwrap().unwrap(), Message::Close(Some(frame)) if u16::from(frame.code) == 4003 && frame.reason == "finished together")
        );
    }
}
