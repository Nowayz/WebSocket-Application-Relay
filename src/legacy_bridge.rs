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
        Error, Message,
        client::{IntoClientRequest, uri_mode},
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
                // CBOR lengths count UTF-8 bytes; encode directly into final storage.
                let payload = &text[12..];
                let capacity = 1
                    + protocol::uint_len(target)
                    + protocol::uint_len(payload.len() as u64)
                    + payload.len();
                let mut e = Encoder::new(Vec::with_capacity(capacity));
                e.array(2)
                    .and_then(|e| e.u64(target))
                    .and_then(|e| e.str(payload))
                    .expect("Vec encoding");
                Some(e.into_writer().into())
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
            let mut text = String::with_capacity(12 + payload.len());
            STANDARD.encode_string(sender.to_le_bytes(), &mut text);
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

/// Check upstream URL syntax and WebSocket scheme without opening a connection.
/// Reachability and TLS availability are checked when a client connects.
pub fn validate_upstream(upstream: &str) -> Result<()> {
    let request = upstream
        .into_client_request()
        .context("invalid legacy upstream URL")?;
    uri_mode(request.uri()).context("invalid legacy upstream URL")?;
    Ok(())
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
    validate_upstream(&upstream)?;
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

// Transport resets have no peer left to notify. Decoder failures get a bounded,
// best-effort Close; their input cannot safely resume a normal Close handshake.
fn frame_error_close(error: &Error) -> Option<CloseFrame> {
    let (code, reason) = match error {
        Error::Capacity(_) => (CloseCode::Size, "Message Too Large"),
        Error::Utf8(_) => (CloseCode::Invalid, "Invalid UTF-8"),
        Error::Protocol(
            tokio_tungstenite::tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
        ) => return None,
        Error::Protocol(_) => (CloseCode::Protocol, "Protocol Error"),
        _ => return None,
    };
    Some(CloseFrame {
        code,
        reason: reason.into(),
    })
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
                .context("legacy client left before join")?
            {
                Err(error) => {
                    if let Some(frame) = frame_error_close(&error) {
                        // Framing is no longer trustworthy: flush Close without
                        // interpreting the denied payload as an acknowledgement.
                        let _ = timeout(WAIT, legacy.close(Some(frame))).await;
                        break Ok(None);
                    }
                    return Err(error.into());
                }
                Ok(Message::Binary(bytes)) => break Ok::<_, anyhow::Error>(Some(bytes)),
                Ok(Message::Text(text)) => break Ok(Some(text.into())),
                Ok(Message::Ping(_)) => legacy.flush().await?,
                Ok(Message::Pong(_)) => {}
                Ok(Message::Close(_)) => {
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
    let subscription = protocol::byte_string(&joined);
    // The encoded subscription owns its bytes; release the frontend join view.
    drop(joined);
    upstream.send(Message::Binary(subscription)).await?;
    let welcome = timeout(WAIT, async {
        loop {
            match upstream
                .next()
                .await
                .context("upstream closed before welcome")??
            {
                Message::Ping(_) => upstream.flush().await?,
                Message::Pong(_) => {}
                Message::Close(frame) => {
                    transport::acknowledge_close(&mut upstream, WAIT).await;
                    transport::initiate_close(&mut legacy, frame, WAIT, None).await;
                    break Ok(None);
                }
                message => break Ok::<_, anyhow::Error>(Some(message)),
            }
        }
    })
    .await
    .context("upstream welcome timeout")??;
    let Some(welcome) = welcome else {
        return Ok(());
    };
    let id = match &welcome {
        Message::Binary(bytes) => {
            let mut decoder = Decoder::new(bytes);
            match decoder.u64() {
                Ok(id) if id != 0 && id != BROADCAST && decoder.position() == bytes.len() => {
                    Some(id)
                }
                _ => None,
            }
        }
        _ => None,
    };
    // Only the scalar ID survives validation, including during error cleanup.
    drop(welcome);
    let Some(id) = id else {
        let frame = Some(CloseFrame {
            code: CloseCode::Protocol,
            reason: "Invalid Upstream Welcome".into(),
        });
        tokio::join!(
            transport::initiate_close(&mut upstream, frame.clone(), WAIT, None),
            transport::initiate_close(&mut legacy, frame, WAIT, None),
        );
        return Ok(());
    };
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
                Some(Err(error)) => {
                    if let Some(frame) = frame_error_close(&error) {
                        let _ = timeout(WAIT, legacy.close(Some(frame))).await;
                    }
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
                Some(Ok(Message::Text(_))) => {
                    transport::initiate_close(&mut legacy, Some(CloseFrame {
                        code: CloseCode::Protocol,
                        reason: "Invalid Upstream Message".into(),
                    }), WAIT, None).await;
                    break;
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

    #[tokio::test]
    async fn upstream_rejection_before_welcome_reaches_legacy_and_releases_the_slot() {
        timeout(Duration::from_secs(2), async {
            let frames = [
                Some(CloseFrame {
                    code: CloseCode::Policy,
                    reason: "Join Denied".into(),
                }),
                None,
            ];
            let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream_url = format!("ws://{}", upstream.local_addr().unwrap());
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let shutdown = CancellationToken::new();
            let mut tasks = JoinSet::new();
            let requested = frames.clone();
            tasks.spawn(async move {
                for frame in requested {
                    let (stream, _) = upstream.accept().await.unwrap();
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    let Some(Ok(Message::Binary(join))) = ws.next().await else {
                        panic!("missing upstream join");
                    };
                    assert_eq!(protocol::join(&join).unwrap(), b"welcome");
                    assert!(
                        transport::initiate_close(&mut ws, frame, Duration::from_secs(1), None)
                            .await
                    );
                }
            });
            let token = shutdown.clone();
            tasks.spawn(async move {
                serve(listener, upstream_url, 1, token).await.unwrap();
            });
            for expected in frames {
                let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
                ws.send(Message::Binary(Bytes::from_static(b"welcome")))
                    .await
                    .unwrap();
                let Some(Ok(Message::Close(actual))) = ws.next().await else {
                    panic!("upstream rejection was not relayed before welcome");
                };
                match (expected, actual) {
                    (Some(expected), Some(actual)) => {
                        assert_eq!(actual.code, expected.code);
                        assert_eq!(actual.reason, expected.reason);
                    }
                    (None, None) => {}
                    _ => panic!("upstream close payload changed"),
                }
                assert!(transport::acknowledge_close(&mut ws, Duration::from_secs(1)).await);
                assert!(
                    ws.next().await.is_none(),
                    "bridge did not retire the rejected connection"
                );
            }
            shutdown.cancel();
            while let Some(result) = tasks.join_next().await {
                result.unwrap();
            }
        })
        .await
        .expect("early rejection and single-slot reuse must be prompt");
    }

    #[tokio::test]
    async fn upstream_ping_and_pong_before_welcome_preserve_join_and_routing() {
        timeout(Duration::from_secs(2), async {
            let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream_url = format!("ws://{}", upstream.local_addr().unwrap());
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let shutdown = CancellationToken::new();
            let mut tasks = JoinSet::new();
            tasks.spawn(async move {
                let (stream, _) = upstream.accept().await.unwrap();
                let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                assert!(matches!(ws.next().await, Some(Ok(Message::Binary(_)))));
                ws.send(Message::Pong(Bytes::from_static(b"unsolicited")))
                    .await
                    .unwrap();
                ws.send(Message::Ping(Bytes::from_static(b"probe")))
                    .await
                    .unwrap();
                assert_eq!(
                    ws.next().await.unwrap().unwrap(),
                    Message::Pong(Bytes::from_static(b"probe"))
                );
                ws.send(Message::Binary(protocol::uint(42))).await.unwrap();
                let expected = protocol::envelope(42, &protocol::byte_string(&[17, 34]));
                assert_eq!(
                    ws.next().await.unwrap().unwrap(),
                    Message::Binary(expected.clone())
                );
                ws.send(Message::Binary(expected)).await.unwrap();
                assert!(
                    transport::initiate_close(&mut ws, None, Duration::from_secs(1), None).await
                );
            });
            let token = shutdown.clone();
            tasks.spawn(async move {
                serve(listener, upstream_url, 1, token).await.unwrap();
            });
            let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
            ws.send(Message::Binary(Bytes::from_static(b"welcome")))
                .await
                .unwrap();
            assert_eq!(
                ws.next().await.unwrap().unwrap(),
                Message::Binary(Bytes::copy_from_slice(&42u64.to_le_bytes()))
            );
            let mut packet = 42u64.to_le_bytes().to_vec();
            packet.extend_from_slice(&[17, 34]);
            ws.send(Message::Binary(Bytes::from(packet.clone())))
                .await
                .unwrap();
            assert_eq!(
                ws.next().await.unwrap().unwrap(),
                Message::Binary(Bytes::from(packet))
            );
            assert!(matches!(ws.next().await, Some(Ok(Message::Close(None)))));
            assert!(transport::acknowledge_close(&mut ws, Duration::from_secs(1)).await);
            assert!(ws.next().await.is_none());
            shutdown.cancel();
            while let Some(result) = tasks.join_next().await {
                result.unwrap();
            }
        })
        .await
        .expect("pre-welcome control frames must not prevent routing");
    }

    #[tokio::test]
    async fn upstream_text_after_welcome_closes_cleanly_and_releases_the_slot() {
        timeout(Duration::from_secs(2), async {
            let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream_url = format!("ws://{}", upstream.local_addr().unwrap());
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let shutdown = CancellationToken::new();
            let mut tasks = JoinSet::new();
            tasks.spawn(async move {
                for _ in 0..2 {
                    let (stream, _) = upstream.accept().await.unwrap();
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    assert!(matches!(ws.next().await, Some(Ok(Message::Binary(_)))));
                    ws.send(Message::Binary(protocol::uint(42))).await.unwrap();
                    ws.send(Message::Text("unexpected".into())).await.unwrap();
                    // Keep the upstream open until the bridge retires it.
                    while let Some(message) = ws.next().await {
                        if message.is_err() {
                            break;
                        }
                    }
                }
            });
            let token = shutdown.clone();
            tasks.spawn(async move {
                serve(listener, upstream_url, 1, token).await.unwrap();
            });
            for _ in 0..2 {
                let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
                ws.send(Message::Binary(Bytes::from_static(b"fault")))
                    .await
                    .unwrap();
                assert_eq!(
                    ws.next().await.unwrap().unwrap(),
                    Message::Binary(Bytes::copy_from_slice(&42u64.to_le_bytes()))
                );
                let Some(Ok(Message::Close(Some(frame)))) = ws.next().await else {
                    panic!("unexpected upstream text caused an unclean legacy disconnect");
                };
                assert_eq!(frame.code, CloseCode::Protocol);
                assert_eq!(frame.reason, "Invalid Upstream Message");
                assert!(transport::acknowledge_close(&mut ws, Duration::from_secs(1)).await);
                assert!(ws.next().await.is_none());
            }
            shutdown.cancel();
            while let Some(result) = tasks.join_next().await {
                result.unwrap();
            }
        })
        .await
        .expect("upstream text rejection and single-slot reuse must be prompt");
    }

    async fn welcome_validation_cases(cases: Vec<(Message, Option<u64>)>) {
        timeout(Duration::from_secs(2), async {
            let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let upstream_url = format!("ws://{}", upstream.local_addr().unwrap());
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("ws://{}", listener.local_addr().unwrap());
            let shutdown = CancellationToken::new();
            let mut tasks = JoinSet::new();
            let expected_cases = cases.clone();
            tasks.spawn(async move {
                for (message, id) in expected_cases {
                    let (stream, _) = upstream.accept().await.unwrap();
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    assert!(matches!(ws.next().await, Some(Ok(Message::Binary(_)))));
                    ws.send(message).await.unwrap();
                    let Some(Ok(Message::Close(Some(frame)))) = ws.next().await else {
                        panic!("upstream did not receive clean Close");
                    };
                    assert_eq!(
                        frame.code,
                        if id.is_some() {
                            CloseCode::Normal
                        } else {
                            CloseCode::Protocol
                        }
                    );
                    assert_eq!(
                        frame.reason,
                        if id.is_some() {
                            "done"
                        } else {
                            "Invalid Upstream Welcome"
                        }
                    );
                    assert!(transport::acknowledge_close(&mut ws, Duration::from_secs(1)).await);
                    assert!(ws.next().await.is_none());
                }
            });
            let token = shutdown.clone();
            tasks.spawn(async move {
                serve(listener, upstream_url, 1, token).await.unwrap();
            });
            for (_, id) in cases {
                let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
                ws.send(Message::Binary(Bytes::from_static(b"validation")))
                    .await
                    .unwrap();
                if let Some(id) = id {
                    assert_eq!(
                        ws.next().await.unwrap().unwrap(),
                        Message::Binary(Bytes::copy_from_slice(&id.to_le_bytes()))
                    );
                    assert!(
                        transport::initiate_close(
                            &mut ws,
                            Some(CloseFrame {
                                code: CloseCode::Normal,
                                reason: "done".into()
                            }),
                            Duration::from_secs(1),
                            None
                        )
                        .await
                    );
                    assert!(ws.next().await.is_none());
                } else {
                    let received = ws.next().await;
                    let Some(Ok(Message::Close(Some(frame)))) = received else {
                        panic!("invalid welcome was exposed as a legacy ID: {received:?}");
                    };
                    assert_eq!(frame.code, CloseCode::Protocol);
                    assert_eq!(frame.reason, "Invalid Upstream Welcome");
                    assert!(transport::acknowledge_close(&mut ws, Duration::from_secs(1)).await);
                    assert!(ws.next().await.is_none());
                }
            }
            shutdown.cancel();
            while let Some(result) = tasks.join_next().await {
                result.unwrap();
            }
        })
        .await
        .expect("welcome validation and one-slot reuse must be prompt");
    }

    #[tokio::test]
    async fn invalid_upstream_welcome_ids_and_trailing_values_close_both_peers() {
        let cases = vec![
            protocol::uint(0),
            Bytes::new(),
            protocol::uint(u64::MAX),
            Bytes::from_static(&[0x18]),
            Bytes::from_static(&[0x20]),
            Bytes::from_static(&[0x61, b'x']),
            Bytes::from_static(&[0xf6]),
            Bytes::from_static(&[0x82, 1, 2]),
            Bytes::from_static(&[0x18, 42, 0]),
        ];
        let mut cases: Vec<_> = cases
            .into_iter()
            .map(|bytes| (Message::Binary(bytes), None))
            .collect();
        cases.push((Message::Text("42".into()), None));
        welcome_validation_cases(cases).await;
    }

    #[tokio::test]
    async fn complete_uint64_welcomes_keep_all_widths_and_noncanonical_encoding() {
        let mut cases: Vec<_> = [
            1,
            23,
            24,
            255,
            256,
            65535,
            65536,
            u32::MAX as u64,
            u64::MAX - 1,
        ]
        .into_iter()
        .map(|id| (Message::Binary(protocol::uint(id)), Some(id)))
        .collect();
        cases.push((
            Message::Binary(Bytes::from_static(&[0x1b, 0, 0, 0, 0, 0, 0, 0, 42])),
            Some(42),
        ));
        welcome_validation_cases(cases).await;
    }

    #[tokio::test]
    async fn invalid_frames_send_policy_close_before_and_after_join() {
        use tokio::io::AsyncWriteExt;
        timeout(Duration::from_secs(2), async {
            for (joined, kind) in [false, true]
                .into_iter()
                .flat_map(|joined| (0..5).map(move |kind| (joined, kind)))
            {
                let upstream = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let upstream_url = format!("ws://{}", upstream.local_addr().unwrap());
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let url = format!("ws://{}", listener.local_addr().unwrap());
                let shutdown = CancellationToken::new();
                let mut tasks = JoinSet::new();
                tasks.spawn(async move {
                    let (stream, _) = upstream.accept().await.unwrap();
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    assert!(matches!(ws.next().await, Some(Ok(Message::Binary(_)))));
                    ws.send(Message::Binary(protocol::uint(1))).await.unwrap();
                    while ws.next().await.is_some_and(|message| message.is_ok()) {}
                });
                let token = shutdown.clone();
                tasks.spawn(async move {
                    serve(listener, upstream_url, 1, token).await.unwrap();
                });
                let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
                if joined {
                    ws.send(Message::Binary(Bytes::from_static(b"size-check")))
                        .await
                        .unwrap();
                    assert!(matches!(ws.next().await, Some(Ok(Message::Binary(_)))));
                }
                // No large payload allocation: rejection follows the advertised length.
                let mut header = [0; 14];
                header[..2].copy_from_slice(&[0x82, 0xff]);
                header[2..10].copy_from_slice(&(16 * 1024 * 1024 + 1u64).to_be_bytes());
                let (wire, code, reason) = match kind {
                    0 => (&header[..], CloseCode::Size, "Message Too Large"),
                    1 => (&[0x82, 1, 42][..], CloseCode::Protocol, "Protocol Error"),
                    2 => (
                        &[0x83, 0x80, 0, 0, 0, 0][..],
                        CloseCode::Protocol,
                        "Protocol Error",
                    ),
                    3 => (
                        &[0x09, 0x80, 0, 0, 0, 0][..],
                        CloseCode::Protocol,
                        "Protocol Error",
                    ),
                    _ => (
                        &[0x81, 0x81, 0, 0, 0, 0, 0xff][..],
                        CloseCode::Invalid,
                        "Invalid UTF-8",
                    ),
                };
                ws.get_mut().write_all(wire).await.unwrap();
                let Some(Ok(Message::Close(Some(frame)))) = ws.next().await else {
                    panic!("invalid frame did not receive Close: joined={joined}, kind={kind}");
                };
                assert_eq!(frame.code, code);
                assert_eq!(frame.reason, reason);
                shutdown.cancel();
                tasks.shutdown().await;
            }
        })
        .await
        .expect("invalid-frame rejection must be prompt");
    }

    #[tokio::test]
    async fn invalid_upstream_is_rejected_before_the_accept_loop() {
        for upstream in ["not a URL", "http://localhost/", "ws://", "/relative"] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let shutdown = CancellationToken::new();
            shutdown.cancel();
            let result = serve_with_limits(
                listener,
                upstream.into(),
                1,
                0,
                crate::origin::OriginPolicy::default(),
                shutdown,
            )
            .await;
            assert!(result.is_err(), "invalid upstream accepted: {upstream}");
        }
    }

    #[test]
    fn upstream_validation_accepts_websocket_urls_without_connecting() {
        for upstream in [
            "ws://127.0.0.1:1338",
            "ws://[::1]:1338/path?mode=cbor",
            "wss://relay.example.invalid:443/path",
        ] {
            validate_upstream(upstream).unwrap();
        }
    }

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

    #[test]
    fn text_envelopes_preserve_utf8_byte_lengths_and_exact_owned_storage() {
        let targets: &[(u64, &[u8])] = &[
            (1, b"\x01"),
            (256, b"\x19\x01\x00"),
            (u64::MAX, b"\x1b\xff\xff\xff\xff\xff\xff\xff\xff"),
        ];
        let lengths: &[(usize, &[u8])] = &[
            (1, b"\x61"),
            (23, b"\x77"),
            (24, b"\x78\x18"),
            (255, b"\x78\xff"),
            (256, b"\x79\x01\x00"),
            (65535, b"\x79\xff\xff"),
            (65536, b"\x7a\x00\x01\x00\x00"),
        ];
        for &(target, encoded_target) in targets {
            for &(len, encoded_length) in lengths {
                let payload = "λ".repeat(len / 2) + if len % 2 == 1 { "x" } else { "" };
                assert_eq!(payload.len(), len);
                let input: tokio_tungstenite::tungstenite::Utf8Bytes =
                    (STANDARD.encode(target.to_le_bytes()) + &payload).into();
                let result = to_cbor(Message::Text(input.clone())).unwrap().unwrap();
                let mut expected = vec![0x82];
                expected.extend_from_slice(encoded_target);
                expected.extend_from_slice(encoded_length);
                expected.extend_from_slice(payload.as_bytes());
                assert_eq!(result.as_ref(), expected);
                assert_eq!(&input[12..], payload);
                let owned = result.try_into_mut().expect("independently owned envelope");
                assert_eq!(owned.capacity(), owned.len());
            }
        }
        let input = STANDARD.encode(1u64.to_le_bytes()) + "\0λ🦀e\u{301}";
        assert_eq!(
            to_cbor(Message::Text(input.into()))
                .unwrap()
                .unwrap()
                .as_ref(),
            b"\x82\x01\x6a\x00\xce\xbb\xf0\x9f\xa6\x80e\xcc\x81",
        );
        let empty = STANDARD.encode(1u64.to_le_bytes());
        assert_eq!(
            to_cbor(Message::Text(empty.into())),
            Err(Disconnect::PROTOCOL)
        );
        assert_eq!(
            to_cbor(Message::Text("!!!!!!!!!!!!x".into())),
            Err(Disconnect::PROTOCOL)
        );
        let ignored = STANDARD.encode(0u64.to_le_bytes()) + "λ";
        assert_eq!(to_cbor(Message::Text(ignored.into())).unwrap(), None);
    }
}
