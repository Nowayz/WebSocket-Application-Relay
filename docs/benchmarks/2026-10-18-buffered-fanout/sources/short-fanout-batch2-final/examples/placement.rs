//! Gated private traffic with externally verified Linux epoll socket ownership.
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::{
    collections::HashSet,
    io::{self, Write},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, task::JoinSet, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use war_relay::protocol;

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
const TEMPLATES: usize = 64;
const MAX_WINDOW: usize = 32;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    url: String,
    #[arg(long, default_value_t = 256)]
    pool: usize,
    #[arg(long, default_value_t = 8)]
    seconds: u64,
    #[arg(long, default_value_t = 1024)]
    payload_bytes: usize,
    #[arg(long, default_value_t = 16)]
    window: usize,
}

fn emit(value: serde_json::Value) -> Result<()> {
    println!("{value}");
    io::stdout().flush()?;
    Ok(())
}

fn line() -> Result<String> {
    let mut value = String::new();
    ensure!(
        io::stdin().read_line(&mut value)? > 0,
        "controller ended at gate"
    );
    Ok(value)
}

fn gate() -> Result<()> {
    ensure!(line()?.trim() == "continue", "expected continue gate");
    Ok(())
}

async fn data(socket: &mut Socket) -> Result<Bytes> {
    timeout(Duration::from_secs(10), async {
        loop {
            match socket.next().await.context("missing data")?? {
                Message::Binary(value) => return Ok(value),
                Message::Ping(_) => socket.flush().await?,
                frame => anyhow::bail!("unexpected frame: {frame:?}"),
            }
        }
    })
    .await?
}

struct Stats {
    verified: u64,
    encoded_bytes: u64,
    latency: [u64; 64],
    max_ns: u64,
}

impl Default for Stats {
    fn default() -> Self {
        Self {
            verified: 0,
            encoded_bytes: 0,
            latency: [0; 64],
            max_ns: 0,
        }
    }
}

impl Stats {
    fn record(&mut self, elapsed: Duration, bytes: usize) {
        let ns = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.latency[ns.checked_ilog2().unwrap_or(0) as usize] += 1;
        self.max_ns = self.max_ns.max(ns);
        self.verified += 1;
        self.encoded_bytes += bytes as u64;
    }
    fn merge(&mut self, other: &Self) {
        self.verified += other.verified;
        self.encoded_bytes += other.encoded_bytes;
        self.max_ns = self.max_ns.max(other.max_ns);
        for (a, b) in self.latency.iter_mut().zip(other.latency) {
            *a += b;
        }
    }
    fn p99(&self) -> f64 {
        let rank = self.verified.saturating_mul(99).div_ceil(100);
        let mut count = 0;
        for (index, samples) in self.latency.iter().enumerate() {
            count += samples;
            if count >= rank {
                return (1u128 << (index + 1)) as f64 / 1e6;
            }
        }
        0.0
    }
}

async fn close(mut socket: Socket, ids: Arc<HashSet<u64>>) -> Result<()> {
    timeout(Duration::from_secs(10), async {
        socket.close(None).await?;
        let mut retired = HashSet::new();
        loop {
            match socket
                .next()
                .await
                .context("missing Close acknowledgement")??
            {
                Message::Close(None) => break,
                Message::Ping(_) => socket.flush().await?,
                Message::Binary(bytes) => {
                    let (sender, value) = protocol::parse(&bytes)
                        .map_err(|_| anyhow::anyhow!("invalid teardown CBOR"))?;
                    let mut decoder = minicbor::Decoder::new(value);
                    let id = decoder.u64()?;
                    ensure!(
                        sender == protocol::BROADCAST
                            && decoder.position() == value.len()
                            && ids.contains(&id)
                            && retired.insert(id),
                        "unexpected teardown notification"
                    );
                }
                frame => anyhow::bail!("unexpected Close frame: {frame:?}"),
            }
        }
        socket.flush().await?;
        let mut transport = socket.into_inner();
        let mut byte = [0];
        ensure!(transport.read(&mut byte).await? == 0, "missing TCP EOF");
        Ok(())
    })
    .await?
}

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        args.pool >= 2
            && args.seconds > 0
            && args.payload_bytes >= 8
            && (1..=MAX_WINDOW).contains(&args.window),
        "invalid workload"
    );
    let mut peers = Vec::with_capacity(args.pool);
    let mut inventory = Vec::with_capacity(args.pool);
    let mut ids = HashSet::new();
    for index in 0..args.pool {
        let config = WebSocketConfig::default().read_buffer_size(4096);
        let (mut socket, _) = connect_async_with_config(&args.url, Some(config), true).await?;
        socket
            .send(Message::Binary(protocol::text("placement")))
            .await?;
        let welcome = data(&mut socket).await?;
        let mut decoder = minicbor::Decoder::new(&welcome);
        let id = decoder.u64()?;
        ensure!(
            decoder.position() == welcome.len()
                && id != 0
                && id != protocol::BROADCAST
                && ids.insert(id),
            "invalid or duplicate welcome"
        );
        let MaybeTlsStream::Plain(tcp) = socket.get_ref() else {
            anyhow::bail!("plaintext workload required")
        };
        inventory.push(serde_json::json!({"index":index,"id":id,"port":tcp.local_addr()?.port()}));
        peers.push(Some(socket));
    }
    emit(serde_json::json!({"event":"connected","peers":inventory}))?;
    let plan: Vec<[usize; 2]> = serde_json::from_str(&line()?)?;
    ensure!(!plan.is_empty(), "empty pairing plan");
    let mut used = HashSet::new();
    let mut work = Vec::with_capacity(plan.len());
    for [a, b] in plan {
        ensure!(
            a < peers.len() && b < peers.len() && used.insert(a) && used.insert(b),
            "invalid or overlapping pair"
        );
        let sender = peers[a].take().context("missing sender")?;
        let receiver = peers[b].take().context("missing receiver")?;
        let sid = inventory[a]["id"].as_u64().context("sender ID")?;
        let rid = inventory[b]["id"].as_u64().context("receiver ID")?;
        let packets: Vec<_> = (0..TEMPLATES)
            .map(|sequence| {
                let mut value = vec![0x5a; args.payload_bytes];
                value[..8].copy_from_slice(&(sequence as u64).to_le_bytes());
                let value = protocol::byte_string(&value);
                (
                    protocol::envelope(rid, &value),
                    protocol::envelope(sid, &value),
                )
            })
            .collect();
        work.push((sender, receiver, packets));
    }
    emit(serde_json::json!({"event":"ready","pairs":work.len(),"pool":args.pool}))?;
    gate()?;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(args.seconds);
    let mut jobs = JoinSet::new();
    for (mut sender, mut receiver, packets) in work {
        let window = args.window;
        jobs.spawn(async move {
            let mut sequence = 0;
            let mut stamps = [Instant::now(); MAX_WINDOW];
            let mut stats = Stats::default();
            while Instant::now() < deadline {
                for (i, stamp) in stamps.iter_mut().enumerate().take(window) {
                    *stamp = Instant::now();
                    sender
                        .send(Message::Binary(
                            packets[(sequence + i) % TEMPLATES].0.clone(),
                        ))
                        .await?;
                }
                for (i, stamp) in stamps.iter().enumerate().take(window) {
                    let expected = &packets[(sequence + i) % TEMPLATES].1;
                    ensure!(
                        data(&mut receiver).await? == *expected,
                        "payload, sender or FIFO mismatch"
                    );
                    stats.record(stamp.elapsed(), expected.len());
                }
                sequence = (sequence + window) % TEMPLATES;
            }
            Ok::<_, anyhow::Error>((sender, receiver, packets, stats))
        });
    }
    let mut finished = Vec::new();
    let mut stats = Stats::default();
    while let Some(job) = jobs.join_next().await {
        let result = job??;
        stats.merge(&result.3);
        finished.push(result);
    }
    let elapsed = start.elapsed().as_secs_f64();
    emit(
        serde_json::json!({"event":"measured","verified_deliveries":stats.verified,
        "expected_encoded_bytes":stats.encoded_bytes,"elapsed_seconds":elapsed,
        "deliveries_per_second":stats.verified as f64/elapsed,
        "latency_p99_upper_ms":stats.p99(),"latency_max_ms":stats.max_ns as f64/1e6}),
    )?;
    gate()?;
    let mut postchecks = 0;
    for (mut sender, mut receiver, packets, _) in finished {
        sender.send(Message::Binary(packets[0].0.clone())).await?;
        ensure!(
            data(&mut receiver).await? == packets[0].1,
            "postcheck mismatch"
        );
        peers.push(Some(sender));
        peers.push(Some(receiver));
        postchecks += 1;
    }
    emit(serde_json::json!({"event":"postcheck","pairs":postchecks}))?;
    gate()?;
    let ids = Arc::new(ids);
    let mut closes = JoinSet::new();
    for socket in peers.into_iter().flatten() {
        closes.spawn(close(socket, ids.clone()));
    }
    let mut retired = 0;
    while let Some(result) = closes.join_next().await {
        result??;
        retired += 1;
    }
    ensure!(retired == args.pool, "incomplete retirement");
    emit(serde_json::json!({"event":"done","normal_close_and_eof":retired}))?;
    Ok(())
}
