//! Verify global routing before/after a transient population in separate rooms.
//! Every reported event waits for the harness to sample and send `continue`.
//! This blocking control gate is outside timed traffic, on the CLI main thread.
use std::{
    io::{self, Write},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use tokio::{net::TcpStream, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        Message,
        protocol::{CloseFrame, WebSocketConfig, frame::coding::CloseCode},
    },
};
use war_relay::protocol;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "ws://127.0.0.1:1338")]
    url: String,
    #[arg(long, default_value_t = 8192)]
    peak_clients: usize,
    #[arg(long, default_value_t = 3)]
    seconds: u64,
    #[arg(long, default_value_t = 1)]
    waves: usize,
    #[arg(long, default_value_t = 1024)]
    payload_bytes: usize,
}

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        args.peak_clients <= 9000,
        "peak-clients must be in 0..=9000"
    );
    ensure!(
        (1..=60).contains(&args.seconds),
        "seconds must be in 1..=60"
    );
    ensure!((1..=8).contains(&args.waves), "waves must be in 1..=8");
    ensure!(
        (8..=65536).contains(&args.payload_bytes),
        "payload-bytes must be in 8..=65536"
    );
    let (mut sender, id, first_welcome) = join(&args.url, "re_globl").await?;
    let (mut receiver, _, second_welcome) = join(&args.url, "re_globl").await?;
    let mut welcome_bytes = first_welcome + second_welcome;
    let mut payload = vec![0x5a; args.payload_bytes];
    let packets: Vec<_> = (0u64..64)
        .map(|i| {
            payload[..8].copy_from_slice(&i.to_le_bytes());
            let value = protocol::byte_string(&payload);
            (
                protocol::envelope(protocol::BROADCAST, &value),
                protocol::envelope(id, &value),
            )
        })
        .collect();
    gate("traffic_ready", 0, serde_json::json!({"sessions":2}))?;
    gate(
        "traffic_done",
        0,
        traffic(&mut sender, &mut receiver, &packets, args.seconds).await?,
    )?;
    let mut fillers = Vec::with_capacity(args.peak_clients);
    for wave in 1..=args.waves {
        for i in 0..args.peak_clients {
            let (socket, _, bytes) = join(&args.url, &format!("peak{i:x}")).await?;
            welcome_bytes += bytes;
            fillers.push(socket);
        }
        gate(
            "population_ready",
            wave,
            serde_json::json!({"sessions":args.peak_clients+2,"channels":args.peak_clients+1}),
        )?;
        for mut socket in fillers.drain(..) {
            close(&mut socket).await?;
        }
        gate("traffic_ready", wave, serde_json::json!({"sessions":2}))?;
        gate(
            "traffic_done",
            wave,
            traffic(&mut sender, &mut receiver, &packets, args.seconds).await?,
        )?;
    }
    close(&mut sender).await?;
    let disconnect = protocol::envelope(protocol::BROADCAST, &protocol::uint(id));
    ensure!(
        data(&mut receiver).await? == disconnect,
        "final disconnect event changed"
    );
    close(&mut receiver).await?;
    gate(
        "done",
        args.waves,
        serde_json::json!({"joins":args.peak_clients*args.waves+2,"normal_closes":args.peak_clients*args.waves+2,"welcome_bytes":welcome_bytes,"disconnect_bytes":disconnect.len(),"disconnects":1}),
    )?;
    Ok(())
}

fn gate(event: &str, wave: usize, details: serde_json::Value) -> Result<()> {
    println!(
        "{}",
        serde_json::json!({"event":event,"wave":wave,"details":details})
    );
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    ensure!(line == "continue\n", "expected harness continuation");
    Ok(())
}

async fn traffic(
    sender: &mut Socket,
    receiver: &mut Socket,
    packets: &[(Bytes, Bytes)],
    seconds: u64,
) -> Result<serde_json::Value> {
    let start = Instant::now();
    let until = start + Duration::from_secs(seconds);
    let mut count = 0u64;
    let mut latency = [0u64; 64];
    let mut maximum = 0u64;
    let mut stamps = [Instant::now(); 16];
    while Instant::now() < until {
        for stamp in &mut stamps {
            *stamp = Instant::now();
            sender
                .send(Message::Binary(
                    packets[count as usize % packets.len()].0.clone(),
                ))
                .await?;
            count += 1;
        }
        for (i, stamp) in stamps.iter().enumerate() {
            let expected = &packets[(count as usize - stamps.len() + i) % packets.len()].1;
            ensure!(
                data(receiver).await? == *expected,
                "payload, sender or FIFO changed"
            );
            let ns = stamp.elapsed().as_nanos().min(u128::from(u64::MAX)) as u64;
            latency[ns.checked_ilog2().unwrap_or(0) as usize] += 1;
            maximum = maximum.max(ns);
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    let rank = (count * 99).div_ceil(100);
    let mut cumulative = 0;
    let p99 = latency
        .iter()
        .position(|n| {
            cumulative += n;
            cumulative >= rank
        })
        .unwrap();
    Ok(
        serde_json::json!({"sent":count,"verified":count,"elapsed_seconds":elapsed,"deliveries_per_second":count as f64/elapsed,"p99_upper_ms":(1u128<<(p99+1)) as f64/1e6,"max_latency_ms":maximum as f64/1e6,"sent_encoded_bytes":count*packets[0].0.len() as u64,"delivered_encoded_bytes":count*packets[0].1.len() as u64,"value_bytes":count*protocol::parse(&packets[0].0).unwrap().1.len() as u64}),
    )
}

async fn join(url: &str, channel: &str) -> Result<(Socket, u64, usize)> {
    let config = WebSocketConfig::default().read_buffer_size(4096);
    let (mut socket, _) = timeout(
        Duration::from_secs(5),
        connect_async_with_config(url, Some(config), true),
    )
    .await??;
    socket
        .send(Message::Binary(protocol::text(channel)))
        .await?;
    let welcome = data(&mut socket).await?;
    let id = minicbor::Decoder::new(&welcome).u64()?;
    ensure!(id != 0 && id != protocol::BROADCAST, "invalid ID");
    Ok((socket, id, welcome.len()))
}

async fn data(socket: &mut Socket) -> Result<Bytes> {
    match timeout(Duration::from_secs(5), socket.next())
        .await?
        .context("socket closed")??
    {
        Message::Binary(bytes) => Ok(bytes),
        message => anyhow::bail!("unexpected data message: {message:?}"),
    }
}

async fn close(socket: &mut Socket) -> Result<()> {
    let expected = CloseFrame {
        code: CloseCode::Normal,
        reason: "registry-peak".into(),
    };
    socket.send(Message::Close(Some(expected.clone()))).await?;
    let reply = timeout(Duration::from_secs(5), socket.next())
        .await?
        .context("missing Close acknowledgement")??;
    ensure!(
        reply == Message::Close(Some(expected)),
        "Close changed: {reply:?}"
    );
    socket.flush().await?;
    ensure!(
        timeout(Duration::from_secs(5), socket.next())
            .await?
            .is_none(),
        "expected EOF after Close"
    );
    Ok(())
}
