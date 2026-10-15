//! Shared WebSocket output/close lifecycle for the CBOR relay and migration bridge.
use std::{future::Future, time::Duration};

use futures_util::{SinkExt, StreamExt};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::watch,
    time::timeout,
};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Error, Message, protocol::CloseFrame},
};
use tokio_util::sync::CancellationToken;

use crate::common::Disconnect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WriteFailure {
    Closed(Disconnect),
    TimedOut,
    Transport,
}

/// A cancelled output future may have partially written a frame. The caller must
/// drop that transport after any failure, rather than attempt a second flush.
pub(crate) async fn output(
    operation: impl Future<Output = Result<(), Error>>,
    duration: Duration,
    shutdown: &CancellationToken,
    closing: Option<&mut watch::Receiver<Option<Disconnect>>>,
) -> Result<(), WriteFailure> {
    let policy = async {
        let Some(receiver) = closing else {
            return std::future::pending::<Disconnect>().await;
        };
        match receiver.wait_for(Option::is_some).await {
            Ok(value) => value.expect("watch predicate requires a reason"),
            Err(_) => Disconnect::SHUTDOWN,
        }
    };
    tokio::select! {
        biased;
        _ = shutdown.cancelled() => Err(WriteFailure::Closed(Disconnect::SHUTDOWN)),
        reason = policy => Err(WriteFailure::Closed(reason)),
        result = timeout(duration, operation) => match result {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(WriteFailure::Transport),
            Err(_) => Err(WriteFailure::TimedOut),
        },
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
    use tokio::io::duplex;
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[tokio::test]
    async fn policy_interrupts_a_blocked_frame_without_waiting_for_write_timeout() {
        let (stream, _unread) = duplex(64);
        let mut ws = WebSocketStream::from_raw_socket(stream, Role::Server, None).await;
        let (closing, mut receiver) = watch::channel(None);
        let shutdown = CancellationToken::new();
        let write = output(
            ws.send(Message::Binary(vec![1; 4096].into())),
            Duration::from_secs(30),
            &shutdown,
            Some(&mut receiver),
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
                &shutdown,
                None
            )
            .await,
            Err(WriteFailure::TimedOut)
        );
    }
}
