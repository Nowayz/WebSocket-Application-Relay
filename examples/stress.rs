//! Bounded-memory stability workloads over real WebSockets, independent of duration.
use std::{
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use tokio::{
    net::{TcpSocket, TcpStream},
    sync::{mpsc, watch},
    task::JoinSet,
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, client_async_with_config, connect_async_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
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
    Fanout,
}

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "ws://127.0.0.1:1338")]
    url: String,
    /// Send this exact Origin header on every connection, including churn.
    #[arg(long)]
    origin: Option<String>,
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
    /// Fanout only: cycle these payload sizes through 64 prebuilt templates.
    #[arg(long, value_delimiter = ',')]
    payload_sizes: Vec<usize>,
    /// Fanout only: welcome these peers, then leave them unread during traffic.
    #[arg(long, default_value_t = 0)]
    unread_clients: usize,
    /// Fanout only: delay the first healthy reader by this much per window.
    #[arg(long, default_value_t = 0)]
    reader_delay_ms: u64,
    /// Churn only: rotate over 127.64.x.y source IPs on a loopback ws:// URL;
    /// zero keeps ordinary unbound connections.
    #[arg(long, default_value_t = 0)]
    source_addresses: usize,
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
        args.source_addresses <= 65534,
        "source-addresses must be in 0..=65534"
    );
    ensure!(
        args.source_addresses == 0 || matches!(args.mode, Mode::Churn),
        "source-addresses requires churn mode"
    );
    if args.source_addresses != 0 {
        source_destination(&args.url)?;
    }
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
    ensure!(
        args.payload_sizes.len() <= TEMPLATES
            && args
                .payload_sizes
                .iter()
                .all(|size| (8..=1024 * 1024 - 32).contains(size)),
        "payload-sizes must contain at most 64 values in 8..=1048544"
    );
    ensure!(
        args.unread_clients <= 16 && args.clients >= args.unread_clients + 2,
        "leave at least one sender and one healthy reader; at most 16 unread clients"
    );
    ensure!(
        args.reader_delay_ms <= 1000,
        "reader-delay-ms must be in 0..=1000"
    );
    ensure!(
        matches!(args.mode, Mode::Fanout)
            || (args.payload_sizes.is_empty()
                && args.unread_clients == 0
                && args.reader_delay_ms == 0),
        "payload-sizes, unread-clients, and reader-delay-ms require fanout mode"
    );
    let pairs = match args.mode {
        Mode::Traffic => args.clients / 2,
        Mode::Slow => 1,
        Mode::Churn => 0,
        Mode::Fanout => 1,
    };
    let largest_payload = args
        .payload_sizes
        .iter()
        .copied()
        .max()
        .unwrap_or(args.payload_bytes);
    let prepared = pairs as u128
        * TEMPLATES as u128
        * (2 * (largest_payload as u128 + 19) + std::mem::size_of::<(Bytes, Bytes)>() as u128);
    ensure!(
        prepared <= 512 * 1024 * 1024,
        "prepared traffic templates exceed 512 MiB; reduce clients or payload size"
    );
    match args.mode {
        Mode::Traffic => traffic(args).await,
        Mode::Churn => churn(args).await,
        Mode::Slow => slow(args).await,
        Mode::Fanout => fanout(args).await,
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
        let (sender, sid) = join(&args.url, &channel, args.origin.as_deref()).await?;
        let (receiver, rid) = join(&args.url, &channel, args.origin.as_deref()).await?;
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
    let destination = (args.source_addresses != 0)
        .then(|| source_destination(&args.url))
        .transpose()?;
    let start = Instant::now();
    let deadline = start + Duration::from_secs(args.duration_secs);
    let mut jobs = JoinSet::new();
    println!(
        "{}",
        serde_json::json!({"event":"ready", "mode":"churn", "workers":args.clients,
            "source_addresses":args.source_addresses})
    );
    for worker in 0..args.clients {
        let url = args.url.clone();
        let origin = args.origin.clone();
        let sources = args.source_addresses;
        let workers = args.clients;
        jobs.spawn(async move {
            let mut stats = Stats::new();
            let channel = format!("churn{worker:x}");
            let mut sequence = 0u64;
            while Instant::now() < deadline {
                let connected = Instant::now();
                let (mut ws, id) = if let Some(destination) = destination {
                    let index = 1
                        + (worker as u64 + sequence.wrapping_mul(workers as u64)) % sources as u64;
                    let source = Ipv4Addr::new(127, 64, (index >> 8) as u8, index as u8);
                    join_from(&url, &channel, origin.as_deref(), destination, source).await?
                } else {
                    join(&url, &channel, origin.as_deref()).await?
                };
                stats.joins += 1;
                let mut bytes = [0; 16];
                bytes[..8].copy_from_slice(&(worker as u64).to_le_bytes());
                bytes[8..].copy_from_slice(&sequence.to_le_bytes());
                let value = protocol::byte_string(&bytes);
                ws.send(Message::Binary(protocol::envelope(id, &value)))
                    .await?;
                ensure!(
                    data(&mut ws).await? == protocol::envelope(id, &value),
                    "churn self-message mismatch"
                );
                ws.close(None).await?;
                timeout(Duration::from_secs(5), async {
                    verify_churn_close(&mut ws).await
                })
                .await??;
                stats.record(connected.elapsed());
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

fn source_destination(url: &str) -> Result<SocketAddr> {
    let authority = url
        .strip_prefix("ws://")
        .context("source binding requires ws://")?
        .split('/')
        .next()
        .context("missing loopback address")?;
    let destination: SocketAddr = authority
        .parse()
        .context("source binding requires an IPv4 loopback literal and port")?;
    ensure!(
        destination.is_ipv4() && destination.ip().is_loopback(),
        "source binding requires an IPv4 loopback destination"
    );
    Ok(destination)
}

async fn join_from(
    url: &str,
    channel: &str,
    origin: Option<&str>,
    destination: SocketAddr,
    source: Ipv4Addr,
) -> Result<(Socket, u64)> {
    let socket = TcpSocket::new_v4()?;
    socket.bind((source, 0).into())?;
    let stream = timeout(Duration::from_secs(10), socket.connect(destination)).await??;
    stream.set_nodelay(true)?;
    let mut request = url.into_client_request()?;
    if let Some(origin) = origin {
        request.headers_mut().insert("origin", origin.parse()?);
    }
    let (mut ws, _) = timeout(
        Duration::from_secs(10),
        client_async_with_config(
            request,
            MaybeTlsStream::Plain(stream),
            Some(WebSocketConfig::default().read_buffer_size(4096)),
        ),
    )
    .await??;
    ws.send(Message::Binary(protocol::text(channel))).await?;
    let welcome = data(&mut ws).await?;
    let id = welcome_id(&welcome)?;
    Ok((ws, id))
}

fn welcome_id(bytes: &[u8]) -> Result<u64> {
    let mut decoder = minicbor::Decoder::new(bytes);
    let id = decoder.u64()?;
    ensure!(
        decoder.position() == bytes.len() && id != 0 && id != protocol::BROADCAST,
        "invalid welcome ID"
    );
    Ok(id)
}

async fn verify_churn_close<S>(ws: &mut WebSocketStream<S>) -> Result<()>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    loop {
        match ws.next().await.context("missing Close acknowledgement")?? {
            Message::Close(None) => break,
            Message::Ping(_) => ws.flush().await?,
            frame => anyhow::bail!("unexpected churn Close acknowledgement: {frame:?}"),
        }
    }
    ws.flush().await?;
    ensure!(
        ws.next().await.is_none(),
        "churn transport did not end after Close"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::AsyncWriteExt;
    use tokio_tungstenite::tungstenite::protocol::Role;

    #[tokio::test]
    async fn churn_close_rejects_missing_acknowledgements_wrong_codes_and_application_data() {
        for bytes in [
            &[][..],
            &[0x88, 2, 3, 0xe8],
            &[0x88, 2, 3, 0xf5],
            &[0x82, 1, 1],
        ] {
            let (client, mut peer) = tokio::io::duplex(128);
            let mut ws = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
            ws.close(None).await.unwrap();
            peer.write_all(bytes).await.unwrap();
            peer.shutdown().await.unwrap();
            assert!(
                timeout(Duration::from_secs(1), verify_churn_close(&mut ws))
                    .await
                    .unwrap()
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn churn_close_waits_for_transport_eof_after_the_exact_acknowledgement() {
        let (client, mut peer) = tokio::io::duplex(128);
        let mut ws = WebSocketStream::from_raw_socket(client, Role::Client, None).await;
        ws.close(None).await.unwrap();
        peer.write_all(&[0x88, 0]).await.unwrap();
        let mut check = Box::pin(verify_churn_close(&mut ws));
        assert!(
            timeout(Duration::from_millis(20), check.as_mut())
                .await
                .is_err()
        );
        peer.shutdown().await.unwrap();
        timeout(Duration::from_secs(1), check)
            .await
            .unwrap()
            .unwrap();
    }

    #[test]
    fn source_binding_and_welcome_validation_reject_ambiguous_inputs() {
        assert_eq!(
            source_destination("ws://127.0.0.1:1338/path").unwrap(),
            "127.0.0.1:1338".parse().unwrap()
        );
        for url in [
            "wss://127.0.0.1:1338/",
            "ws://192.0.2.1:1338/",
            "ws://localhost:1338/",
            "ws://[::1]:1338/",
            "ws://127.0.0.1/",
        ] {
            assert!(source_destination(url).is_err());
        }
        assert_eq!(welcome_id(&[1]).unwrap(), 1);
        for bytes in [
            &[][..],
            &[0],
            &[0x61, b'1'],
            &[1, 0],
            &[0x1b, 255, 255, 255, 255, 255, 255, 255, 255],
        ] {
            assert!(welcome_id(bytes).is_err());
        }
    }
}

async fn slow(args: Args) -> Result<()> {
    let setup = Instant::now();
    let (sender, sid) = join(&args.url, "slow-reader", args.origin.as_deref()).await?;
    let (mut reader, _) = join(&args.url, "slow-reader", args.origin.as_deref()).await?;
    let (mut slow, slow_id) = join(&args.url, "slow-reader", args.origin.as_deref()).await?;
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

#[derive(Clone, Copy)]
struct Window {
    first: u64,
    sent: Instant,
}

/// All healthy readers verify the same window before the next one begins. One
/// shared template set and fixed histograms keep memory independent of duration.
async fn fanout(args: Args) -> Result<()> {
    let setup = Instant::now();
    let (sender, sid) = join(&args.url, "fanout", args.origin.as_deref()).await?;
    let healthy = args.clients - args.unread_clients - 1;
    let mut readers = Vec::with_capacity(healthy);
    for _ in 0..healthy {
        readers.push(join(&args.url, "fanout", args.origin.as_deref()).await?.0);
    }
    let mut unread = Vec::with_capacity(args.unread_clients);
    let mut unread_ids = Vec::with_capacity(args.unread_clients);
    for _ in 0..args.unread_clients {
        let (socket, id) = join(&args.url, "fanout", args.origin.as_deref()).await?;
        unread.push(socket);
        unread_ids.push(id);
    }
    unread_ids.sort_unstable();
    let unread_ids = Arc::new(unread_ids);
    let sizes = if args.payload_sizes.is_empty() {
        vec![args.payload_bytes]
    } else {
        args.payload_sizes.clone()
    };
    let packets = Arc::new(packets_with_sizes(protocol::BROADCAST, sid, &sizes));
    let value_sizes: [u64; TEMPLATES] = std::array::from_fn(|index| {
        protocol::parse(&packets[index].0)
            .expect("prepared valid CBOR")
            .1
            .len() as u64
    });
    let (windows, initial) = watch::channel::<Option<Window>>(None);
    let (acknowledge, mut acknowledged) = mpsc::channel(healthy);
    let mut jobs = JoinSet::new();
    for (index, mut reader) in readers.into_iter().enumerate() {
        let packets = packets.clone();
        let unread_ids = unread_ids.clone();
        let mut windows = initial.clone();
        let acknowledge = acknowledge.clone();
        let width = args.window;
        let delay = if index == 0 { args.reader_delay_ms } else { 0 };
        jobs.spawn(async move {
            let mut stats = Stats::new();
            let mut departed = [false; 16];
            while windows.changed().await.is_ok() {
                let window = (*windows.borrow_and_update()).context("missing window")?;
                if delay > 0 {
                    sleep(Duration::from_millis(delay)).await;
                }
                for offset in 0..width {
                    loop {
                        let bytes = data(&mut reader).await?;
                        let (origin, value) = protocol::parse(&bytes)
                            .map_err(|_| anyhow::anyhow!("invalid forwarded CBOR"))?;
                        if origin == protocol::BROADCAST {
                            let id = minicbor::Decoder::new(value).u64()?;
                            let slot = unread_ids
                                .binary_search(&id)
                                .map_err(|_| anyhow::anyhow!("unexpected departed peer {id}"))?;
                            ensure!(!departed[slot], "duplicate disconnect notification");
                            departed[slot] = true;
                            stats.disconnects += 1;
                            continue;
                        }
                        let sequence = (window.first as usize + offset) % TEMPLATES;
                        ensure!(
                            bytes == packets[sequence].1,
                            "reader {index}: payload/order mismatch"
                        );
                        stats.record(window.sent.elapsed());
                        break;
                    }
                }
                acknowledge
                    .send(index)
                    .await
                    .context("acknowledgement receiver stopped")?;
            }
            let capacity_close = close_client(&mut reader)
                .await
                .with_context(|| format!("reader {index} close handshake"))?;
            Ok::<_, anyhow::Error>((stats, capacity_close))
        });
    }
    drop(initial);
    drop(acknowledge);
    let (mut sender, mut pump) = sender_with_heartbeat(sender);
    let mut seen = vec![false; healthy];
    let setup_secs = setup.elapsed().as_secs_f64();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(args.duration_secs);
    let mut sent_messages = 0u64;
    let mut sent_encoded_bytes = 0u64;
    let mut sent_value_bytes = 0u64;
    println!(
        "{}",
        serde_json::json!({"event":"ready", "mode":"fanout", "clients":args.clients,
        "healthy_readers":healthy,"unread_clients":args.unread_clients,"payload_sizes":sizes})
    );
    while Instant::now() < deadline {
        windows.send_replace(Some(Window {
            first: sent_messages,
            sent: Instant::now(),
        }));
        for offset in 0..args.window {
            let sequence = (sent_messages as usize + offset) % TEMPLATES;
            sender
                .send(Message::Binary(packets[sequence].0.clone()))
                .await?;
            sent_encoded_bytes += packets[sequence].0.len() as u64;
            sent_value_bytes += value_sizes[sequence];
        }
        seen.fill(false);
        for _ in 0..healthy {
            let index = timeout(Duration::from_secs(12), async {
                tokio::select! {
                    index = acknowledged.recv() => index.context("all readers stopped"),
                    ended = jobs.join_next() => match ended {
                        Some(job) => { job??; anyhow::bail!("reader ended before final window"); }
                        None => anyhow::bail!("no reader tasks remain"),
                    },
                }
            })
            .await
            .context("healthy reader window timed out")??;
            ensure!(!seen[index], "duplicate window acknowledgement");
            seen[index] = true;
        }
        sent_messages += args.window as u64;
    }
    let elapsed = start.elapsed();
    println!(
        "{}",
        serde_json::json!({"event":"traffic_finished", "mode":"fanout", "sent_messages":sent_messages,
        "verified_deliveries":sent_messages * healthy as u64, "elapsed_seconds":elapsed.as_secs_f64()})
    );
    drop(windows);
    let mut stats = Stats::new();
    stats.joins = args.clients as u64;
    let mut teardown_capacity_closes = 0;
    while let Some(job) = jobs.join_next().await {
        let (reader, capacity_close) = job??;
        stats.merge(reader);
        teardown_capacity_closes += usize::from(capacity_close);
    }
    sender.close().await?;
    timeout(Duration::from_secs(5), &mut pump)
        .await
        .context("sender close timed out")??;
    let observations = futures_util::future::try_join_all(unread.into_iter().map(|mut socket| {
        let packets = packets.clone();
        async move {
            timeout(Duration::from_secs(5), async {
                let mut verified = 0usize;
                loop {
                    match socket.next().await {
                        Some(Ok(Message::Binary(bytes))) => {
                            let (origin, _) = protocol::parse(&bytes).map_err(|_| anyhow::anyhow!("invalid unread-recipient CBOR"))?;
                            if origin != protocol::BROADCAST {
                                ensure!(bytes == packets[verified % TEMPLATES].1, "unread recipient buffered payload/order mismatch");
                                verified += 1;
                            }
                        }
                        Some(Ok(Message::Ping(_))) => { let _ = socket.flush().await; }
                        Some(Ok(Message::Close(frame))) => {
                            ensure!(frame.as_ref().is_some_and(|f| u16::from(f.code) == 1013), "unread recipient did not close for pressure");
                            let _ = socket.flush().await;
                            break Ok(serde_json::json!({"kind":"close","code":1013,"verified_buffered_frames":verified}));
                        }
                        Some(Err(error)) => {
                            use tokio_tungstenite::tungstenite::{Error, error::ProtocolError};
                            ensure!(matches!(error, Error::Io(_) | Error::Protocol(ProtocolError::ResetWithoutClosingHandshake) | Error::ConnectionClosed | Error::AlreadyClosed), "unexpected unread socket error: {error}");
                            break Ok(serde_json::json!({"kind":"transport_error","error":error.to_string(),"verified_buffered_frames":verified}));
                        }
                        None => break Ok(serde_json::json!({"kind":"eof","verified_buffered_frames":verified})),
                        _ => {}
                    }
                }
            }).await.context("unread recipient remained connected")?
        }
    })).await?;
    ensure!(
        stats.verified == sent_messages * healthy as u64,
        "delivery matrix mismatch"
    );
    ensure!(
        stats.disconnects == (args.unread_clients * healthy) as u64,
        "missing disconnect notification"
    );
    let mut result = summary(&args, setup_secs, elapsed, &stats, serde_json::Value::Null);
    result["payload_sizes"] = serde_json::json!(sizes);
    result["payload_bytes"] = serde_json::Value::Null;
    result["sent_messages"] = sent_messages.into();
    result["sent_encoded_bytes"] = sent_encoded_bytes.into();
    result["sent_value_bytes"] = sent_value_bytes.into();
    result["healthy_readers"] = healthy.into();
    result["unread_clients"] = args.unread_clients.into();
    result["reader_delay_ms"] = args.reader_delay_ms.into();
    result["teardown_capacity_closes"] = teardown_capacity_closes.into();
    result["unread_observations"] = serde_json::json!(observations);
    println!("{result}");
    Ok(())
}

async fn close_client(ws: &mut Socket) -> Result<bool> {
    timeout(Duration::from_secs(5), async {
        ws.close(None).await?;
        while let Some(frame) = ws.next().await {
            match frame? {
                Message::Close(frame) => {
                    let capacity = frame
                        .as_ref()
                        .is_some_and(|frame| u16::from(frame.code) == 1013);
                    ensure!(
                        frame.is_none() || capacity,
                        "unexpected healthy reader close: {frame:?}"
                    );
                    ws.flush().await?;
                    return Ok(capacity);
                }
                Message::Ping(_) => ws.flush().await?,
                _ => {}
            }
        }
        anyhow::bail!("connection ended without acknowledging Close")
    })
    .await
    .context("client close timed out")?
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

async fn join(url: &str, channel: &str, origin: Option<&str>) -> Result<(Socket, u64)> {
    let config = WebSocketConfig::default().read_buffer_size(4096);
    let mut request = url.into_client_request()?;
    if let Some(origin) = origin {
        request.headers_mut().insert("origin", origin.parse()?);
    }
    let (mut ws, _) = timeout(
        Duration::from_secs(10),
        connect_async_with_config(request, Some(config), true),
    )
    .await??;
    ws.send(Message::Binary(protocol::text(channel))).await?;
    let welcome = data(&mut ws).await?;
    let id = welcome_id(&welcome)?;
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
    packets_with_sizes(target, sender, &[payload_bytes])
}

fn packets_with_sizes(target: u64, sender: u64, sizes: &[usize]) -> Vec<(Bytes, Bytes)> {
    let mut payload = Vec::with_capacity(*sizes.iter().max().expect("nonempty sizes"));
    (0..TEMPLATES)
        .map(|sequence| {
            payload.resize(sizes[sequence % sizes.len()], 0x5a);
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
        summary(args, setup_secs, elapsed, stats, slow_observation)
    );
}

fn summary(
    args: &Args,
    setup_secs: f64,
    elapsed: Duration,
    stats: &Stats,
    slow_observation: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "event":"result", "mode":format!("{:?}",args.mode).to_lowercase(), "clients":if matches!(args.mode,Mode::Slow){3}else{args.clients},
    "payload_bytes":if matches!(args.mode,Mode::Churn){16}else{args.payload_bytes}, "window":if matches!(args.mode,Mode::Traffic | Mode::Fanout){args.window}else{1},
    "source_addresses":args.source_addresses,
        "setup_seconds":setup_secs, "elapsed_seconds":elapsed.as_secs_f64(), "verified_deliveries":stats.verified,
        "joins":stats.joins, "disconnect_notifications":stats.disconnects,
        "deliveries_per_second":stats.verified as f64/elapsed.as_secs_f64(),
        "latency_p50_upper_ms":stats.percentile_upper_ms(50), "latency_p99_upper_ms":stats.percentile_upper_ms(99),
        "latency_max_ms":stats.max_ns as f64/1_000_000.0, "slow_observation":slow_observation,
    "validation":if matches!(args.mode,Mode::Churn){"distinct worker ID plus monotonic counter; exact self-message, Close(None) acknowledgement and EOF; latency covers connect through TCP retirement; fixed histogram"}else if matches!(args.mode,Mode::Fanout){"64 cyclic templates shared across readers; every full encoded frame and its order checked before next window; unique expected disconnect notifications; latency measured from window start; fixed histogram"}else{"64 distinct cyclic templates, window <=32, every delivered frame checked before next window; fixed 64-bucket latency histogram"},
    })
}
