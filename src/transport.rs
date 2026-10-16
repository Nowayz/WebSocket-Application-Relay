//! Asynchronous HTTP-upgrade and Close helpers for the migration bridge.
use std::{
    io::{self, IoSlice},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    time::timeout,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Error, Message, protocol::CloseFrame},
};
use tokio_util::sync::CancellationToken;

// Bound fragment parsing in the asynchronous migration bridge.
pub(crate) const READ_FRAMES_PER_POLL: std::num::NonZeroUsize =
    std::num::NonZeroUsize::new(256).unwrap();

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
