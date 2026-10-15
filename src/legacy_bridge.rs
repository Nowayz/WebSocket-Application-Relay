//! Optional migration adapter. The relay runtime itself only speaks CBOR.
use crate::{
    legacy,
    protocol::{self, BROADCAST, Disconnect},
};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use minicbor::{Decoder, Encoder, data::Type};
use std::{sync::Arc, time::Duration};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::Semaphore,
    task::JoinSet,
    time::timeout,
};
use tokio_tungstenite::{
    accept_async_with_config, connect_async,
    tungstenite::{
        Message,
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};
use tokio_util::sync::CancellationToken;

pub fn to_cbor(message: Message) -> Result<Option<Bytes>, Disconnect> {
    Ok(match message {
        Message::Binary(bytes) => {
            let target = legacy::binary_target(&bytes)?;
            if target != 0 {
                return Ok(Some(protocol::envelope(
                    target,
                    &protocol::byte_string(&bytes[8..]),
                )));
            }
            Some(match legacy::command(&bytes[8..])? {
                legacy::Command::Authenticate(secret) => {
                    protocol::request(0, &protocol::byte_string(secret))
                }
                legacy::Command::Listen(flags) => {
                    protocol::request(1, &protocol::uint(u64::from(flags)))
                }
                legacy::Command::Channels => protocol::request(2, &[0xf6]),
                legacy::Command::Assign(id) => protocol::request(3, &protocol::uint(id)),
                legacy::Command::SetVariable { key, value } => {
                    let mut e = Encoder::new(Vec::with_capacity(20 + key.len() + value.len()));
                    e.array(2)
                        .and_then(|e| e.bytes(key))
                        .and_then(|e| e.bytes(value))
                        .expect("Vec encoding");
                    protocol::request(4, &e.into_writer())
                }
                legacy::Command::GetVariable(key) => {
                    protocol::request(5, &protocol::byte_string(key))
                }
                legacy::Command::Ignore => return Ok(None),
            })
        }
        Message::Text(text) => {
            let target = legacy::text_target(&text)?;
            if target == 0 {
                None
            } else {
                Some(protocol::envelope(target, &protocol::text(&text[12..])))
            }
        }
        _ => {
            return Err(Disconnect {
                code: 1003,
                reason: "Type Unsupported",
            });
        }
    })
}

pub fn from_cbor(bytes: &[u8]) -> Result<Option<Message>> {
    let (sender, value) =
        protocol::parse(bytes).map_err(|_| anyhow::anyhow!("invalid upstream envelope"))?;
    let mut d = Decoder::new(value);
    if sender == BROADCAST {
        return Ok(Some(legacy::disconnect_event(d.u64()?)));
    }
    if sender == 0 {
        if d.array()? != Some(2) {
            bail!("invalid upstream command reply");
        }
        return Ok(match d.u8()? {
            0 | 1 | 3 | 4 => None, // Old clients do not expect command acknowledgements.
            2 => {
                let count = d.array()?.context("indefinite channel list")?;
                let mut names = Vec::new();
                let mut populations = Vec::new();
                for _ in 0..count {
                    if d.array()? != Some(2) {
                        bail!("invalid channel entry");
                    }
                    let name = d.bytes()?;
                    names.push(u8::try_from(name.len())?);
                    names.extend_from_slice(name);
                    populations.extend_from_slice(&u32::try_from(d.u64()?)?.to_le_bytes());
                }
                let mut reply = vec![0; 8];
                reply.extend_from_slice(&u32::try_from(count)?.to_le_bytes());
                reply.extend_from_slice(&names);
                reply.extend_from_slice(&populations);
                Some(Message::Binary(reply.into()))
            }
            200 => {
                if d.array()? != Some(2) {
                    bail!("invalid variable reply");
                }
                let found = d.bool()?;
                let value = if found {
                    match d.datatype()? {
                        Type::Bytes => d.bytes()?,
                        Type::String => d.str()?.as_bytes(),
                        _ => bail!("typed variable requires a CBOR client"),
                    }
                } else {
                    &[]
                };
                Some(legacy::variable_reply(value))
            }
            _ => bail!("unknown upstream command reply"),
        });
    }
    match d.datatype()? {
        Type::Bytes => {
            let payload = d.bytes()?;
            let mut bytes = Vec::with_capacity(8 + payload.len());
            bytes.extend_from_slice(&sender.to_le_bytes());
            bytes.extend_from_slice(payload);
            Ok(Some(Message::Binary(bytes.into())))
        }
        Type::String => {
            let payload = d.str()?;
            let mut text = STANDARD.encode(sender.to_le_bytes());
            text.push_str(payload);
            Ok(Some(Message::Text(text.into())))
        }
        _ => bail!("structured payload requires a CBOR client"),
    }
}

/// Bind on loopback by default. Bound admission, handshake/join/write time, and
/// frame sizes independently of the upstream relay's own limits.
pub async fn serve(
    listener: TcpListener,
    upstream: String,
    max_connections: usize,
    shutdown: CancellationToken,
) -> Result<()> {
    anyhow::ensure!(
        max_connections > 0 && max_connections <= Semaphore::MAX_PERMITS,
        "invalid bridge connection limit"
    );
    let slots = Arc::new(Semaphore::new(max_connections));
    let mut tasks = JoinSet::new();
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            incoming = listener.accept() => {
                let (socket, _) = incoming?;
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let upstream = upstream.clone(); let token = shutdown.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    tokio::select! {
                        _ = token.cancelled() => {},
                        result = bridge(socket, upstream) => { if let Err(error) = result { tracing::debug!(%error, "legacy bridge connection ended"); } }
                    }
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

async fn bridge(socket: TcpStream, upstream: String) -> Result<()> {
    const WAIT: Duration = Duration::from_secs(10);
    socket.set_nodelay(true)?;
    let config = WebSocketConfig::default()
        .read_buffer_size(4096)
        .write_buffer_size(4096)
        .max_write_buffer_size(17 * 1024 * 1024)
        .max_message_size(Some(16 * 1024 * 1024));
    let mut legacy = timeout(WAIT, accept_async_with_config(socket, Some(config))).await??;
    let joined = timeout(WAIT, async {
        loop {
            match legacy
                .next()
                .await
                .context("legacy client left before join")??
            {
                Message::Binary(bytes) => break Ok::<_, anyhow::Error>(bytes),
                Message::Text(text) => break Ok(Bytes::copy_from_slice(text.as_bytes())),
                Message::Ping(_) => legacy.flush().await?,
                Message::Pong(_) => {}
                _ => bail!("legacy client left before join"),
            }
        }
    })
    .await??;
    if joined.len() > 16 {
        legacy
            .close(Some(CloseFrame {
                code: CloseCode::Protocol,
                reason: "Channel Length Exceeded".into(),
            }))
            .await?;
        return Ok(());
    }
    let (mut upstream, _) = timeout(WAIT, connect_async(upstream)).await??;
    upstream
        .send(Message::Binary(protocol::byte_string(&joined)))
        .await?;
    let welcome = timeout(WAIT, upstream.next())
        .await
        .context("upstream welcome timeout")?
        .context("upstream closed before welcome")??;
    let Message::Binary(welcome) = welcome else {
        bail!("invalid upstream welcome");
    };
    let id = Decoder::new(&welcome).u64()?;
    legacy
        .send(Message::Binary(Bytes::copy_from_slice(&id.to_le_bytes())))
        .await?;
    loop {
        tokio::select! {
            incoming = legacy.next() => match incoming {
                Some(Ok(Message::Ping(_))) => { timeout(WAIT, legacy.flush()).await??; }
                Some(Ok(Message::Pong(_))) => {}
                Some(Ok(Message::Close(frame))) => { let _ = timeout(WAIT, upstream.close(frame)).await; break; }
                Some(Ok(message)) => match to_cbor(message) {
                    Ok(Some(packet)) => timeout(WAIT, upstream.send(Message::Binary(packet))).await??,
                    Ok(None) => {},
                    Err(reason) => { let _ = timeout(WAIT, legacy.close(Some(CloseFrame { code: reason.code.into(), reason: reason.reason.into() }))).await; break; }
                }
                _ => break,
            },
            incoming = upstream.next() => match incoming {
                Some(Ok(Message::Binary(bytes))) => match from_cbor(&bytes) {
                    Ok(Some(message)) => timeout(WAIT, legacy.send(message)).await??,
                    Ok(None) => {},
                    Err(_) => { let _ = timeout(WAIT, legacy.close(Some(CloseFrame { code: CloseCode::Unsupported, reason: "Value Requires CBOR Client".into() }))).await; break; }
                }
                Some(Ok(Message::Ping(_))) => { timeout(WAIT, upstream.flush()).await??; }
                Some(Ok(Message::Pong(_))) => {},
                Some(Ok(Message::Close(frame))) => { let _ = timeout(WAIT, legacy.close(frame)).await; break; }
                _ => break,
            }
        }
    }
    Ok(())
}
