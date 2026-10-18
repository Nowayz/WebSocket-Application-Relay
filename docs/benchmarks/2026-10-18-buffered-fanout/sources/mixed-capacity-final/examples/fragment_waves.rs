//! Repeated fragmented self routes, control interleaving and healthy traffic.
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering::Relaxed},
    },
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use bytes::Bytes;
use clap::Parser;
use futures_util::{SinkExt, StreamExt, future::try_join_all};
use minicbor::Decoder;
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
    #[arg(long, default_value_t = 8)]
    clients: usize,
    #[arg(long, default_value_t = 3)]
    cycles: usize,
    #[arg(long, default_value_t = 4 * 1024 * 1024)]
    message_bytes: usize,
    #[arg(long)]
    split_bytes: Option<usize>,
    #[arg(long, default_value_t = 250)]
    hold_ms: u64,
    #[arg(long, default_value_t = 2)]
    cooldown_secs: u64,
    #[arg(long)]
    expect_completed: Option<usize>,
    #[arg(long)]
    expect_rejected: Option<usize>,
}

struct Source {
    socket: Socket,
    id: u64,
    packet: Vec<u8>,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        (1..=64).contains(&args.clients) && (1..=8).contains(&args.cycles),
        "use 1..64 clients and 1..8 cycles"
    );
    ensure!(
        (65536..=8 * 1024 * 1024).contains(&args.message_bytes),
        "message bytes outside 64 KiB..8 MiB"
    );
    ensure!(
        (50..=2000).contains(&args.hold_ms) && args.cooldown_secs <= 10,
        "hold/cooldown outside bounds"
    );
    let split = args.split_bytes.unwrap_or(args.message_bytes * 3 / 4);
    ensure!(
        split > 0 && split < args.message_bytes,
        "split must leave two nonempty fragments"
    );
    ensure!(
        args.expect_completed
            .is_none_or(|n| n <= args.clients * args.cycles)
            && args.expect_rejected.is_none_or(|n| n <= args.clients),
        "expectations outside bounds"
    );
    let mut sources = Vec::with_capacity(args.clients);
    for index in 0..args.clients {
        let (socket, id) = join(args.address, &format!("fragwave-{index:x}")).await?;
        let mut packet = Vec::with_capacity(args.message_bytes);
        // A definite byte string, with an explicit uint64 target. The input
        // remains exactly the configured limit even for a short assigned ID.
        packet.extend_from_slice(&[0x82, 0x1b]);
        packet.extend_from_slice(&id.to_be_bytes());
        packet.push(0x5a);
        packet.extend_from_slice(&((args.message_bytes - 15) as u32).to_be_bytes());
        packet.resize(args.message_bytes, 0x5a);
        sources.push(Source { socket, id, packet });
    }
    let (mut healthy, healthy_id) = join(args.address, "frag-healthy").await?;
    let stop = Arc::new(AtomicBool::new(false));
    let stopped = stop.clone();
    let healthy_task = tokio::spawn(async move {
        let mut probes = 0u64;
        let mut max_latency = Duration::ZERO;
        let mut sent_bytes = 0u64;
        while !stopped.load(Relaxed) {
            let mut value = [0x39; 1024];
            value[..8].copy_from_slice(&probes.to_le_bytes());
            let packet = protocol::envelope(healthy_id, &protocol::byte_string(&value));
            let started = Instant::now();
            healthy.send(Message::Binary(packet.clone())).await?;
            ensure!(
                binary(&mut healthy).await? == packet,
                "healthy self echo changed at {probes}"
            );
            max_latency = max_latency.max(started.elapsed());
            sent_bytes += packet.len() as u64;
            probes += 1;
            sleep(Duration::from_millis(20)).await;
        }
        close(&mut healthy).await?;
        Ok::<_, anyhow::Error>((probes, max_latency, sent_bytes))
    });
    event("ready", None, serde_json::json!({"clients":args.clients}));
    let mut completed = 0usize;
    let mut rejected = Vec::new();
    let mut post_release_bytes = 0u64;
    for wave in 0..args.cycles {
        event(
            "starting",
            Some(wave),
            serde_json::json!({"active":sources.len()}),
        );
        try_join_all(sources.iter_mut().map(|source| async move {
            let tail = source.packet.len() - 8;
            source.packet[tail..].copy_from_slice(&(wave as u64).to_le_bytes());
            write_frame(source.socket.get_mut(), 0x02, &source.packet[..split]).await?;
            let ping = Bytes::copy_from_slice(&(wave as u64).to_le_bytes());
            source.socket.send(Message::Ping(ping.clone())).await?;
            let reply = timeout(Duration::from_secs(5), source.socket.next())
                .await?
                .context("first-fragment EOF")??;
            ensure!(
                reply == Message::Pong(ping),
                "first fragment was not assembled before hold: {reply:?}"
            );
            Ok::<_, anyhow::Error>(())
        }))
        .await?;
        event(
            "assembling",
            Some(wave),
            serde_json::json!({"active":sources.len(),"first_bytes":split}),
        );
        sleep(Duration::from_millis(args.hold_ms)).await;
        let decisions = try_join_all(sources.into_iter().map(|mut source| async move {
            write_frame(source.socket.get_mut(), 0x80, &source.packet[split..]).await?;
            let reply = timeout(Duration::from_secs(5), source.socket.next())
                .await?
                .context("final-fragment EOF")??;
            match reply {
                Message::Binary(bytes) => {
                    let mut d = Decoder::new(&bytes);
                    ensure!(
                        d.array()? == Some(2) && d.u64()? == source.id,
                        "fragmented sender/envelope changed"
                    );
                    ensure!(
                        bytes[d.position()..] == source.packet[10..],
                        "fragmented value changed in wave {wave}"
                    );
                    Ok((Some(source), None))
                }
                Message::Close(Some(frame)) => {
                    let code = u16::from(frame.code);
                    ensure!(
                        code == 1013 && frame.reason == "Input Memory Limit",
                        "unexpected refusal: {frame:?}"
                    );
                    // The server has failed its decoder. Verify the policy Close
                    // but do not claim an acknowledgement on this partial path.
                    Ok((None, Some(code)))
                }
                other => bail!("unexpected fragmented result: {other:?}"),
            }
        }))
        .await?;
        sources = Vec::with_capacity(args.clients);
        for (source, code) in decisions {
            if let Some(source) = source {
                sources.push(source);
                completed += 1;
            }
            if let Some(code) = code {
                rejected.push(code);
            }
        }
        event(
            "cooldown",
            Some(wave),
            serde_json::json!({"active":sources.len(),"completed":completed,"rejected":rejected.len()}),
        );
        sleep(Duration::from_secs(args.cooldown_secs)).await;
        // A fresh connection must remain possible after the wave settles.
        let (mut after, id) = join(args.address, "frag-after").await?;
        let packet = protocol::envelope(id, &protocol::byte_string(&(wave as u64).to_le_bytes()));
        after.send(Message::Binary(packet.clone())).await?;
        ensure!(
            binary(&mut after).await? == packet,
            "post-wave routing changed"
        );
        post_release_bytes += packet.len() as u64;
        close(&mut after).await?;
        event(
            "settled",
            Some(wave),
            serde_json::json!({"active":sources.len(),"post_release_join_and_route":true}),
        );
        // Give the observer a stable post-wave interval, separate from the
        // following burst and from connections retiring after their Close.
        sleep(Duration::from_millis(150)).await;
    }
    if let Some(expected) = args.expect_completed {
        ensure!(
            completed == expected,
            "completed {completed}, expected {expected}"
        );
    }
    if let Some(expected) = args.expect_rejected {
        ensure!(
            rejected.len() == expected,
            "refused {}, expected {expected}",
            rejected.len()
        );
    }
    event("closing", None, serde_json::json!({"active":sources.len()}));
    let normal_sources = sources.len();
    try_join_all(sources.iter_mut().map(|source| close(&mut source.socket))).await?;
    stop.store(true, Relaxed);
    let (probes, latency, healthy_bytes) = healthy_task.await??;
    event("retired", None, serde_json::json!({}));
    sleep(Duration::from_secs(1)).await;
    event(
        "result",
        None,
        serde_json::json!({"clients":args.clients,"cycles":args.cycles,"message_bytes":args.message_bytes,"split_bytes":split,
        "completed":completed,"rejected_codes":rejected,"healthy_verified":probes,"healthy_latency_max_ms":latency.as_secs_f64()*1000.0,
        "expected_incoming_messages":completed as u64+probes+args.cycles as u64,
        "expected_incoming_bytes":completed as u64*args.message_bytes as u64+healthy_bytes+post_release_bytes,
        "expected_joins":args.clients+1+args.cycles,"clean_close_handshakes":normal_sources+1+args.cycles,
        "expected_incomplete_tcp_drops":0,"post_release_join_and_route":true}),
    );
    Ok(())
}

fn event(name: &str, wave: Option<usize>, value: serde_json::Value) {
    let mut value = value.as_object().unwrap().clone();
    value.insert("event".into(), name.into());
    if let Some(wave) = wave {
        value.insert("wave".into(), wave.into());
    }
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
    let id = Decoder::new(&binary(&mut socket).await?).u64()?;
    Ok((socket, id))
}

async fn binary(socket: &mut Socket) -> Result<Bytes> {
    timeout(Duration::from_secs(5), async {
        match socket.next().await.context("data EOF")?? {
            Message::Binary(bytes) => Ok(bytes),
            other => bail!("unexpected data: {other:?}"),
        }
    })
    .await?
}

async fn close(socket: &mut Socket) -> Result<()> {
    timeout(Duration::from_secs(5), async {
        socket.close(None).await?;
        match socket.next().await.context("Close EOF")?? {
            Message::Close(None) => {
                socket.flush().await?;
                Ok(())
            }
            other => bail!("unexpected Close: {other:?}"),
        }
    })
    .await?
}

async fn write_frame(stream: &mut TcpStream, opcode: u8, body: &[u8]) -> Result<()> {
    let mut header = [0u8; 14];
    header[0] = opcode;
    let size = match body.len() {
        0..=125 => {
            header[1] = 0x80 | body.len() as u8;
            6
        }
        126..=65535 => {
            header[1] = 0xfe;
            header[2..4].copy_from_slice(&(body.len() as u16).to_be_bytes());
            8
        }
        _ => {
            header[1] = 0xff;
            header[2..10].copy_from_slice(&(body.len() as u64).to_be_bytes());
            14
        }
    };
    // A fixed zero mask keeps this testing client from copying its prepared body.
    stream.write_all(&header[..size]).await?;
    stream.write_all(body).await?;
    Ok(())
}
