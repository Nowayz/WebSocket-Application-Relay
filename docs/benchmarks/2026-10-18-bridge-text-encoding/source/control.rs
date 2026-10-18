//! Optional migration adapter. The relay runtime itself only speaks CBOR.
use crate::{
    legacy,
    protocol::{self, BROADCAST, Disconnect},
    transport,
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
    accept_hdr_async_with_config, connect_async_with_config,
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
                // Write the envelope and value once, directly into their final storage.
                let payload = &bytes[8..];
                let capacity = 1
                    + protocol::uint_len(target)
                    + protocol::uint_len(payload.len() as u64)
                    + payload.len();
                let mut e = Encoder::new(Vec::with_capacity(capacity));
                e.array(2)
                    .and_then(|e| e.u64(target))
                    .and_then(|e| e.bytes(payload))
                    .expect("Vec encoding");
                return Ok(Some(e.into_writer().into()));
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
    serve_with_origin_policy(
        listener,
        upstream,
        max_connections,
        crate::origin::OriginPolicy::default(),
        shutdown,
    )
    .await
}

/// Serve the original wire protocol with an optional browser-origin boundary.
/// Apply the policy at this public ingress; the upstream bridge connection has
/// no browser Origin and must not be relied on to enforce downstream origins.
pub async fn serve_with_origin_policy(
    listener: TcpListener,
    upstream: String,
    max_connections: usize,
    origin_policy: crate::origin::OriginPolicy,
    shutdown: CancellationToken,
) -> Result<()> {
    serve_with_limits(
        listener,
        upstream,
        max_connections,
        0,
        origin_policy,
        shutdown,
    )
    .await
}

/// Optional direct-peer admission at the public legacy ingress. Forwarding
/// headers do not change the address observed on the accepted TCP connection.
pub async fn serve_with_limits(
    listener: TcpListener,
    upstream: String,
    max_connections: usize,
    max_connections_per_ip: usize,
    origin_policy: crate::origin::OriginPolicy,
    shutdown: CancellationToken,
) -> Result<()> {
    origin_policy.validate()?;
    anyhow::ensure!(
        max_connections_per_ip <= u32::MAX as usize,
        "max-connections-per-ip must be in 0..=4294967295"
    );
    anyhow::ensure!(
        max_connections > 0 && max_connections <= Semaphore::MAX_PERMITS,
        "invalid bridge connection limit"
    );
    let slots = Arc::new(Semaphore::new(max_connections));
    let origin_policy = Arc::new(origin_policy);
    let ip_limit = crate::admission::IpLimit::new(max_connections_per_ip);
    let mut tasks = JoinSet::new();
    let mut accept_retry = None;
    loop {
        tokio::select! {
            biased;
            _ = shutdown.cancelled() => break,
            _ = tasks.join_next(), if !tasks.is_empty() => {},
            incoming = async {
                if let Some(deadline) = accept_retry {
                    tokio::time::sleep_until(deadline).await;
                }
                listener.accept().await
            } => {
                let (socket, address) = match incoming {
                    Ok(incoming) => {
                        accept_retry = None;
                        incoming
                    }
                    Err(error) => {
                        let delay = transport::accept_retry_delay(&error);
                        accept_retry = (!delay.is_zero())
                            .then(|| tokio::time::Instant::now() + delay);
                        if !delay.is_zero() {
                            tracing::warn!(%error, "legacy accept failed; retrying in one second");
                        }
                        continue;
                    }
                };
                let Ok(permit) = slots.clone().try_acquire_owned() else { continue; };
                let ip_permit = if let Some(limit) = &ip_limit {
                    let Some(permit) = limit.acquire(address.ip()) else { continue; };
                    Some(permit)
                } else { None };
                let socket = transport::HandshakeStream::new(socket, ip_permit);
                let upstream = upstream.clone(); let token = shutdown.clone();
                let origin_policy = origin_policy.clone();
                tasks.spawn(async move {
                    let _permit = permit;
                    tokio::select! {
                        _ = token.cancelled() => {},
                        result = bridge(socket, upstream, &origin_policy) => { if let Err(error) = result { tracing::debug!(%error, "legacy bridge connection ended"); } }
                    }
                });
            }
        }
    }
    tasks.abort_all();
    while tasks.join_next().await.is_some() {}
    Ok(())
}

async fn bridge(
    socket: transport::HandshakeStream<TcpStream>,
    upstream: String,
    origin_policy: &crate::origin::OriginPolicy,
) -> Result<()> {
    const WAIT: Duration = Duration::from_secs(10);
    socket.get_ref().set_nodelay(true)?;
    let config = WebSocketConfig::default()
        .read_buffer_size(4096)
        .write_buffer_size(4096)
        .max_write_buffer_size(17 * 1024 * 1024)
        .max_message_size(Some(16 * 1024 * 1024));
    let mut legacy = timeout(
        WAIT,
        accept_hdr_async_with_config(
            socket,
            |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                if origin_policy.allows(request.headers()) {
                    Ok(response)
                } else {
                    Err(crate::origin::forbidden())
                }
            },
            Some(config),
        ),
    )
    .await??;
    legacy.get_mut().handshake_done();
    legacy.set_read_frame_limit(Some(transport::READ_FRAMES_PER_POLL));
    let joined = timeout(WAIT, async {
        loop {
            match legacy
                .next()
                .await
                .context("legacy client left before join")??
            {
                Message::Binary(bytes) => break Ok::<_, anyhow::Error>(Some(bytes)),
                Message::Text(text) => break Ok(Some(Bytes::copy_from_slice(text.as_bytes()))),
                Message::Ping(_) => legacy.flush().await?,
                Message::Pong(_) => {}
                Message::Close(_) => {
                    transport::acknowledge_close(&mut legacy, WAIT).await;
                    break Ok(None);
                }
                _ => bail!("legacy client left before join"),
            }
        }
    })
    .await??;
    let Some(joined) = joined else {
        return Ok(());
    };
    if joined.len() > 16 {
        transport::initiate_close(
            &mut legacy,
            Some(CloseFrame {
                code: CloseCode::Protocol,
                reason: "Channel Length Exceeded".into(),
            }),
            WAIT,
            None,
        )
        .await;
        return Ok(());
    }
    // Disable Nagle's ACK-dependent delay for consecutive small relay packets.
    // Match the frontend's small initial buffer without reducing upstream frame limits.
    let upstream_config = WebSocketConfig::default().read_buffer_size(4096);
    let (mut upstream, _) = timeout(
        WAIT,
        connect_async_with_config(upstream, Some(upstream_config), true),
    )
    .await??;
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
                Some(Ok(Message::Close(frame))) => {
                    transport::acknowledge_close(&mut legacy, WAIT).await;
                    transport::initiate_close(&mut upstream, frame, WAIT, None).await;
                    break;
                }
                Some(Ok(message)) => match to_cbor(message) {
                    Ok(Some(packet)) => timeout(WAIT, upstream.send(Message::Binary(packet))).await??,
                    Ok(None) => {},
                    Err(reason) => { transport::initiate_close(&mut legacy, Some(CloseFrame { code: reason.code.into(), reason: reason.reason.into() }), WAIT, None).await; break; }
                }
                _ => break,
            },
            incoming = upstream.next() => match incoming {
                Some(Ok(Message::Binary(bytes))) => match from_cbor(&bytes) {
                    Ok(Some(message)) => timeout(WAIT, legacy.send(message)).await??,
                    Ok(None) => {},
                    Err(_) => { transport::initiate_close(&mut legacy, Some(CloseFrame { code: CloseCode::Unsupported, reason: "Value Requires CBOR Client".into() }), WAIT, None).await; break; }
                }
                Some(Ok(Message::Ping(_))) => { timeout(WAIT, upstream.flush()).await??; }
                Some(Ok(Message::Pong(_))) => {},
                Some(Ok(Message::Close(frame))) => {
                    transport::acknowledge_close(&mut upstream, WAIT).await;
                    transport::initiate_close(&mut legacy, frame, WAIT, None).await;
                    break;
                }
                _ => break,
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_envelopes_keep_canonical_headers_and_exact_owned_storage() {
        let targets: &[(u64, &[u8])] = &[
            (1, b"\x01"),
            (23, b"\x17"),
            (24, b"\x18\x18"),
            (255, b"\x18\xff"),
            (256, b"\x19\x01\x00"),
            (65535, b"\x19\xff\xff"),
            (65536, b"\x1a\x00\x01\x00\x00"),
            (u32::MAX as u64, b"\x1a\xff\xff\xff\xff"),
            (u32::MAX as u64 + 1, b"\x1b\x00\x00\x00\x01\x00\x00\x00\x00"),
            (u64::MAX, b"\x1b\xff\xff\xff\xff\xff\xff\xff\xff"),
        ];
        let lengths: &[(usize, &[u8])] = &[
            (1, b"\x41"),
            (23, b"\x57"),
            (24, b"\x58\x18"),
            (255, b"\x58\xff"),
            (256, b"\x59\x01\x00"),
            (65535, b"\x59\xff\xff"),
            (65536, b"\x5a\x00\x01\x00\x00"),
        ];
        for &(target, encoded_target) in targets {
            for &(len, encoded_length) in lengths {
                let payload: Vec<_> = (0..len).map(|i| i as u8).collect();
                let mut input = target.to_le_bytes().to_vec();
                input.extend_from_slice(&payload);
                let input = Bytes::from(input);
                let result = to_cbor(Message::Binary(input.clone())).unwrap().unwrap();
                let mut expected = vec![0x82];
                expected.extend_from_slice(encoded_target);
                expected.extend_from_slice(encoded_length);
                expected.extend_from_slice(&payload);
                assert_eq!(result.as_ref(), expected);
                assert_eq!(&input[8..], payload);
                let owned = result.try_into_mut().expect("independently owned envelope");
                assert_eq!(owned.capacity(), owned.len());
            }
        }
        assert_eq!(
            to_cbor(Message::Binary(Bytes::copy_from_slice(&1u64.to_le_bytes()))),
            Err(Disconnect::PROTOCOL),
        );
    }
}
