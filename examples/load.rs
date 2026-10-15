//! Real WebSocket workload. Preparation is excluded; every delivered frame is checked.
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt, future::try_join_all};
use std::time::{Duration, Instant};
use tokio::{net::TcpStream, task::JoinSet, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config, tungstenite::Message,
};
use war_relay::protocol;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    Private,
    Broadcast,
    Idle,
}

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "ws://127.0.0.1:1338")]
    url: String,
    #[arg(long, value_enum, default_value = "private")]
    mode: Mode,
    #[arg(long, default_value_t = 128)]
    clients: usize,
    #[arg(long, default_value_t = 1000)]
    messages: usize,
    #[arg(long, default_value_t = 256)]
    payload_bytes: usize,
    #[arg(long, default_value_t = 16)]
    window: usize,
    #[arg(long, default_value_t = 15)]
    hold_secs: u64,
    /// Enable Nagle in the load client for a separate transport diagnostic.
    #[arg(long)]
    nagle: bool,
}

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        args.clients >= 2 && args.clients <= 20_000,
        "clients must be in 2..=20000"
    );
    ensure!(
        args.messages > 0 && args.messages <= 1_000_000,
        "invalid message count"
    );
    ensure!(
        args.payload_bytes >= 8 && args.payload_bytes <= 16 * 1024 * 1024 - 32,
        "invalid payload size"
    );
    ensure!((1..=32).contains(&args.window), "window must be in 1..=32");
    let setup = Instant::now();
    match args.mode {
        Mode::Idle => {
            let mut sockets = Vec::with_capacity(args.clients);
            for i in 0..args.clients {
                sockets.push(join(&args.url, &format!("idle{i:x}"), !args.nagle).await?.0);
            }
            println!(
                "{}",
                serde_json::json!({ "mode": "idle", "clients": sockets.len(), "setup_seconds": setup.elapsed().as_secs_f64(), "holding_seconds": args.hold_secs })
            );
            tokio::time::sleep(Duration::from_secs(args.hold_secs)).await;
        }
        Mode::Private => {
            ensure!(
                args.clients.is_multiple_of(2),
                "private mode requires an even number of clients"
            );
            let mut work = Vec::with_capacity(args.clients / 2);
            for pair in 0..args.clients / 2 {
                let channel = format!("bench{pair:x}");
                let (sender, sid) = join(&args.url, &channel, !args.nagle).await?;
                let (receiver, rid) = join(&args.url, &channel, !args.nagle).await?;
                work.push((sender, receiver, packets(&args, rid, sid)));
            }
            let setup_seconds = setup.elapsed().as_secs_f64();
            let start = Instant::now();
            let mut tasks = JoinSet::new();
            for (mut sender, mut receiver, packets) in work {
                let window = args.window;
                tasks.spawn(async move {
                    let mut samples = Vec::with_capacity(packets.len());
                    let mut sent = vec![Instant::now(); window];
                    for batch in packets.chunks(window) {
                        for (i, (packet, _)) in batch.iter().enumerate() {
                            sent[i] = Instant::now();
                            sender.send(Message::Binary(packet.clone())).await?;
                        }
                        for (i, (_, expected)) in batch.iter().enumerate() {
                            ensure!(
                                receive(&mut receiver).await? == *expected,
                                "private payload or order mismatch"
                            );
                            samples.push(sent[i].elapsed().as_secs_f64() * 1000.0);
                        }
                    }
                    Ok::<_, anyhow::Error>(samples)
                });
            }
            let mut samples = Vec::with_capacity(args.messages * args.clients / 2);
            while let Some(result) = tasks.join_next().await {
                samples.extend(result??);
            }
            report(&args, setup_seconds, start.elapsed(), samples);
        }
        Mode::Broadcast => {
            let (mut sender, sid) = join(&args.url, "benchbcast", !args.nagle).await?;
            let mut receivers = Vec::with_capacity(args.clients - 1);
            for _ in 1..args.clients {
                receivers.push(join(&args.url, "benchbcast", !args.nagle).await?.0);
            }
            let packets = packets(&args, protocol::BROADCAST, sid);
            let mut samples = Vec::with_capacity(args.messages * receivers.len());
            let mut sent = vec![Instant::now(); args.window];
            let setup_seconds = setup.elapsed().as_secs_f64();
            let start = Instant::now();
            for batch in packets.chunks(args.window) {
                for (i, (packet, _)) in batch.iter().enumerate() {
                    sent[i] = Instant::now();
                    sender.send(Message::Binary(packet.clone())).await?;
                }
                let results = try_join_all(receivers.iter_mut().map(|receiver| {
                    let sent = &sent;
                    async move {
                        let mut samples = Vec::with_capacity(batch.len());
                        for (i, (_, expected)) in batch.iter().enumerate() {
                            ensure!(
                                receive(receiver).await? == *expected,
                                "broadcast payload or order mismatch"
                            );
                            samples.push(sent[i].elapsed().as_secs_f64() * 1000.0);
                        }
                        Ok::<_, anyhow::Error>(samples)
                    }
                }))
                .await?;
                for result in results {
                    samples.extend(result);
                }
            }
            report(&args, setup_seconds, start.elapsed(), samples);
        }
    }
    Ok(())
}

async fn join(url: &str, channel: &str, disable_nagle: bool) -> Result<(Socket, u64)> {
    let (mut ws, _) = timeout(
        Duration::from_secs(10),
        connect_async_with_config(url, None, disable_nagle),
    )
    .await??;
    ws.send(Message::Binary(protocol::text(channel))).await?;
    let welcome = receive(&mut ws).await?;
    let id = minicbor::Decoder::new(&welcome).u64()?;
    Ok((ws, id))
}

async fn receive(ws: &mut Socket) -> Result<Bytes> {
    timeout(Duration::from_secs(10), async {
        loop {
            match ws.next().await.context("connection closed")?? {
                Message::Binary(bytes) => return Ok(bytes),
                Message::Ping(_) => ws.flush().await?,
                other => anyhow::bail!("unexpected frame: {other:?}"),
            }
        }
    })
    .await?
}

fn packets(args: &Args, target: u64, sender: u64) -> Vec<(Bytes, Bytes)> {
    let mut payload = vec![0x5a; args.payload_bytes];
    (0..args.messages)
        .map(|sequence| {
            payload[..8].copy_from_slice(&(sequence as u64).to_le_bytes());
            let value = protocol::byte_string(&payload);
            (
                protocol::envelope(target, &value),
                protocol::envelope(sender, &value),
            )
        })
        .collect()
}

fn report(args: &Args, setup_seconds: f64, elapsed: Duration, mut samples: Vec<f64>) {
    samples.sort_unstable_by(f64::total_cmp);
    let percentile = |p: f64| samples[((samples.len() - 1) as f64 * p) as usize];
    println!(
        "{}",
        serde_json::json!({
            "mode": format!("{:?}", args.mode).to_lowercase(), "clients": args.clients,
            "client_tcp_nodelay": !args.nagle,
            "messages_per_sender": args.messages, "payload_bytes": args.payload_bytes, "window": args.window,
            "setup_seconds": setup_seconds, "elapsed_seconds": elapsed.as_secs_f64(), "verified_deliveries": samples.len(),
            "deliveries_per_second": samples.len() as f64 / elapsed.as_secs_f64(),
            "latency_p50_ms": percentile(0.5), "latency_p99_ms": percentile(0.99), "latency_max_ms": percentile(1.0),
        })
    );
}
