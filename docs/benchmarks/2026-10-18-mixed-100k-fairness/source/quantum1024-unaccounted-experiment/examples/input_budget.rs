//! Hold incomplete large frames while checking an independent healthy session.
use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use clap::Parser;
use futures_util::{SinkExt, StreamExt, future::try_join_all};
use tokio::{
    io::AsyncWriteExt,
    net::TcpStream,
    time::{Instant, sleep, timeout},
};
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use war_relay::protocol;

type Socket = WebSocketStream<TcpStream>;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    address: SocketAddr,
    #[arg(long, default_value_t = 64)]
    clients: usize,
    #[arg(long, default_value_t = 8 * 1024 * 1024)]
    frame_bytes: usize,
    #[arg(long, default_value_t = 2 * 1024 * 1024)]
    partial_bytes: usize,
    #[arg(long, default_value_t = 2)]
    hold_secs: u64,
    #[arg(long)]
    expect_admitted: Option<usize>,
    #[arg(long)]
    expect_admission_denial: bool,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        (1..=64).contains(&args.clients),
        "clients must be in 1..=64"
    );
    ensure!(
        (65536..=8 * 1024 * 1024).contains(&args.frame_bytes),
        "frame size outside 64 KiB..=8 MiB"
    );
    ensure!(
        (1024..=2 * 1024 * 1024).contains(&args.partial_bytes)
            && args.partial_bytes < args.frame_bytes,
        "partial size outside bounds"
    );
    ensure!(
        (1..=10).contains(&args.hold_secs),
        "hold must be in 1..=10 seconds"
    );
    let mut sources = Vec::with_capacity(args.clients);
    let mut ids = Vec::with_capacity(args.clients);
    for index in 0..args.clients {
        let (socket, id) = join(args.address, &format!("inflight-{index:x}")).await?;
        sources.push(socket);
        ids.push(id);
    }
    let (mut healthy, healthy_id) = join(args.address, "budget-healthy").await?;
    let unknown = (1..)
        .find(|id| !ids.contains(id) && *id != healthy_id)
        .unwrap();
    // A one-byte target and an explicit five-byte byte-string header make seven
    // CBOR prefix bytes, including at nonpreferred length boundaries. The full
    // packet would be valid if completed.
    ensure!(unknown < 24, "unexpected occupied low-ID range");
    let mut packet = Vec::with_capacity(args.frame_bytes);
    packet.extend_from_slice(&[0x82, unknown as u8, 0x5a]);
    packet.extend_from_slice(&((args.frame_bytes - 7) as u32).to_be_bytes());
    packet.resize(args.frame_bytes, 0x5a);
    ensure!(
        packet.len() == args.frame_bytes,
        "declared frame sizing changed"
    );
    let mut header = vec![0x82, 0xff];
    header.extend_from_slice(&(args.frame_bytes as u64).to_be_bytes());
    header.extend_from_slice(&[0; 4]); // Fixed test mask; payload bytes need no transformation.
    event(
        "ready",
        serde_json::json!({"clients":args.clients,"healthy_id":healthy_id}),
    );
    for socket in &mut sources {
        socket.get_mut().write_all(&header).await?;
    }
    let decisions = try_join_all(sources.into_iter().map(|mut socket| async move {
        match timeout(Duration::from_millis(500), socket.next()).await {
            Err(_) => Ok((Some(socket), None)),
            Ok(Some(Ok(Message::Close(Some(frame))))) => Ok((None, Some(u16::from(frame.code)))),
            other => bail!("unexpected header admission response: {other:?}"),
        }
    }))
    .await?;
    let mut admitted = Vec::new();
    let mut rejected_codes = Vec::new();
    for (socket, code) in decisions {
        if let Some(socket) = socket {
            admitted.push(socket);
        }
        if let Some(code) = code {
            rejected_codes.push(code);
        }
    }
    if let Some(expected) = args.expect_admitted {
        ensure!(
            admitted.len() == expected,
            "admitted {} instead of {expected}",
            admitted.len()
        );
    }
    ensure!(
        rejected_codes.iter().all(|code| *code == 1013),
        "unexpected rejection codes: {rejected_codes:?}"
    );
    event(
        "headers",
        serde_json::json!({"admitted":admitted.len(),"rejected_codes":rejected_codes}),
    );
    try_join_all(
        admitted
            .iter_mut()
            .map(|socket| socket.get_mut().write_all(&packet[..args.partial_bytes])),
    )
    .await?;
    if args.expect_admission_denial {
        let result = client_async_with_config(
            format!("ws://{}", args.address),
            TcpStream::connect(args.address).await?,
            None,
        )
        .await;
        ensure!(
            matches!(result, Err(tokio_tungstenite::tungstenite::Error::Http(ref response)) if response.status()==503),
            "new connection was not rejected with HTTP 503"
        );
    }
    event(
        "holding",
        serde_json::json!({"admitted":admitted.len(),"partial_bytes":args.partial_bytes}),
    );
    let until = Instant::now() + Duration::from_secs(args.hold_secs);
    let mut probes = 0u64;
    let mut max_latency = Duration::ZERO;
    while Instant::now() < until {
        let mut payload = vec![0x39; 1024];
        payload[..8].copy_from_slice(&probes.to_le_bytes());
        let packet = protocol::envelope(healthy_id, &protocol::byte_string(&payload));
        let start = Instant::now();
        healthy.send(Message::Binary(packet.clone())).await?;
        ensure!(
            data(&mut healthy).await? == packet,
            "healthy echo changed at {probes}"
        );
        max_latency = max_latency.max(start.elapsed());
        probes += 1;
        sleep(Duration::from_millis(20)).await;
    }
    let count = admitted.len();
    event("dropping", serde_json::json!({"incomplete_sockets":count}));
    // These frames intentionally never finish. TCP drop tests decoder release;
    // it is not a graceful WebSocket departure.
    drop(admitted);
    // A new session must be possible after the incomplete owners retire.
    let (mut after, after_id) = timeout(Duration::from_secs(3), async {
        loop {
            match join(args.address, "budget-after").await {
                Ok(value) => break Ok::<_, anyhow::Error>(value),
                Err(error) if matches!(error.downcast_ref::<tokio_tungstenite::tungstenite::Error>(),
                    Some(tokio_tungstenite::tungstenite::Error::Http(response)) if response.status()==503) => {
                    sleep(Duration::from_millis(20)).await;
                }
                Err(error) => break Err(error),
            }
        }
    }).await??;
    let check = protocol::envelope(after_id, &[0]);
    after.send(Message::Binary(check.clone())).await?;
    ensure!(
        data(&mut after).await? == check,
        "post-release routing changed"
    );
    close(&mut after).await?;
    close(&mut healthy).await?;
    event("retired", serde_json::json!({}));
    sleep(Duration::from_secs(1)).await;
    event(
        "result",
        serde_json::json!({"clients":args.clients,"admitted":count,"rejected_codes":rejected_codes,
        "healthy_verified":probes,"healthy_latency_max_ms":max_latency.as_secs_f64()*1000.0,
        "expected_incomplete_tcp_drops":count,"clean_close_handshakes":2,"post_release_join_and_route":true,
        "new_connection_denial_verified":args.expect_admission_denial,"frame_bytes":args.frame_bytes,"partial_bytes":args.partial_bytes}),
    );
    Ok(())
}

fn event(name: &str, value: serde_json::Value) {
    let mut value = value.as_object().unwrap().clone();
    value.insert("event".into(), name.into());
    println!("{}", serde_json::Value::Object(value));
}

async fn join(address: SocketAddr, channel: &str) -> Result<(Socket, u64)> {
    let stream = TcpStream::connect(address).await?;
    stream.set_nodelay(true)?;
    let (mut socket, _) = client_async_with_config(
        format!("ws://{address}"),
        stream,
        Some(WebSocketConfig::default().read_buffer_size(4096)),
    )
    .await?;
    socket
        .send(Message::Binary(protocol::text(channel)))
        .await?;
    let id = minicbor::Decoder::new(&data(&mut socket).await?).u64()?;
    Ok((socket, id))
}

async fn data(socket: &mut Socket) -> Result<Bytes> {
    timeout(Duration::from_secs(3), async {
        loop {
            match socket.next().await.context("unexpected EOF")?? {
                Message::Binary(bytes) => return Ok(bytes),
                Message::Ping(_) => socket.flush().await?,
                other => bail!("unexpected data: {other:?}"),
            }
        }
    })
    .await?
}

async fn close(socket: &mut Socket) -> Result<()> {
    timeout(Duration::from_secs(3), async {
        socket.close(None).await?;
        while let Some(message) = socket.next().await {
            match message? {
                Message::Close(None) => {
                    socket.flush().await?;
                    return Ok(());
                }
                Message::Ping(_) => socket.flush().await?,
                other => bail!("unexpected close response: {other:?}"),
            }
        }
        bail!("missing Close acknowledgement")
    })
    .await?
}
