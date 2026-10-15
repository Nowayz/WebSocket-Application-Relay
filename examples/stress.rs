//! Bounded-memory stability workloads over real WebSockets, independent of duration.
use std::time::{Duration, Instant};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use tokio::{net::TcpStream, task::JoinSet, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use tokio_util::task::AbortOnDropHandle;
use war_relay::protocol;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TEMPLATES: usize = 64;
const MAX_WINDOW: usize = 32;

#[derive(Clone, Copy, Debug, ValueEnum)]
enum Mode {
    Traffic,
    Churn,
    Slow,
}

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "ws://127.0.0.1:1338")]
    url: String,
    #[arg(long, value_enum, default_value = "traffic")]
    mode: Mode,
    /// Traffic uses pairs; churn uses this many independent workers; slow uses three sockets.
    #[arg(long, default_value_t = 64)]
    clients: usize,
    #[arg(long, default_value_t = 30)]
    duration_secs: u64,
    #[arg(long, default_value_t = 1024)]
    payload_bytes: usize,
    #[arg(long, default_value_t = 16)]
    window: usize,
}

/// Fixed logarithmic latency histogram. Reported quantiles are bucket upper bounds.
struct Stats {
    verified: u64,
    joins: u64,
    disconnects: u64,
    latency: [u64; 64],
    max_ns: u64,
}

impl Stats {
    fn new() -> Self {
        Self {
            verified: 0,
            joins: 0,
            disconnects: 0,
            latency: [0; 64],
            max_ns: 0,
        }
    }
    fn record(&mut self, elapsed: Duration) {
        let nanos = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
        let bucket = nanos.checked_ilog2().unwrap_or(0) as usize;
        self.latency[bucket] += 1;
        self.max_ns = self.max_ns.max(nanos);
        self.verified += 1;
    }
    fn merge(&mut self, other: Self) {
        self.verified += other.verified;
        self.joins += other.joins;
        self.disconnects += other.disconnects;
        self.max_ns = self.max_ns.max(other.max_ns);
        for (dst, src) in self.latency.iter_mut().zip(other.latency) {
            *dst += src;
        }
    }
    fn percentile_upper_ms(&self, percent: u64) -> f64 {
        let rank = self.verified.saturating_mul(percent).div_ceil(100);
        let mut count = 0;
        for (bucket, entries) in self.latency.iter().enumerate() {
            count += entries;
            if count >= rank {
                return (1u128 << (bucket + 1)) as f64 / 1_000_000.0;
            }
        }
        0.0
    }
}

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        (2..=10_000).contains(&args.clients),
        "clients must be in 2..=10000"
    );
    ensure!(
        (1..=3600).contains(&args.duration_secs),
        "duration-secs must be in 1..=3600"
    );
    ensure!(
        (8..=1024 * 1024 - 32).contains(&args.payload_bytes),
        "invalid payload size"
    );
    ensure!(
        (1..=MAX_WINDOW).contains(&args.window),
        "window must be in 1..=32"
    );
    let pairs = match args.mode {
        Mode::Traffic => args.clients / 2,
        Mode::Slow => 1,
        Mode::Churn => 0,
    };
    let prepared = pairs as u128
        * TEMPLATES as u128
        * (2 * (args.payload_bytes as u128 + 19) + std::mem::size_of::<(Bytes, Bytes)>() as u128);
    ensure!(
        prepared <= 512 * 1024 * 1024,
        "prepared traffic templates exceed 512 MiB; reduce clients or payload size"
    );
    match args.mode {
        Mode::Traffic => traffic(args).await,
        Mode::Churn => churn(args).await,
        Mode::Slow => slow(args).await,
    }
}

async fn traffic(args: Args) -> Result<()> {
    ensure!(
        args.clients.is_multiple_of(2),
        "traffic requires an even client count"
    );
    let setup = Instant::now();
    let mut work = Vec::with_capacity(args.clients / 2);
    for pair in 0..args.clients / 2 {
        let channel = format!("sustain{pair:x}");
        let (sender, sid) = join(&args.url, &channel).await?;
        let (receiver, rid) = join(&args.url, &channel).await?;
        work.push((sender, receiver, packets(rid, sid, args.payload_bytes)));
    }
    let setup_secs = setup.elapsed().as_secs_f64();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(args.duration_secs);
    println!(
        "{}",
        serde_json::json!({"event":"ready", "mode":"traffic", "clients":args.clients})
    );
    let mut jobs = JoinSet::new();
    for (sender, mut receiver, packets) in work {
        let window = args.window;
        jobs.spawn(async move {
            let (mut sender, _pump) = sender_with_heartbeat(sender);
            let mut sequence = 0usize;
            let mut sent = [Instant::now(); MAX_WINDOW];
            let mut stats = Stats::new();
            stats.joins = 2;
            while Instant::now() < deadline {
                for (i, timestamp) in sent.iter_mut().enumerate().take(window) {
                    *timestamp = Instant::now();
                    sender
                        .send(Message::Binary(
                            packets[(sequence + i) % TEMPLATES].0.clone(),
                        ))
                        .await?;
                }
                for (i, timestamp) in sent.iter().enumerate().take(window) {
                    ensure!(
                        data(&mut receiver).await? == packets[(sequence + i) % TEMPLATES].1,
                        "traffic payload/order mismatch"
                    );
                    stats.record(timestamp.elapsed());
                }
                sequence = (sequence + window) % TEMPLATES;
            }
            Ok::<_, anyhow::Error>(stats)
        });
    }
    let mut stats = Stats::new();
    while let Some(job) = jobs.join_next().await {
        stats.merge(job??);
    }
    report(
        &args,
        setup_secs,
        start.elapsed(),
        &stats,
        serde_json::Value::Null,
    );
    Ok(())
}

async fn churn(args: Args) -> Result<()> {
    let start = Instant::now();
    let deadline = start + Duration::from_secs(args.duration_secs);
    let mut jobs = JoinSet::new();
    println!(
        "{}",
        serde_json::json!({"event":"ready", "mode":"churn", "workers":args.clients})
    );
    for worker in 0..args.clients {
        let url = args.url.clone();
        jobs.spawn(async move {
            let mut stats = Stats::new();
            let channel = format!("churn{worker:x}");
            let mut sequence = 0u64;
            while Instant::now() < deadline {
                let connected = Instant::now();
                let (mut ws, id) = join(&url, &channel).await?;
                stats.joins += 1;
                let value = protocol::byte_string(&sequence.to_le_bytes());
                ws.send(Message::Binary(protocol::envelope(id, &value)))
                    .await?;
                ensure!(
                    data(&mut ws).await? == protocol::envelope(id, &value),
                    "churn self-message mismatch"
                );
                stats.record(connected.elapsed());
                ws.close(None).await?;
                timeout(Duration::from_secs(5), async {
                    while let Some(frame) = ws.next().await {
                        if matches!(frame?, Message::Close(_)) {
                            break;
                        }
                    }
                    Ok::<_, anyhow::Error>(())
                })
                .await??;
                sequence += 1;
            }
            Ok::<_, anyhow::Error>(stats)
        });
    }
    let mut stats = Stats::new();
    while let Some(job) = jobs.join_next().await {
        stats.merge(job??);
    }
    report(&args, 0.0, start.elapsed(), &stats, serde_json::Value::Null);
    Ok(())
}

async fn slow(args: Args) -> Result<()> {
    let setup = Instant::now();
    let (sender, sid) = join(&args.url, "slow-reader").await?;
    let (mut reader, _) = join(&args.url, "slow-reader").await?;
    let (mut slow, slow_id) = join(&args.url, "slow-reader").await?;
    // Read the welcome, then deliberately stop polling this socket until after traffic.
    let packets = packets(protocol::BROADCAST, sid, args.payload_bytes);
    let (mut sender, _pump) = sender_with_heartbeat(sender);
    let setup_secs = setup.elapsed().as_secs_f64();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(args.duration_secs);
    println!(
        "{}",
        serde_json::json!({"event":"ready", "mode":"slow", "slow_id":slow_id.to_string()})
    );
    let mut stats = Stats::new();
    stats.joins = 3;
    let mut sequence = 0usize;
    while Instant::now() < deadline {
        let sent = Instant::now();
        sender
            .send(Message::Binary(packets[sequence].0.clone()))
            .await?;
        loop {
            let bytes = data(&mut reader).await?;
            let (origin, value) =
                protocol::parse(&bytes).map_err(|_| anyhow::anyhow!("invalid forwarded CBOR"))?;
            if origin == protocol::BROADCAST {
                ensure!(
                    minicbor::Decoder::new(value).u64()? == slow_id,
                    "unexpected departed peer"
                );
                stats.disconnects += 1;
                continue;
            }
            ensure!(
                bytes == packets[sequence].1,
                "healthy peer payload/order mismatch under slow-reader pressure"
            );
            break;
        }
        stats.record(sent.elapsed());
        sequence = (sequence + 1) % TEMPLATES;
    }
    let elapsed = start.elapsed();
    let observation = timeout(Duration::from_secs(12), async {
        let mut buffered = 0;
        loop {
            match slow.next().await {
                Some(Ok(Message::Binary(_))) => buffered += 1,
                Some(Ok(Message::Ping(_))) => { let _ = slow.flush().await; }
                Some(Ok(Message::Close(frame))) => break serde_json::json!({"kind":"close", "code":frame.map(|f|u16::from(f.code)), "buffered_frames":buffered}),
                Some(Err(error)) => break serde_json::json!({"kind":"transport_error", "error":error.to_string(), "buffered_frames":buffered}),
                None => break serde_json::json!({"kind":"eof", "buffered_frames":buffered}),
                _ => {}
            }
        }
    }).await.unwrap_or_else(|_| serde_json::json!({"kind":"still_connected"}));
    report(&args, setup_secs, elapsed, &stats, observation);
    Ok(())
}

fn sender_with_heartbeat(ws: Socket) -> (SplitSink<Socket, Message>, AbortOnDropHandle<()>) {
    let (sink, mut input) = ws.split();
    let task = tokio::spawn(async move {
        while let Some(Ok(frame)) = input.next().await {
            if matches!(frame, Message::Close(_)) {
                break;
            }
        }
    });
    (sink, AbortOnDropHandle::new(task))
}

async fn join(url: &str, channel: &str) -> Result<(Socket, u64)> {
    let config = WebSocketConfig::default().read_buffer_size(4096);
    let (mut ws, _) = timeout(
        Duration::from_secs(10),
        connect_async_with_config(url, Some(config), true),
    )
    .await??;
    ws.send(Message::Binary(protocol::text(channel))).await?;
    let welcome = data(&mut ws).await?;
    let id = minicbor::Decoder::new(&welcome).u64()?;
    Ok((ws, id))
}

async fn data(ws: &mut Socket) -> Result<Bytes> {
    timeout(Duration::from_secs(10), async {
        loop {
            match ws
                .next()
                .await
                .context("connection ended before expected data")??
            {
                Message::Binary(bytes) => return Ok(bytes),
                Message::Ping(_) => ws.flush().await?,
                Message::Pong(_) => {}
                other => anyhow::bail!("unexpected frame: {other:?}"),
            }
        }
    })
    .await?
}

fn packets(target: u64, sender: u64, payload_bytes: usize) -> Vec<(Bytes, Bytes)> {
    let mut payload = vec![0x5a; payload_bytes];
    (0..TEMPLATES)
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

fn report(
    args: &Args,
    setup_secs: f64,
    elapsed: Duration,
    stats: &Stats,
    slow_observation: serde_json::Value,
) {
    println!(
        "{}",
        serde_json::json!({
            "event":"result", "mode":format!("{:?}",args.mode).to_lowercase(), "clients":if matches!(args.mode,Mode::Slow){3}else{args.clients},
        "payload_bytes":if matches!(args.mode,Mode::Churn){8}else{args.payload_bytes}, "window":if matches!(args.mode,Mode::Traffic){args.window}else{1},
            "setup_seconds":setup_secs, "elapsed_seconds":elapsed.as_secs_f64(), "verified_deliveries":stats.verified,
            "joins":stats.joins, "disconnect_notifications":stats.disconnects,
            "deliveries_per_second":stats.verified as f64/elapsed.as_secs_f64(),
            "latency_p50_upper_ms":stats.percentile_upper_ms(50), "latency_p99_upper_ms":stats.percentile_upper_ms(99),
            "latency_max_ms":stats.max_ns as f64/1_000_000.0, "slow_observation":slow_observation,
        "validation":if matches!(args.mode,Mode::Churn){"monotonic 8-byte counter per worker, unique assigned sender ID, every self-message and close handshake verified; fixed latency histogram"}else{"64 distinct cyclic templates, window <=32, every delivered frame checked before next window; fixed 64-bucket latency histogram"},
        })
    );
}
