//! Paced open-loop private/fanout/mixed traffic, checked at every recipient.
//! Missing datagrams are observation-window misses, not attributed network loss.
#[allow(dead_code)]
#[path = "support/socket.rs"]
mod transport_socket;
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::{Parser, ValueEnum};
use serde::Serialize;
use std::{
    collections::HashSet,
    io::{self, Write},
    sync::{
        Arc, OnceLock,
        atomic::{
            AtomicU64,
            Ordering::{Acquire, Release},
        },
    },
    time::{Duration, Instant},
};
use tokio::{
    sync::{Barrier, mpsc, watch},
    task::JoinSet,
    time::timeout,
};
use transport_socket::Socket;
use tungstenite::{Message, protocol::CloseFrame};
use war_relay::protocol;

#[derive(Clone, Copy, PartialEq, ValueEnum, Serialize)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    Websocket,
    Stream,
    Datagram,
}
#[derive(Parser)]
struct Args {
    #[arg(long)]
    url: String,
    #[arg(long, value_enum)]
    mode: Mode,
    #[arg(long)]
    certificate: Option<String>,
    #[arg(long, default_value_t = 32)]
    pairs: usize,
    #[arg(long, default_value_t = 0)]
    readers: usize,
    #[arg(long, default_value_t = 1000)]
    private_rate: u64,
    #[arg(long, default_value_t = 1000)]
    fanout_rate: u64,
    #[arg(long, default_value_t = 4)]
    seconds: u64,
    #[arg(long, default_value_t = 1000)]
    drain_ms: u64,
    #[arg(long, default_value_t = 1024)]
    payload_bytes: usize,
    #[arg(long, default_value_t = 8)]
    client_workers: usize,
    #[arg(long, default_value_t = 4194304)]
    udp_receive_buffer_bytes: usize,
    #[arg(long, default_value_t = 30)]
    io_timeout_secs: u64,
}
#[derive(Clone)]
struct Histogram {
    buckets: [u64; 64],
    n: u64,
    max: u64,
}
impl Default for Histogram {
    fn default() -> Self {
        Self {
            buckets: [0; 64],
            n: 0,
            max: 0,
        }
    }
}
impl Histogram {
    fn record(&mut self, ns: u64) {
        self.n += 1;
        self.max = self.max.max(ns);
        self.buckets[ns.checked_ilog2().unwrap_or(0) as usize] += 1;
    }
    fn merge(&mut self, other: &Self) {
        self.n += other.n;
        self.max = self.max.max(other.max);
        for (a, b) in self.buckets.iter_mut().zip(other.buckets) {
            *a += b;
        }
    }
    fn report(&self) -> serde_json::Value {
        let percentile = |p: u64| {
            let rank = (self.n as u128 * p as u128).div_ceil(100);
            let mut n = 0u128;
            self.buckets
                .iter()
                .position(|x| {
                    n += *x as u128;
                    n >= rank
                })
                .map_or(0., |i| {
                    if self.n == 0 {
                        0.
                    } else {
                        (1u128 << (i + 1)) as f64 / 1e6
                    }
                })
        };
        serde_json::json!({"samples":self.n,"p50_upper_ms":percentile(50),"p99_upper_ms":percentile(99),"max_ms":self.max as f64/1e6})
    }
}
struct Source {
    socket: Socket,
    sender: u64,
    target: u64,
    rate: u64,
    stamps: Arc<Vec<AtomicU64>>,
    fanout: bool,
    template: Vec<u8>,
}
struct Recipient {
    socket: Socket,
    id: u64,
    sender: u64,
    rate: u64,
    stamps: Arc<Vec<AtomicU64>>,
    fanout: bool,
    seen: Vec<u64>,
}
#[derive(Default)]
struct Received {
    unique: u64,
    in_offer: u64,
    duplicates: u64,
    reordered: u64,
    latest: Option<usize>,
    age: Histogram,
    scheduled_age: Histogram,
}
struct SendResult {
    source: Source,
    generated: u64,
    lag: Histogram,
    last_submit_ns: u64,
}
struct ReadResult {
    recipient: Recipient,
    observed: Received,
}
fn stamps(count: usize) -> Result<Arc<Vec<AtomicU64>>> {
    let mut v = Vec::new();
    v.try_reserve_exact(count)?;
    v.resize_with(count, || AtomicU64::new(0));
    Ok(Arc::new(v))
}
fn count(rate: u64, seconds: u64) -> Result<usize> {
    ensure!(rate > 0, "rate must be positive");
    Ok(usize::try_from(
        rate.checked_mul(seconds)
            .context("planned update count overflow")?,
    )?)
}
fn due_ns(sequence: usize, rate: u64) -> Result<u64> {
    Ok(u64::try_from(
        (sequence as u128) * 1_000_000_000 / (rate as u128),
    )?)
}
fn elapsed_ns(start: Instant) -> Result<u64> {
    Ok(start.elapsed().as_nanos().try_into()?)
}
async fn binary(socket: &mut Socket) -> Result<Bytes> {
    match socket.next().await.context("unexpected reliable EOF")?? {
        Message::Binary(bytes) => Ok(bytes),
        other => anyhow::bail!("unexpected frame {other:?}"),
    }
}
async fn peer(args: &Args, room: &str) -> Result<(Socket, u64)> {
    let mut socket = if args.mode == Mode::Websocket {
        Socket::websocket(
            tokio_tungstenite::connect_async_with_config(
                &args.url,
                Some(tungstenite::protocol::WebSocketConfig::default().read_buffer_size(4096)),
                true,
            )
            .await?
            .0,
        )
    } else {
        Socket::connect(&args.url).await?
    };
    socket.send(Message::Binary(protocol::text(room))).await?;
    let bytes = binary(&mut socket).await?;
    let mut d = minicbor::Decoder::new(&bytes);
    let id = d.u64()?;
    ensure!(d.position() == bytes.len(), "trailing welcome bytes");
    Ok((socket, id))
}
async fn publish(
    source: Source,
    clock: Arc<OnceLock<Instant>>,
    args: Arc<Args>,
    barrier: Arc<Barrier>,
) -> Result<SendResult> {
    barrier.wait().await;
    let start = *clock.get().context("start clock missing")?;
    let offset = source.template.len() - args.payload_bytes;
    let end = start + Duration::from_secs(args.seconds);
    let mut result = SendResult {
        source,
        generated: 0,
        lag: Histogram::default(),
        last_submit_ns: 0,
    };
    for sequence in 0..result.source.stamps.len() {
        let scheduled = due_ns(sequence, result.source.rate)?;
        let due = start + Duration::from_nanos(scheduled);
        if Instant::now() < due {
            tokio::time::sleep_until(due.into()).await;
        }
        if Instant::now() >= end {
            break;
        }
        let now = elapsed_ns(start)?;
        let stamp = now.checked_add(1).context("stamp overflow")?;
        let mut packet = result.source.template.clone();
        packet[offset..offset + 8].copy_from_slice(&(sequence as u64).to_be_bytes());
        packet[offset + 8..offset + 16].copy_from_slice(&stamp.to_be_bytes());
        result.source.stamps[sequence].store(stamp, Release);
        let packet = Bytes::from(packet);
        if args.mode == Mode::Datagram {
            result.source.socket.send_datagram(packet)?;
        } else {
            result.source.socket.send(Message::Binary(packet)).await?;
        }
        result.generated += 1;
        result.lag.record(now.saturating_sub(scheduled));
        result.last_submit_ns = elapsed_ns(start)?;
        tokio::task::consume_budget().await;
    }
    Ok(result)
}
struct Expected<'a> {
    sender: u64,
    rate: u64,
    stamps: &'a [AtomicU64],
}
fn verify(
    packet: &[u8],
    expected: Expected<'_>,
    o: &mut Received,
    seen: &mut [u64],
    now: u64,
    args: &Args,
) -> Result<()> {
    let mut d = minicbor::Decoder::new(packet);
    ensure!(
        d.array()? == Some(2) && d.u64()? == expected.sender,
        "sender or envelope mismatch"
    );
    let body = d.bytes()?;
    ensure!(
        d.position() == packet.len() && body.len() == args.payload_bytes,
        "value size or trailing data mismatch"
    );
    ensure!(body[16..].iter().all(|x| *x == 0x5a), "corrupt payload");
    let seq = usize::try_from(u64::from_be_bytes(body[..8].try_into()?))?;
    ensure!(
        seq < expected.stamps.len(),
        "sequence outside planned workload"
    );
    let stamp = u64::from_be_bytes(body[8..16].try_into()?);
    ensure!(
        stamp > 0 && stamp == expected.stamps[seq].load(Acquire),
        "generation stamp mismatch"
    );
    ensure!(stamp - 1 <= now, "future stamp");
    ensure!(
        stamp > due_ns(seq, expected.rate)?,
        "update generated before scheduled slot"
    );
    let mask = 1u64 << (seq % 64);
    let slot = &mut seen[seq / 64];
    if *slot & mask != 0 {
        o.duplicates += 1;
        ensure!(args.mode == Mode::Datagram, "duplicate reliable update");
        return Ok(());
    }
    *slot |= mask;
    if o.latest.is_some_and(|last| seq < last) {
        o.reordered += 1;
    }
    if args.mode != Mode::Datagram {
        ensure!(seq as u64 == o.unique, "reliable FIFO gap or reordering");
    }
    o.latest = Some(o.latest.map_or(seq, |x| x.max(seq)));
    o.unique += 1;
    if now < args.seconds * 1_000_000_000 {
        o.in_offer += 1;
    }
    o.age.record(now - (stamp - 1));
    o.scheduled_age
        .record(now.saturating_sub(due_ns(seq, expected.rate)?));
    Ok(())
}
async fn receive(
    mut r: Recipient,
    clock: Arc<OnceLock<Instant>>,
    args: Arc<Args>,
    barrier: Arc<Barrier>,
) -> Result<ReadResult> {
    let mut seen = std::mem::take(&mut r.seen);
    barrier.wait().await;
    let start = *clock.get().context("start clock missing")?;
    let end = start + Duration::from_secs(args.seconds) + Duration::from_millis(args.drain_ms);
    let mut observed = Received::default();
    loop {
        if Instant::now() >= end {
            break;
        }
        let packet = if args.mode == Mode::Datagram {
            tokio::time::timeout_at(end.into(), r.socket.next_datagram_checked()).await
        } else {
            tokio::time::timeout_at(end.into(), binary(&mut r.socket)).await
        };
        let Ok(packet) = packet else {
            break;
        };
        let now = elapsed_ns(start)?;
        if now >= u64::try_from(end.duration_since(start).as_nanos())? {
            break;
        }
        verify(
            &packet?,
            Expected {
                sender: r.sender,
                rate: r.rate,
                stamps: &r.stamps,
            },
            &mut observed,
            &mut seen,
            now,
            &args,
        )?;
    }
    Ok(ReadResult {
        recipient: r,
        observed,
    })
}
fn departure(packet: &[u8]) -> Result<u64> {
    let mut d = minicbor::Decoder::new(packet);
    ensure!(
        d.array()? == Some(2) && d.u64()? == protocol::BROADCAST,
        "unexpected close traffic"
    );
    let id = d.u64()?;
    ensure!(d.position() == packet.len(), "trailing departure bytes");
    Ok(id)
}
async fn close(mut socket: Socket) -> Result<()> {
    if socket.close_transport().await? {
        return Ok(());
    }
    let frame = CloseFrame {
        code: 1000.into(),
        reason: "load complete".into(),
    };
    socket.send(Message::Close(Some(frame.clone()))).await?;
    ensure!(
        socket
            .next()
            .await
            .context("close EOF before acknowledgement")??
            == Message::Close(Some(frame)),
        "close acknowledgement mismatch"
    );
    socket.flush().await?;
    ensure!(socket.next().await.is_none(), "missing EOF after close");
    Ok(())
}

async fn close_room(readers: Vec<Socket>, ids: Arc<Vec<u64>>, sender: u64) -> Result<()> {
    let count = readers.len();
    if count == 0 {
        return Ok(());
    }
    // At most one acknowledgement per live peer can be outstanding in a stage.
    let (acks, mut received) = mpsc::channel::<Result<usize>>(count);
    let (advance, stage) = watch::channel(None::<usize>);
    let mut jobs = JoinSet::new();
    for (index, mut socket) in readers.into_iter().enumerate() {
        let mut stage = stage.clone();
        let acks = acks.clone();
        let ids = ids.clone();
        jobs.spawn(async move {
            let result = async {
                ensure!(
                    departure(&binary(&mut socket).await?)? == sender,
                    "source departure mismatch"
                );
                acks.send(Ok(index)).await?;
                loop {
                    stage.changed().await?;
                    let closing = stage.borrow_and_update().context("missing cleanup stage")?;
                    ensure!(closing <= index, "cleanup stage skipped a peer");
                    if closing == index {
                        close(socket).await?;
                        acks.send(Ok(index)).await?;
                        return Ok::<_, anyhow::Error>(());
                    }
                    ensure!(
                        departure(&binary(&mut socket).await?)? == ids[closing],
                        "reader departure mismatch"
                    );
                    acks.send(Ok(index)).await?;
                }
            }
            .await;
            if let Err(error) = result {
                let _ = acks.send(Err(error)).await;
            }
        });
    }
    drop(acks);
    let mut seen = vec![false; count];
    // Initial readiness means every reader has observed the source departure.
    for first in std::iter::once(None).chain((0..count).map(Some)) {
        seen.fill(false);
        if let Some(index) = first {
            advance.send(Some(index))?;
        }
        let first = first.unwrap_or(0);
        for _ in first..count {
            let index = received
                .recv()
                .await
                .context("cleanup acknowledgement EOF")??;
            ensure!(
                index >= first && index < count && !seen[index],
                "invalid cleanup acknowledgement"
            );
            seen[index] = true;
        }
    }
    while let Some(job) = jobs.join_next().await {
        job?;
    }
    Ok(())
}

fn gate(event: &str, details: serde_json::Value) -> Result<()> {
    println!("{}", serde_json::json!({"event":event,"details":details}));
    io::stdout().flush()?;
    let mut line = String::new();
    ensure!(
        io::stdin().read_line(&mut line)? > 0 && line.trim() == "continue",
        "controller gate missing"
    );
    Ok(())
}
fn report(
    readers: &[ReadResult],
    senders: &[SendResult],
    fanout: bool,
    args: &Args,
) -> serde_json::Value {
    let mut total = Received::default();
    for r in readers.iter().filter(|r| r.recipient.fanout == fanout) {
        total.unique += r.observed.unique;
        total.in_offer += r.observed.in_offer;
        total.duplicates += r.observed.duplicates;
        total.reordered += r.observed.reordered;
        total.age.merge(&r.observed.age);
        total.scheduled_age.merge(&r.observed.scheduled_age);
    }
    let mut lag = Histogram::default();
    let mut generated = 0;
    let mut planned = 0;
    let mut last = 0;
    for s in senders.iter().filter(|s| s.source.fanout == fanout) {
        generated += s.generated;
        planned += s.source.stamps.len() as u64;
        lag.merge(&s.lag);
        last = last.max(s.last_submit_ns);
    }
    let per_recipient: Vec<_> = readers.iter().filter(|r| r.recipient.fanout == fanout).map(|r| {
        let source = senders.iter().find(|s| s.source.sender == r.recipient.sender).expect("source validated");
        serde_json::json!({"id":r.recipient.id,"generated":source.generated,"verified_unique":r.observed.unique,"received_during_offer":r.observed.in_offer,"unreceived_within_window":source.generated-r.observed.unique})
    }).collect();
    let recipients = if fanout { args.readers as u64 } else { 1 };
    let expected = generated * recipients;
    serde_json::json!({"per_recipient":per_recipient,"planned_source_updates":planned,"generated_source_updates":generated,"not_generated":planned-generated,"expected_generated_deliveries":expected,"verified_unique":total.unique,"unreceived_within_window":expected-total.unique,"duplicates":total.duplicates,"out_of_order":total.reordered,"received_during_offer":total.in_offer,"received_during_drain":total.unique-total.in_offer,"verified_during_offer_per_second":total.in_offer as f64/args.seconds as f64,"arrival_age":total.age.report(),"scheduled_arrival_age":total.scheduled_age.report(),"generation_lag":lag.report(),"last_submit_ms":last as f64/1e6})
}
async fn run(args: Arc<Args>) -> Result<()> {
    ensure!(
        args.seconds > 0
            && args.io_timeout_secs > 0
            && args.client_workers > 0
            && args.payload_bytes >= 16
            && (args.pairs > 0 || args.readers > 0),
        "invalid workload shape"
    );
    args.seconds
        .checked_mul(1_000_000_000)
        .context("duration overflow")?;
    transport_socket::initialize(
        args.certificate.as_deref(),
        args.udp_receive_buffer_bytes,
        None,
    )?;
    let mut sources = Vec::new();
    let mut recipients = Vec::new();
    for index in 0..args.pairs {
        let channel = u64::try_from(index)?
            .checked_mul(2)
            .context("channel index overflow")?;
        let (source, sender) = peer(&args, &format!("{channel:016x}")).await?;
        let (target, id) = peer(&args, &format!("{:016x}", channel + 1)).await?;
        let stamps = stamps(count(args.private_rate, args.seconds)?)?;
        sources.push(Source {
            socket: source,
            sender,
            target: id,
            rate: args.private_rate,
            stamps: stamps.clone(),
            fanout: false,
            template: Vec::new(),
        });
        recipients.push(Recipient {
            socket: target,
            id,
            sender,
            rate: args.private_rate,
            stamps,
            fanout: false,
            seen: Vec::new(),
        });
    }
    if args.readers > 0 {
        let (source, sender) = peer(&args, "load-fanout").await?;
        let stamps = stamps(count(args.fanout_rate, args.seconds)?)?;
        sources.push(Source {
            socket: source,
            sender,
            target: protocol::BROADCAST,
            rate: args.fanout_rate,
            stamps: stamps.clone(),
            fanout: true,
            template: Vec::new(),
        });
        for _ in 0..args.readers {
            let (socket, id) = peer(&args, "load-fanout").await?;
            recipients.push(Recipient {
                socket,
                id,
                sender,
                rate: args.fanout_rate,
                stamps: stamps.clone(),
                fanout: true,
                seen: Vec::new(),
            });
        }
    }
    let connections = sources.len() + recipients.len();
    let mut ids = HashSet::with_capacity(connections);
    for id in sources
        .iter()
        .map(|s| s.sender)
        .chain(recipients.iter().map(|r| r.id))
    {
        ensure!(ids.insert(id), "duplicate assigned peer ID");
    }

    let body = vec![0x5a; args.payload_bytes];
    for source in &mut sources {
        minicbor::Encoder::new(&mut source.template)
            .array(2)?
            .u64(source.target)?
            .bytes(&body)?;
    }
    for recipient in &mut recipients {
        let words = recipient.stamps.len().div_ceil(64);
        recipient.seen.try_reserve_exact(words)?;
        recipient.seen.resize(words, 0);
    }
    let clock = Arc::new(OnceLock::new());
    let barrier = Arc::new(Barrier::new(connections + 1));
    let mut sends = JoinSet::new();
    let mut reads = JoinSet::new();
    for source in sources {
        sends.spawn(publish(
            source,
            clock.clone(),
            args.clone(),
            barrier.clone(),
        ));
    }
    for recipient in recipients {
        reads.spawn(receive(
            recipient,
            clock.clone(),
            args.clone(),
            barrier.clone(),
        ));
    }
    gate(
        "ready",
        serde_json::json!({"sessions":connections,"pairs":args.pairs,"readers":args.readers}),
    )?;
    let start = Instant::now();
    clock
        .set(start)
        .map_err(|_| anyhow::anyhow!("start clock already set"))?;
    barrier.wait().await;
    let mut sent = Vec::new();
    let mut received = Vec::new();
    timeout(
        Duration::from_secs(args.seconds + args.io_timeout_secs),
        async {
            while let Some(s) = sends.join_next().await {
                sent.push(s??);
            }
            while let Some(r) = reads.join_next().await {
                received.push(r??);
            }
            Ok::<_, anyhow::Error>(())
        },
    )
    .await??;
    let mut reliable_complete = true;
    for r in &received {
        let s = sent
            .iter()
            .find(|s| s.source.sender == r.recipient.sender)
            .context("source missing")?;
        ensure!(
            r.observed.unique <= s.generated,
            "more unique updates than generated"
        );
        if args.mode != Mode::Datagram {
            reliable_complete &= r.observed.unique == s.generated;
        }
    }
    let private = report(&received, &sent, false, &args);
    let fanout = report(&received, &sent, true, &args);
    gate(
        "measured",
        serde_json::json!({"reliable_complete":reliable_complete,"mode":args.mode,"offered_seconds":args.seconds,"observation_seconds":args.seconds as f64+args.drain_ms as f64/1000.,"elapsed_seconds":start.elapsed().as_secs_f64(),"private":private,"fanout":fanout}),
    )?;
    ensure!(
        reliable_complete,
        "reliable update unreceived within observation window"
    );
    let mut closing = JoinSet::new();
    let mut fanout_source = None;
    for source in sent {
        if source.source.fanout {
            fanout_source = Some((source.source.socket, source.source.sender));
        } else {
            closing.spawn(close(source.source.socket));
        }
    }
    let mut room = Vec::new();
    let mut ids = Vec::new();
    for recipient in received {
        if recipient.recipient.fanout {
            ids.push(recipient.recipient.id);
            room.push(recipient.recipient.socket);
        } else {
            closing.spawn(close(recipient.recipient.socket));
        }
    }
    if let Some((source, sender)) = fanout_source {
        closing.spawn(async move {
            close(source).await?;
            close_room(room, Arc::new(ids), sender).await
        });
    }
    timeout(Duration::from_secs(args.io_timeout_secs), async {
        while let Some(done) = closing.join_next().await {
            done??;
        }
        Ok::<_, anyhow::Error>(())
    })
    .await??;
    gate(
        "done",
        serde_json::json!({"passed":true,"normal_closes":connections,"source_departures_verified":args.readers}),
    )?;
    Ok(())
}
fn main() -> Result<()> {
    let args = Arc::new(Args::parse());
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.client_workers)
        .enable_all()
        .build()?
        .block_on(run(args))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(mode: &str) -> Args {
        Args::try_parse_from(["load", "--url", "ws://localhost", "--mode", mode]).unwrap()
    }
    fn packet(sender: u64, seq: u64, stamp: u64, bytes: usize) -> Bytes {
        let mut body = vec![0x5a; bytes];
        body[..8].copy_from_slice(&seq.to_be_bytes());
        body[8..16].copy_from_slice(&stamp.to_be_bytes());
        protocol::envelope(sender, &protocol::byte_string(&body))
    }
    #[test]
    fn datagram_holes_reordering_and_duplicates_are_distinct() {
        let args = args("datagram");
        let stamps = [
            AtomicU64::new(1),
            AtomicU64::new(1_000_001),
            AtomicU64::new(2_000_001),
        ];
        let mut observed = Received::default();
        let mut seen = [0];
        for seq in [2, 0, 2] {
            verify(
                &packet(42, seq, seq * 1_000_000 + 1, args.payload_bytes),
                Expected {
                    sender: 42,
                    rate: 1000,
                    stamps: &stamps,
                },
                &mut observed,
                &mut seen,
                4_000_000,
                &args,
            )
            .unwrap();
        }
        assert_eq!(observed.unique, 2);
        assert_eq!(observed.reordered, 1);
        assert_eq!(observed.duplicates, 1);
        assert_eq!(seen[0], 0b101);
        assert_eq!(observed.age.n, 2);
    }
    #[test]
    fn reliable_fifo_and_payload_provenance_are_enforced() {
        let args = args("stream");
        let stamps = [AtomicU64::new(1), AtomicU64::new(2)];
        let mut observed = Received::default();
        let mut seen = [0];
        let expected = || Expected {
            sender: 42,
            rate: 1000,
            stamps: &stamps,
        };
        assert!(
            verify(
                &packet(42, 1, 2, args.payload_bytes),
                expected(),
                &mut observed,
                &mut seen,
                4,
                &args
            )
            .is_err()
        );
        observed = Received::default();
        seen = [0];
        for bad in [
            packet(43, 0, 1, args.payload_bytes),
            packet(42, 0, 9, args.payload_bytes),
            packet(42, 2, 3, args.payload_bytes),
        ] {
            assert!(verify(&bad, expected(), &mut observed, &mut seen, 4, &args).is_err());
        }
        let mut corrupt = packet(42, 0, 1, args.payload_bytes).to_vec();
        *corrupt.last_mut().unwrap() = 0;
        assert!(verify(&corrupt, expected(), &mut observed, &mut seen, 4, &args).is_err());
        verify(
            &packet(42, 0, 1, args.payload_bytes),
            expected(),
            &mut observed,
            &mut seen,
            4,
            &args,
        )
        .unwrap();
        assert!(
            verify(
                &packet(42, 0, 1, args.payload_bytes),
                expected(),
                &mut observed,
                &mut seen,
                4,
                &args
            )
            .is_err()
        );
    }
}
