//! Concurrent inspection, private traffic and connection churn over real sockets.
//! The controller gates setup, measurement and cleanup; every output is checked.
#[path = "inspection/fanout.rs"]
mod fanout;
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use clap::{Parser, ValueEnum};
use futures_util::{SinkExt, StreamExt};
use minicbor::Decoder;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet},
    io::{self, Write},
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    time::{MissedTickBehavior, timeout},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        Message,
        protocol::{CloseFrame, WebSocketConfig},
    },
};
use war_relay::protocol;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
const TOKEN: &str = "inspection-benchmark";
const WINDOW: usize = 16;
static AUDITED_IDS: OnceLock<parking_lot::Mutex<HashSet<u64>>> = OnceLock::new();

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Mode {
    None,
    Cbor,
    Binary,
    Http,
}

#[derive(Clone, Copy, ValueEnum, PartialEq)]
enum Wire {
    Cbor,
    Cpp,
}

impl Wire {
    fn envelope(self, target: u64, value: &[u8]) -> Bytes {
        match self {
            Self::Cbor => protocol::envelope(target, value),
            Self::Cpp => {
                let mut bytes = Vec::with_capacity(8 + value.len());
                bytes.extend_from_slice(&target.to_le_bytes());
                bytes.extend_from_slice(value);
                bytes.into()
            }
        }
    }
    fn query(self) -> Bytes {
        match self {
            Self::Cbor => protocol::request(2, b"\xf6"),
            Self::Cpp => self.envelope(0, &[2]),
        }
    }
    fn check_query(self, expected: &Expected, bytes: &[u8], quiescent: bool) -> Result<()> {
        match self {
            Self::Cbor => expected.cbor(bytes, quiescent),
            Self::Cpp => expected.cpp(bytes),
        }
    }
}

#[derive(Parser)]
struct Args {
    #[arg(long)]
    url: String,
    #[arg(long)]
    #[arg(default_value = "127.0.0.1:1339")]
    admin: String,
    #[arg(long, value_enum, default_value = "cbor")]
    wire: Wire,
    /// Verify setup/cleanup using a retained authenticated socket, without HTTP.
    #[arg(long)]
    protocol_only: bool,
    /// Diagnostic only: reject repeated welcome IDs before the first C++ GC.
    #[arg(long)]
    audit_ids: bool,
    #[arg(long, value_enum, default_value = "http")]
    mode: Mode,
    /// Inspect the first bounded HTTP page during traffic; traverse all pages at gates.
    #[arg(long)]
    page_size: Option<usize>,
    #[arg(long, default_value_t = 8192)]
    rooms: usize,
    #[arg(long, default_value_t = 8)]
    seconds: u64,
    #[arg(long, default_value_t = 2)]
    inspectors: usize,
    /// Per-inspector interval; missed ticks are skipped, never replayed in a burst.
    #[arg(long, default_value_t = 10)]
    inspection_interval_ms: u64,
    #[arg(long, default_value_t = 2)]
    pairs: usize,
    #[arg(long, default_value_t = 8)]
    churn_workers: usize,
    /// Add a stable broadcast room with this many fully verifying readers.
    #[arg(long, default_value_t = 0)]
    fanout_readers: usize,
}

#[derive(Default, Serialize)]
struct Counts {
    joins: u64,
    normal_closes: u64,
    sent_messages: u64,
    sent_bytes: u64,
    deliveries: u64,
    delivered_bytes: u64,
    value_bytes: u64,
}
impl Counts {
    fn merge(&mut self, other: &Self) {
        self.joins += other.joins;
        self.normal_closes += other.normal_closes;
        self.sent_messages += other.sent_messages;
        self.sent_bytes += other.sent_bytes;
        self.deliveries += other.deliveries;
        self.delivered_bytes += other.delivered_bytes;
        self.value_bytes += other.value_bytes;
    }
    fn sent(&mut self, bytes: &Bytes) {
        self.sent_messages += 1;
        self.sent_bytes += bytes.len() as u64;
    }
    fn received(&mut self, bytes: &Bytes) {
        self.deliveries += 1;
        self.delivered_bytes += bytes.len() as u64;
    }
}

struct Histogram {
    n: u64,
    buckets: [u64; 64],
    maximum: u64,
}
impl Histogram {
    fn new() -> Self {
        Self {
            n: 0,
            buckets: [0; 64],
            maximum: 0,
        }
    }
    fn record(&mut self, elapsed: Duration) {
        let ns = elapsed.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.n += 1;
        self.buckets[ns.checked_ilog2().unwrap_or(0) as usize] += 1;
        self.maximum = self.maximum.max(ns);
    }
    fn merge(&mut self, other: &Self) {
        self.n += other.n;
        self.maximum = self.maximum.max(other.maximum);
        for (a, b) in self.buckets.iter_mut().zip(other.buckets) {
            *a += b;
        }
    }
    fn report(&self, seconds: f64) -> serde_json::Value {
        let percentile = |p: u64| {
            if self.n == 0 {
                return 0.0;
            }
            let rank = (self.n * p).div_ceil(100);
            let mut n = 0;
            self.buckets
                .iter()
                .position(|v| {
                    n += v;
                    n >= rank
                })
                .map_or(0.0, |i| (1u128 << (i + 1)) as f64 / 1e6)
        };
        serde_json::json!({"verified":self.n,"per_second":self.n as f64/seconds,"p50_upper_ms":percentile(50),"p99_upper_ms":percentile(99),"max_ms":self.maximum as f64/1e6})
    }
}

#[derive(Deserialize, PartialEq, Debug)]
#[serde(deny_unknown_fields)]
struct Info {
    name: Option<String>,
    name_base64: String,
    members: u64,
    variables: u64,
    variable_bytes: u64,
}
struct Expected {
    raw: HashMap<Bytes, u64>,
    json: HashMap<String, Info>,
    transient_raw: HashMap<Bytes, ()>,
    transient_json: HashMap<String, Info>,
    sorted_json: Vec<String>,
    page_size: Option<usize>,
}
impl Expected {
    fn new(stable: &[(Bytes, u64)], workers: usize, page_size: Option<usize>) -> Self {
        let info = |name: &Bytes, members| Info {
            name: std::str::from_utf8(name).ok().map(str::to_owned),
            name_base64: STANDARD.encode(name),
            members,
            variables: u64::from(name.as_ref() == b"health0"),
            variable_bytes: if name.as_ref() == b"health0" { 6 } else { 0 },
        };
        let json: HashMap<_, _> = stable
            .iter()
            .map(|(name, n)| {
                let entry = info(name, *n);
                (entry.name_base64.clone(), entry)
            })
            .collect();
        let mut sorted_json: Vec<_> = json.keys().cloned().collect();
        sorted_json.sort_unstable();
        let transient: Vec<_> = (0..workers)
            .map(|i| Bytes::from(format!("churn{i}")))
            .collect();
        Self {
            raw: stable.iter().cloned().collect(),
            json,
            sorted_json,
            page_size,
            transient_raw: transient.iter().map(|n| (n.clone(), ())).collect(),
            transient_json: transient
                .iter()
                .map(|n| {
                    let entry = info(n, 1);
                    (entry.name_base64.clone(), entry)
                })
                .collect(),
        }
    }
    fn cbor(&self, bytes: &[u8], quiescent: bool) -> Result<()> {
        let mut d = Decoder::new(bytes);
        ensure!(
            d.array()? == Some(2) && d.u64()? == 0 && d.array()? == Some(2) && d.u8()? == 2,
            "query envelope changed"
        );
        let count = d.array()?.context("indefinite query list")? as usize;
        ensure!(
            count >= self.raw.len() && count <= self.raw.len() + self.transient_raw.len(),
            "query length changed"
        );
        let mut fixed = 0;
        let mut previous: Option<&[u8]> = None;
        for _ in 0..count {
            ensure!(d.array()? == Some(2), "query entry shape");
            let name = d.bytes()?;
            let members = d.u64()?;
            ensure!(
                previous.is_none_or(|p| p < name),
                "raw-byte sorting/duplicates changed"
            );
            previous = Some(name);
            if let Some(expected) = self.raw.get(name) {
                ensure!(*expected == members, "stable members changed");
                fixed += 1;
            } else {
                ensure!(
                    !quiescent && self.transient_raw.contains_key(name) && members == 1,
                    "unexpected transient room"
                );
            }
        }
        ensure!(
            fixed == self.raw.len() && d.position() == bytes.len(),
            "missing channel/trailing CBOR"
        );
        Ok(())
    }
    fn cpp(&self, bytes: &[u8]) -> Result<()> {
        ensure!(
            bytes.len() >= 12 && bytes[..8] == [0; 8],
            "binary query header"
        );
        let count = u32::from_le_bytes(bytes[8..12].try_into()?) as usize;
        ensure!(count == self.raw.len(), "binary query channel count");
        let mut names = Vec::with_capacity(count);
        let mut position = 12;
        for _ in 0..count {
            let len = usize::from(*bytes.get(position).context("missing name length")?);
            position += 1;
            let name = bytes
                .get(position..position + len)
                .context("truncated name")?;
            names.push(name);
            position += len;
        }
        ensure!(
            bytes.len() == position + count * 4,
            "binary query count/trailing bytes"
        );
        // The upstream protocol does not promise an order; normalize it only here.
        let mut entries: Vec<_> = names
            .into_iter()
            .zip(bytes[position..].chunks_exact(4))
            .map(|(name, count)| {
                (
                    name,
                    u64::from(u32::from_le_bytes(count.try_into().unwrap())),
                )
            })
            .collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let mut previous: Option<&[u8]> = None;
        for (name, members) in entries {
            ensure!(
                previous.is_none_or(|p| p < name),
                "duplicate binary channel"
            );
            ensure!(
                self.raw.get(name) == Some(&members),
                "binary name/member mismatch"
            );
            previous = Some(name);
        }
        Ok(())
    }
    fn http(&self, bytes: &[u8], quiescent: bool) -> Result<()> {
        let rows: Vec<Info> = serde_json::from_slice(bytes)?;
        ensure!(
            rows.len() >= self.json.len()
                && rows.len() <= self.json.len() + self.transient_json.len(),
            "HTTP list length"
        );
        let mut fixed = 0;
        let mut previous: Option<&str> = None;
        for row in &rows {
            ensure!(
                previous.is_none_or(|p| p < row.name_base64.as_str()),
                "base64 sorting/duplicates changed"
            );
            previous = Some(&row.name_base64);
            if let Some(expected) = self.json.get(&row.name_base64) {
                ensure!(expected == row, "HTTP metadata changed: {row:?}");
                fixed += 1;
            } else {
                ensure!(
                    !quiescent && self.transient_json.get(&row.name_base64) == Some(row),
                    "unexpected HTTP transient"
                );
            }
        }
        ensure!(fixed == self.json.len(), "missing HTTP channel");
        Ok(())
    }

    fn http_page(
        &self,
        bytes: &[u8],
        after: Option<&str>,
        limit: usize,
        quiescent: bool,
    ) -> Result<Option<String>> {
        let rows: Vec<Info> = serde_json::from_slice(bytes)?;
        ensure!(rows.len() <= limit, "HTTP page exceeded limit");
        let mut next = self
            .sorted_json
            .partition_point(|key| after.is_some_and(|after| key.as_str() <= after));
        let mut previous = after;
        for row in &rows {
            ensure!(
                previous.is_none_or(|key| key < row.name_base64.as_str()),
                "page cursor/order changed"
            );
            previous = Some(&row.name_base64);
            if let Some(expected) = self.json.get(&row.name_base64) {
                ensure!(expected == row, "page metadata changed: {row:?}");
                ensure!(
                    self.sorted_json.get(next) == Some(&row.name_base64),
                    "page skipped a stable channel"
                );
                next += 1;
            } else {
                ensure!(
                    !quiescent && self.transient_json.get(&row.name_base64) == Some(row),
                    "unexpected page transient"
                );
            }
        }
        if let Some(last) = previous {
            ensure!(
                self.sorted_json
                    .get(next)
                    .is_none_or(|key| key.as_str() > last),
                "page omitted a stable channel before its last key"
            );
        }
        if rows.len() < limit {
            ensure!(
                next == self.sorted_json.len(),
                "short page omitted remaining stable channels"
            );
        }
        Ok((quiescent && rows.len() == limit).then(|| rows.last().unwrap().name_base64.clone()))
    }
}

fn gate(event: &str, details: serde_json::Value) -> Result<()> {
    println!("{}", serde_json::json!({"event":event,"details":details}));
    io::stdout().flush()?;
    let mut s = String::new();
    io::stdin().read_line(&mut s)?;
    ensure!(s == "continue\n", "expected controller continuation");
    Ok(())
}
async fn data(socket: &mut Socket) -> Result<Bytes> {
    match timeout(Duration::from_secs(5), socket.next())
        .await?
        .context("missing data")??
    {
        Message::Binary(b) => Ok(b),
        m => anyhow::bail!("unexpected {m:?}"),
    }
}
async fn join(url: &str, name: &Bytes, wire: Wire, counts: &mut Counts) -> Result<(Socket, u64)> {
    let (mut socket, _) = timeout(
        Duration::from_secs(5),
        connect_async_with_config(
            url,
            Some(WebSocketConfig::default().read_buffer_size(4096)),
            true,
        ),
    )
    .await??;
    socket
        .send(Message::Binary(match wire {
            Wire::Cbor => protocol::byte_string(name),
            Wire::Cpp => name.clone(),
        }))
        .await?;
    let welcome = data(&mut socket).await?;
    let id = match wire {
        Wire::Cbor => {
            let mut d = Decoder::new(&welcome);
            let id = d.u64()?;
            ensure!(d.position() == welcome.len(), "trailing welcome");
            id
        }
        Wire::Cpp => u64::from_le_bytes(welcome.as_ref().try_into()?),
    };
    ensure!(id != 0 && id != protocol::BROADCAST, "invalid welcome");
    if let Some(ids) = AUDITED_IDS.get() {
        ensure!(
            ids.lock().insert(id),
            "repeated welcome id {id}, room {name:?}"
        );
    }
    counts.joins += 1;
    counts.received(&welcome);
    Ok((socket, id))
}
async fn exchange(socket: &mut Socket, request: Bytes, counts: &mut Counts) -> Result<Bytes> {
    counts.sent(&request);
    socket.send(Message::Binary(request)).await?;
    let result = data(socket).await?;
    counts.received(&result);
    Ok(result)
}
async fn authenticate(socket: &mut Socket, wire: Wire, counts: &mut Counts) -> Result<()> {
    match wire {
        Wire::Cbor => ensure!(
            exchange(socket, protocol::request(0, &protocol::text(TOKEN)), counts)
                .await?
                .as_ref()
                == b"\x82\0\x82\0\xf5",
            "authentication"
        ),
        Wire::Cpp => {
            // The original has no auth acknowledgement; the checked query is the barrier.
            let request = wire.envelope(0, b"\0metalgear");
            counts.sent(&request);
            socket.send(Message::Binary(request)).await?;
        }
    }
    Ok(())
}
async fn close(socket: &mut Socket, counts: &mut Counts) -> Result<()> {
    let frame = CloseFrame {
        code: 1000.into(),
        reason: "inspection".into(),
    };
    socket.send(Message::Close(Some(frame.clone()))).await?;
    let reply = timeout(Duration::from_secs(5), socket.next())
        .await?
        .context("missing Close")??;
    ensure!(
        reply == Message::Close(Some(frame)),
        "Close mismatch: {reply:?}"
    );
    socket.flush().await?;
    ensure!(
        timeout(Duration::from_secs(5), socket.next())
            .await?
            .is_none(),
        "missing EOF"
    );
    counts.normal_closes += 1;
    Ok(())
}
async fn http(
    admin: &str,
    bytes: &mut Vec<u8>,
    expected: &Expected,
    quiescent: bool,
) -> Result<()> {
    let deadline = if quiescent && expected.page_size.is_some() {
        30
    } else {
        5
    };
    timeout(Duration::from_secs(deadline),async {
        let mut after: Option<String> = None;
        loop {
        let mut socket=TcpStream::connect(admin).await?;socket.set_nodelay(true)?;
        if let Some(limit) = expected.page_size {
            let mut path = format!("/v1/channels?limit={limit}");
            if let Some(after) = &after {
                path.push_str("&after=");
                for byte in after.bytes() {
                    match byte {b'+' => path.push_str("%2B"), b'/' => path.push_str("%2F"), b'=' => path.push_str("%3D"), _ => path.push(char::from(byte))}
                }
            }
            socket.write_all(format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer inspection-benchmark\r\nConnection: close\r\n\r\n").as_bytes()).await?;
        } else {
            socket.write_all(b"GET /v1/channels HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer inspection-benchmark\r\nConnection: close\r\n\r\n").await?;
        }
        bytes.clear();(&mut socket).take(2*1024*1024).read_to_end(bytes).await?;
        ensure!(bytes.starts_with(b"HTTP/1.1 200 OK\r\n"),"management status");let end=bytes.windows(4).position(|v| v==b"\r\n\r\n").context("missing HTTP body")?+4;
        if let Some(limit) = expected.page_size {
            after = expected.http_page(&bytes[end..],after.as_deref(),limit,quiescent)?;
            if after.is_none() {break;}
        } else {
            expected.http(&bytes[end..],quiescent)?;
            break;
        }
        }
        Ok(())
    }).await?
}

struct Pair {
    source: Socket,
    target: Socket,
    id: u64,
    packets: Vec<(Bytes, Bytes)>,
}
async fn healthy(mut pair: Pair, until: Instant) -> Result<(Pair, Counts, Histogram)> {
    let mut counts = Counts::default();
    let mut stats = Histogram::new();
    let mut sent = 0usize;
    let mut stamps = [Instant::now(); WINDOW];
    while Instant::now() < until {
        for stamp in &mut stamps {
            *stamp = Instant::now();
            let packet = pair.packets[sent % 64].0.clone();
            counts.sent(&packet);
            counts.value_bytes += 1027;
            pair.source.send(Message::Binary(packet)).await?;
            sent += 1;
        }
        for (i, stamp) in stamps.iter().enumerate() {
            let bytes = data(&mut pair.target)
                .await
                .context("waiting for healthy private delivery")?;
            ensure!(
                bytes == pair.packets[(sent - WINDOW + i) % 64].1,
                "private sender/value/order changed"
            );
            counts.received(&bytes);
            stats.record(stamp.elapsed());
        }
    }
    Ok((pair, counts, stats))
}
async fn churn(
    url: String,
    name: Bytes,
    wire: Wire,
    until: Instant,
) -> Result<(Counts, Histogram)> {
    let mut counts = Counts::default();
    let mut stats = Histogram::new();
    let mut sequence = 0u64;
    while Instant::now() < until {
        let stamp = Instant::now();
        let (mut socket, id) = join(&url, &name, wire, &mut counts)
            .await
            .with_context(|| format!("churn join, room {name:?}, sequence {sequence}"))?;
        let value = protocol::byte_string(&sequence.to_le_bytes());
        let request = wire.envelope(id, &value);
        let reply = exchange(&mut socket, request.clone(), &mut counts)
            .await
            .with_context(|| {
                format!("churn self reply, room {name:?}, sequence {sequence}, id {id}")
            })?;
        ensure!(
            reply == request,
            "churn self delivery changed, room {name:?}, sequence {sequence}, id {id}: expected {request:02x?}, received {reply:02x?}"
        );
        counts.value_bytes += value.len() as u64;
        close(&mut socket, &mut counts)
            .await
            .with_context(|| format!("churn Close, room {name:?}, sequence {sequence}, id {id}"))?;
        stats.record(stamp.elapsed());
        sequence += 1;
    }
    Ok((counts, stats))
}
async fn inspect(
    mut socket: Option<Socket>,
    wire: Wire,
    admin: String,
    expected: Arc<Expected>,
    until: Instant,
    start: Instant,
    interval_ms: u64,
) -> Result<(Option<Socket>, Counts, Histogram)> {
    let mut counts = Counts::default();
    let mut stats = Histogram::new();
    let mut buffer = Vec::with_capacity(1024 * 1024);
    let mut timer = tokio::time::interval_at(
        tokio::time::Instant::from_std(start),
        Duration::from_millis(interval_ms),
    );
    timer.set_missed_tick_behavior(MissedTickBehavior::Skip);
    loop {
        timer.tick().await;
        if Instant::now() >= until {
            break;
        }
        let stamp = Instant::now();
        if let Some(socket) = &mut socket {
            let bytes = exchange(socket, wire.query(), &mut counts).await?;
            wire.check_query(&expected, &bytes, false)?;
        } else {
            http(&admin, &mut buffer, &expected, false).await?;
        }
        stats.record(stamp.elapsed());
    }
    Ok((socket, counts, stats))
}

#[tokio::main(worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.audit_ids {
        ensure!(
            AUDITED_IDS
                .set(parking_lot::Mutex::new(HashSet::new()))
                .is_ok(),
            "audit already initialized"
        );
    }
    ensure!(
        args.rooms <= 8192
            && (1..=60).contains(&args.seconds)
            && (1..=8).contains(&args.pairs)
            && args.churn_workers <= 64
            && args.fanout_readers <= 1024
            && args.inspectors <= 8
            && (1..=1000).contains(&args.inspection_interval_ms),
        "invalid workload bounds"
    );
    ensure!(
        args.page_size
            .is_none_or(|limit| (1..=1000).contains(&limit)
                && args.mode == Mode::Http
                && args.wire == Wire::Cbor),
        "bounded pages require HTTP/CBOR and a limit in 1..=1000"
    );
    ensure!(
        match args.wire {
            Wire::Cpp =>
                args.protocol_only
                    && (args.mode == Mode::None
                        || (args.mode == Mode::Binary && args.churn_workers == 0)),
            Wire::Cbor => args.mode != Mode::Binary,
        },
        "C++ requires --protocol-only and either --mode none or --mode binary --churn-workers 0"
    );
    let inspectors = if args.mode == Mode::None {
        0
    } else {
        args.inspectors
    };
    let mut counts = Counts::default();
    let mut idle = Vec::with_capacity(args.rooms);
    let mut stable = vec![(Bytes::from_static(protocol::GLOBAL_CHANNEL), 0)];
    for i in 0..args.rooms {
        let name = match i {
            0 => Bytes::from_static(b"\xff\0"),
            1 => Bytes::from_static(b"\0"),
            2 => Bytes::from_static("π".as_bytes()),
            _ => Bytes::from(format!("room{i:04x}")),
        };
        idle.push(join(&args.url, &name, args.wire, &mut counts).await?.0);
        stable.push((name, 1));
    }
    let mut pairs = Vec::with_capacity(args.pairs);
    for i in 0..args.pairs {
        let name = Bytes::from(format!("health{i}"));
        let (mut source, id) = join(&args.url, &name, args.wire, &mut counts).await?;
        let (target, target_id) = join(&args.url, &name, args.wire, &mut counts).await?;
        if i == 0 {
            if args.wire == Wire::Cbor {
                ensure!(
                    exchange(
                        &mut source,
                        protocol::request(4, b"\x82\x45ready\xf5"),
                        &mut counts
                    )
                    .await?
                    .as_ref()
                        == b"\x82\0\x82\x04\xf5",
                    "variable setup"
                );
            } else {
                let request = args.wire.envelope(0, b"\x04\x05ready\xf5");
                counts.sent(&request);
                source.send(Message::Binary(request)).await?;
                ensure!(
                    exchange(
                        &mut source,
                        args.wire.envelope(0, b"\x05ready"),
                        &mut counts
                    )
                    .await?
                    .as_ref()
                        == b"\0\0\0\0\0\0\0\0\xc8\xf5",
                    "binary variable setup"
                );
            }
        }
        let mut payload = [0x5a; 1024];
        let packets = (0u64..64)
            .map(|n| {
                payload[..8].copy_from_slice(&n.to_le_bytes());
                let value = protocol::byte_string(&payload);
                (
                    args.wire.envelope(target_id, &value),
                    args.wire.envelope(id, &value),
                )
            })
            .collect();
        pairs.push(Pair {
            source,
            target,
            id,
            packets,
        });
        stable.push((name, 2));
    }
    let mut sockets = Vec::with_capacity(inspectors);
    for i in 0..inspectors {
        if matches!(args.mode, Mode::Cbor | Mode::Binary) {
            let name = Bytes::from(format!("inspect{i}"));
            let (mut socket, _) = join(&args.url, &name, args.wire, &mut counts).await?;
            authenticate(&mut socket, args.wire, &mut counts).await?;
            sockets.push(Some(socket));
            stable.push((name, 1));
        } else {
            sockets.push(None);
        }
    }
    let mut probe = if args.protocol_only {
        let name = Bytes::from_static(b"probe");
        let (mut socket, _) = join(&args.url, &name, args.wire, &mut counts).await?;
        authenticate(&mut socket, args.wire, &mut counts).await?;
        stable.push((name, 1));
        Some(socket)
    } else {
        None
    };
    let fanout = if args.fanout_readers > 0 {
        let group =
            fanout::Fanout::new(&args.url, args.fanout_readers, args.wire, &mut counts).await?;
        stable.push((
            Bytes::from_static(b"fanout"),
            args.fanout_readers as u64 + 1,
        ));
        Some(group)
    } else {
        None
    };
    let expected = Arc::new(Expected::new(&stable, args.churn_workers, args.page_size));
    let mut buffer = Vec::with_capacity(1024 * 1024);
    let mut inspections_verified = 0u64;
    if let Some(socket) = &mut probe {
        let bytes = exchange(socket, args.wire.query(), &mut counts).await?;
        args.wire.check_query(&expected, &bytes, true)?;
        inspections_verified += 1;
        // Also prove every retained inspector authenticated before starting time.
        for socket in sockets.iter_mut().flatten() {
            let bytes = exchange(socket, args.wire.query(), &mut counts).await?;
            args.wire.check_query(&expected, &bytes, true)?;
            inspections_verified += 1;
        }
    } else {
        http(&args.admin, &mut buffer, &expected, true).await?;
        inspections_verified += 1;
    }
    gate(
        "ready",
        serde_json::json!({"counts":counts,"stable_channels":stable.len(),"stable_sessions":counts.joins}),
    )?;
    let started = Instant::now();
    let until = started + Duration::from_secs(args.seconds);
    let fanout = fanout.map(|group| tokio::spawn(group.run(until)));
    let traffic: Vec<_> = pairs
        .into_iter()
        .map(|pair| tokio::spawn(healthy(pair, until)))
        .collect();
    let departures: Vec<_> = (0..args.churn_workers)
        .map(|i| {
            tokio::spawn(churn(
                args.url.clone(),
                Bytes::from(format!("churn{i}")),
                args.wire,
                until,
            ))
        })
        .collect();
    let inspection: Vec<_> = sockets
        .into_iter()
        .map(|s| {
            tokio::spawn(inspect(
                s,
                args.wire,
                args.admin.clone(),
                expected.clone(),
                until,
                started,
                args.inspection_interval_ms,
            ))
        })
        .collect();
    let mut healthy_stats = Histogram::new();
    let mut churn_stats = Histogram::new();
    let mut inspection_stats = Histogram::new();
    let mut pairs = Vec::new();
    let mut sockets = Vec::new();
    let mut churn_lifecycles = Vec::new();
    let mut fanout_stats = Histogram::new();
    for task in traffic {
        let (pair, n, stats) = task.await??;
        counts.merge(&n);
        healthy_stats.merge(&stats);
        pairs.push(pair);
    }
    for task in departures {
        let (n, stats) = task.await??;
        churn_lifecycles.push(stats.n);
        counts.merge(&n);
        churn_stats.merge(&stats);
    }
    for task in inspection {
        let (s, n, stats) = task.await??;
        counts.merge(&n);
        inspection_stats.merge(&stats);
        sockets.push(s);
    }
    let fanout = if let Some(task) = fanout {
        let (group, n, stats) = task.await??;
        counts.merge(&n);
        fanout_stats.merge(&stats);
        Some(group)
    } else {
        None
    };
    let elapsed = started.elapsed().as_secs_f64();
    gate(
        "measured",
        serde_json::json!({"elapsed_seconds":elapsed,"counts":counts,"healthy":healthy_stats.report(elapsed),"churn":churn_stats.report(elapsed),"churn_lifecycles_per_worker":churn_lifecycles,"inspection":inspection_stats.report(elapsed),"fanout":fanout_stats.report(elapsed)}),
    )?;
    // For short C++ churn runs, verify its delayed retirement explicitly. The
    // controller bounds the entire case to finish before the first 30-second GC.
    if let Some(socket) = &mut probe {
        let bytes = exchange(socket, args.wire.query(), &mut counts).await?;
        if args.wire == Wire::Cpp && args.churn_workers > 0 {
            let mut retained = stable.clone();
            retained.extend(
                churn_lifecycles
                    .iter()
                    .enumerate()
                    .filter(|(_, count)| **count > 0)
                    .map(|(i, count)| (Bytes::from(format!("churn{i}")), *count)),
            );
            Expected::new(&retained, 0, None).cpp(&bytes)?;
        } else {
            args.wire.check_query(&expected, &bytes, true)?;
        }
        inspections_verified += 1;
    } else {
        http(&args.admin, &mut buffer, &expected, true).await?;
        inspections_verified += 1;
    }
    for socket in sockets.iter_mut().flatten() {
        let bytes = exchange(socket, args.wire.query(), &mut counts).await?;
        args.wire.check_query(&expected, &bytes, true)?;
        inspections_verified += 1;
    }
    if let Some(group) = fanout {
        group.close(&mut counts).await?;
    }
    for socket in &mut idle {
        close(socket, &mut counts).await?;
    }
    for pair in &mut pairs {
        close(&mut pair.source, &mut counts).await?;
        let departure = data(&mut pair.target).await?;
        ensure!(
            departure
                == args.wire.envelope(
                    protocol::BROADCAST,
                    &match args.wire {
                        Wire::Cbor => protocol::uint(pair.id),
                        Wire::Cpp => Bytes::copy_from_slice(&pair.id.to_le_bytes()),
                    }
                ),
            "departure changed"
        );
        counts.received(&departure);
        close(&mut pair.target, &mut counts).await?;
    }
    for socket in sockets.iter_mut().flatten() {
        close(socket, &mut counts).await?;
    }
    if let Some(socket) = &mut probe {
        close(socket, &mut counts).await?;
    }
    gate(
        "done",
        serde_json::json!({"counts":counts,"inspections_verified":inspection_stats.n+inspections_verified,"retired_churn_members_verified":if args.wire==Wire::Cpp {churn_lifecycles.iter().sum::<u64>()}else{0},"passed":true}),
    )?;
    Ok(())
}
