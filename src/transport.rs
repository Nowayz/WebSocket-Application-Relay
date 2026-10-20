//! Native accept retry policy and test WebSocket close helpers.
#[cfg(test)]
use futures_util::{SinkExt, StreamExt};
use std::{io, time::Duration};
#[cfg(test)]
use tokio::{
    io::{AsyncRead, AsyncWrite},
    time::timeout,
};
#[cfg(test)]
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{Error, Message, protocol::CloseFrame},
};
#[cfg(test)]
use tokio_util::sync::CancellationToken;

pub(crate) fn accept_retry_delay(error: &io::Error) -> Duration {
    match error.kind() {
        io::ErrorKind::Interrupted
        | io::ErrorKind::ConnectionRefused
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::ConnectionReset => Duration::ZERO,
        _ => Duration::from_secs(1),
    }
}

/// Reading Close already queued the exact acknowledgement. Sending a fresh Close
/// here would be SendAfterClosing and would leave the acknowledgement unflushed.
#[cfg(test)]
pub(crate) async fn acknowledge_close<S>(ws: &mut WebSocketStream<S>, duration: Duration) -> bool
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    matches!(timeout(duration, ws.flush()).await, Ok(Ok(())))
}

/// Use one deadline for sending Close and receiving its acknowledgement. An
/// already requested service shutdown skips waiting after the Close is sent.
#[cfg(test)]
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
