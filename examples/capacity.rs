//! Real connected-population acceptance, paced private rings and exact retirement.
//! One task owns a small room; setup and teardown have bounded concurrency.
use std::{
    collections::HashSet,
    io::{self, Write},
    net::{Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use minicbor::Decoder;
use serde::Serialize;
use tokio::{
    net::{TcpSocket, TcpStream},
    task::JoinSet,
    time::{sleep_until, timeout},
};
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{
        Message,
        protocol::{CloseFrame, WebSocketConfig},
    },
};
use war_relay::protocol;

type Socket = WebSocketStream<TcpStream>;
const DEADLINE: Duration = Duration::from_secs(15);

#[derive(Parser)]
struct Args {
    #[arg(long)]
    url: String,
    #[arg(long, default_value_t = 100_000)]
    users: usize,
    #[arg(long, default_value_t = 16)]
    room_size: usize,
    /// Bound concurrent setup/teardown rooms, rather than spawning per user.
    #[arg(long, default_value_t = 64)]
    concurrency: usize,
    #[arg(long, default_value_t = 64)]
    source_addresses: usize,
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    /// Refill the same relay after complete retirement, preserving exact totals.
    #[arg(long, default_value_t = 1)]
    waves: usize,
    #[arg(long, default_value_t = 1024)]
    payload_bytes: usize,
    /// Each selected user sends one private message per second.
    #[arg(long, default_value_t = 100)]
    active_percent: usize,
    #[arg(long, default_value_t = 1)]
    messages_per_second: u64,
    /// Additional small rooms complete a join/ring/Close/EOF cycle every 250ms.
    #[arg(long, default_value_t = 0)]
    churn_workers: usize,
    #[arg(long, default_value_t = 8)]
    client_workers: usize,
    /// Diagnostic only: exact per-user send-to-receive and schedule-to-receive samples.
    #[arg(long)]
    user_latency_output: Option<std::path::PathBuf>,
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
    routing_checks: u64,
    departure_checks: u64,
    churn_cycles: u64,
}

impl Counts {
    fn merge(&mut self, rhs: &Self) {
        self.joins += rhs.joins;
        self.normal_closes += rhs.normal_closes;
        self.sent_messages += rhs.sent_messages;
        self.sent_bytes += rhs.sent_bytes;
        self.deliveries += rhs.deliveries;
        self.delivered_bytes += rhs.delivered_bytes;
        self.value_bytes += rhs.value_bytes;
        self.routing_checks += rhs.routing_checks;
        self.departure_checks += rhs.departure_checks;
        self.churn_cycles += rhs.churn_cycles;
    }
    fn received(&mut self, bytes: &Bytes) {
        self.deliveries += 1;
        self.delivered_bytes += bytes.len() as u64;
    }
}

struct Histogram {
    buckets: [u64; 64],
    n: u64,
    maximum: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Self {
            buckets: [0; 64],
            n: 0,
            maximum: 0,
        }
    }
}

impl Histogram {
    fn record(&mut self, duration: Duration) {
        let ns = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.buckets[ns.checked_ilog2().unwrap_or(0) as usize] += 1;
        self.maximum = self.maximum.max(ns);
        self.n += 1;
    }
    fn merge(&mut self, rhs: &Self) {
        for (a, b) in self.buckets.iter_mut().zip(rhs.buckets) {
            *a += b;
        }
        self.n += rhs.n;
        self.maximum = self.maximum.max(rhs.maximum);
    }
    fn report(&self) -> serde_json::Value {
        let percentile = |p: u64| {
            if self.n == 0 {
                return 0.0;
            }
            let rank = (self.n * p).div_ceil(100);
            let mut count = 0;
            for (bucket, n) in self.buckets.iter().enumerate() {
                count += n;
                if count >= rank {
                    return (1u128 << (bucket + 1)) as f64 / 1e6;
                }
            }
            unreachable!()
        };
        serde_json::json!({"samples":self.n,"p50_upper_ms":percentile(50),
            "p99_upper_ms":percentile(99),"max_ms":self.maximum as f64/1e6})
    }
}

struct Peer {
    socket: Socket,
    id: u64,
    latency: Option<Vec<(u64, u64)>>,
}
struct Room {
    index: usize,
    peers: Vec<Peer>,
}
#[derive(Default)]
struct Observations {
    counts: Counts,
    round_trip: Histogram,
    schedule_lag: Histogram,
    lifecycle: Histogram,
    scheduled: Option<Instant>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        (2..=200_000).contains(&args.users),
        "users must be in 2..=200000"
    );
    ensure!(
        (2..=64).contains(&args.room_size),
        "room-size must be in 2..=64"
    );
    ensure!(
        args.users.is_multiple_of(args.room_size),
        "users must be divisible by room-size"
    );
    ensure!(
        (1..=256).contains(&args.concurrency),
        "concurrency must be in 1..=256"
    );
    ensure!(
        (1..=4096).contains(&args.source_addresses),
        "source-addresses must be in 1..=4096"
    );
    ensure!(
        (1..=3600).contains(&args.seconds),
        "seconds must be in 1..=3600"
    );
    ensure!((1..=16).contains(&args.waves), "waves must be in 1..=16");
    ensure!(
        (16..=65520).contains(&args.payload_bytes),
        "payload-bytes must be in 16..=65520 (including envelope headroom)"
    );
    ensure!(
        args.active_percent <= 100,
        "active-percent must be in 0..=100"
    );
    ensure!(
        (1..=100).contains(&args.messages_per_second),
        "messages-per-second must be in 1..=100"
    );
    ensure!(
        args.churn_workers <= 128,
        "churn-workers must be in 0..=128"
    );
    ensure!(
        (1..=32).contains(&args.client_workers),
        "client-workers must be in 1..=32"
    );
    let authority = args
        .url
        .strip_prefix("ws://")
        .context("requires ws:// IPv4 loopback URL")?;
    let destination: SocketAddr = authority
        .parse()
        .context("requires ws://IPv4:port without a path")?;
    ensure!(
        destination.is_ipv4() && destination.ip().is_loopback(),
        "requires IPv4 loopback"
    );
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.client_workers)
        .enable_all()
        .build()?
        .block_on(run(args, destination))
}

fn gate(event: &str, counts: &Counts, details: serde_json::Value, wave: usize) -> Result<()> {
    println!(
        "{}",
        serde_json::json!({"event":event,"counts":counts,"details":details,"wave":wave})
    );
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    ensure!(input == "continue\n", "expected controller continuation");
    Ok(())
}

async fn data(socket: &mut Socket) -> Result<Bytes> {
    loop {
        match timeout(DEADLINE, socket.next())
            .await?
            .context("missing application data")??
        {
            Message::Binary(bytes) => return Ok(bytes),
            Message::Ping(_) => socket.flush().await?,
            other => anyhow::bail!("unexpected message: {other:?}"),
        }
    }
}

async fn join_room(index: usize, args: &Args, destination: SocketAddr) -> Result<(Room, Counts)> {
    let name = protocol::text(&format!("cap{index:x}"));
    let mut peers = Vec::with_capacity(args.room_size);
    let mut counts = Counts::default();
    for member in 0..args.room_size {
        let source = 1 + (index * args.room_size + member) % args.source_addresses;
        let source = Ipv4Addr::new(127, 80, (source >> 8) as u8, source as u8);
        let socket = TcpSocket::new_v4()?;
        socket.bind((source, 0).into())?;
        let stream = timeout(DEADLINE, socket.connect(destination)).await??;
        stream.set_nodelay(true)?;
        let (mut socket, _) = timeout(
            DEADLINE,
            client_async_with_config(
                &args.url,
                stream,
                Some(
                    WebSocketConfig::default()
                        .read_buffer_size(256)
                        .write_buffer_size(0),
                ),
            ),
        )
        .await??;
        socket.send(Message::Binary(name.clone())).await?;
        let welcome = data(&mut socket).await?;
        let mut decoder = Decoder::new(&welcome);
        let id = decoder.u64()?;
        ensure!(
            decoder.position() == welcome.len() && id != 0 && id != protocol::BROADCAST,
            "invalid welcome in room {index}"
        );
        counts.joins += 1;
        counts.received(&welcome);
        peers.push(Peer {
            socket,
            id,
            latency: None,
        });
    }
    Ok((Room { index, peers }, counts))
}

/// Every socket sends to the next member and receives from the previous one.
/// Complete bytes include independent sender, room, direction and sequence data.
async fn ring(
    room: &mut Room,
    sequence: u64,
    payload_bytes: usize,
    observations: &mut Observations,
) -> Result<()> {
    let mut values = Vec::with_capacity(room.peers.len());
    for peer in &room.peers {
        let mut payload = vec![0x5a; payload_bytes];
        payload[..8].copy_from_slice(&peer.id.to_le_bytes());
        payload[8..16].copy_from_slice(&sequence.to_le_bytes());
        values.push(protocol::byte_string(&payload));
    }
    let diagnostic = room.peers[0].latency.is_some();
    let ids: Vec<_> = if diagnostic {
        room.peers.iter().map(|p| p.id).collect()
    } else {
        Vec::new()
    };
    let started = Instant::now();
    let mut sent_at = if diagnostic {
        Vec::with_capacity(room.peers.len())
    } else {
        Vec::new()
    };
    for (member, value) in values.iter().enumerate() {
        let next = (member + 1) % room.peers.len();
        let packet = protocol::envelope(room.peers[next].id, value);
        observations.counts.sent_messages += 1;
        observations.counts.sent_bytes += packet.len() as u64;
        observations.counts.value_bytes += value.len() as u64;
        if diagnostic {
            sent_at.push(Instant::now());
        }
        room.peers[member]
            .socket
            .send(Message::Binary(packet))
            .await?;
    }
    if room.peers[0].latency.is_some() {
        let mut receives = futures_util::stream::FuturesUnordered::new();
        for (member, peer) in room.peers.iter_mut().enumerate() {
            receives.push(async move {
                let packet = data(&mut peer.socket).await?;
                Ok::<_, anyhow::Error>((member, packet, Instant::now(), &mut peer.latency))
            });
        }
        while let Some(received) = receives.next().await {
            let (member, packet, received, samples) = received?;
            let previous = (member + values.len() - 1) % values.len();
            let latency = received.duration_since(sent_at[previous]);
            check_packet(&packet, ids[previous], &values[previous])?;
            samples.as_mut().expect("diagnostic samples").push((
                latency.as_nanos() as u64,
                received
                    .duration_since(observations.scheduled.unwrap_or(started))
                    .as_nanos() as u64,
            ));
            observations.round_trip.record(latency);
            observations.counts.received(&packet);
            observations.counts.routing_checks += 1;
        }
        return Ok(());
    }
    for member in 0..room.peers.len() {
        let previous = (member + room.peers.len() - 1) % room.peers.len();
        let expected_id = room.peers[previous].id;
        let packet = data(&mut room.peers[member].socket).await?;
        check_packet(&packet, expected_id, &values[previous])
            .with_context(|| format!("ring room {} sequence {sequence}", room.index))?;
        observations.round_trip.record(started.elapsed());
        observations.counts.received(&packet);
        observations.counts.routing_checks += 1;
    }
    Ok(())
}

fn check_packet(packet: &[u8], expected_id: u64, expected_value: &[u8]) -> Result<()> {
    let (sender, value) =
        protocol::parse(packet).map_err(|e| anyhow::anyhow!("invalid routed CBOR: {e:?}"))?;
    ensure!(
        sender == expected_id && value == expected_value,
        "sender/body/FIFO mismatch"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_oracle_rejects_wrong_sender_corruption_sequence_and_trailing_data() {
        let mut payload = vec![0x5a; 1024];
        payload[..8].copy_from_slice(&42u64.to_le_bytes());
        payload[8..16].copy_from_slice(&7u64.to_le_bytes());
        let expected = protocol::byte_string(&payload);
        let valid = protocol::envelope(42, &expected);
        check_packet(&valid, 42, &expected).unwrap();
        assert!(check_packet(&valid, 43, &expected).is_err());
        for position in [0, 8, 512, 1023] {
            let mut corrupt = payload.clone();
            corrupt[position] ^= 1;
            let packet = protocol::envelope(42, &protocol::byte_string(&corrupt));
            assert!(check_packet(&packet, 42, &expected).is_err());
        }
        let mut trailing = valid.to_vec();
        trailing.push(0xf6);
        assert!(check_packet(&trailing, 42, &expected).is_err());
        assert!(check_packet(&valid[..valid.len() - 1], 42, &expected).is_err());
    }
}

async fn retire(mut room: Room) -> Result<Counts> {
    let mut counts = Counts::default();
    // Retire backwards so each still-live member can verify every departure.
    while let Some(mut peer) = room.peers.pop() {
        let close = CloseFrame {
            code: 1000.into(),
            reason: "capacity".into(),
        };
        peer.socket
            .send(Message::Close(Some(close.clone())))
            .await?;
        loop {
            match timeout(DEADLINE, peer.socket.next())
                .await?
                .context("missing Close acknowledgement")??
            {
                Message::Close(Some(frame)) if frame == close => break,
                Message::Ping(_) => peer.socket.flush().await?,
                other => anyhow::bail!("Close mismatch: {other:?}"),
            }
        }
        peer.socket.flush().await?;
        ensure!(
            timeout(DEADLINE, peer.socket.next()).await?.is_none(),
            "missing transport EOF"
        );
        counts.normal_closes += 1;
        let expected = protocol::envelope(protocol::BROADCAST, &protocol::uint(peer.id));
        for remaining in &mut room.peers {
            let packet = data(&mut remaining.socket).await?;
            ensure!(
                packet == expected,
                "departure mismatch in room {}",
                room.index
            );
            counts.received(&packet);
            counts.departure_checks += 1;
        }
    }
    Ok(counts)
}

fn merge(total: &mut Observations, part: Observations) {
    total.counts.merge(&part.counts);
    total.round_trip.merge(&part.round_trip);
    total.schedule_lag.merge(&part.schedule_lag);
    total.lifecycle.merge(&part.lifecycle);
}

/// Poll every inactive socket so the server's real heartbeat policy stays active.
async fn wait_idle(room: &mut Room, until: Instant) -> Result<()> {
    while Instant::now() < until {
        let next = async {
            let futures = room
                .peers
                .iter_mut()
                .map(|peer| Box::pin(peer.socket.next()));
            let (message, member, pending) = futures_util::future::select_all(futures).await;
            drop(pending);
            (message, member)
        };
        let (message, member) = tokio::select! {
            _ = sleep_until(until.into()) => return Ok(()),
            result = next => result,
        };
        match message.context("idle socket ended")?? {
            Message::Ping(_) => room.peers[member].socket.flush().await?,
            other => anyhow::bail!("unexpected idle message: {other:?}"),
        }
    }
    Ok(())
}

async fn churn(
    worker: usize,
    args: std::sync::Arc<Args>,
    destination: SocketAddr,
    start: Instant,
    audit: std::sync::Arc<parking_lot::Mutex<HashSet<u64>>>,
) -> Result<Observations> {
    let until = start + Duration::from_secs(args.seconds);
    let mut observations = Observations::default();
    let mut sequence = 0;
    let mut due =
        start + Duration::from_micros(worker as u64 * 250_000 / args.churn_workers as u64);
    while due < until {
        sleep_until(due.into()).await;
        if Instant::now() >= until {
            break;
        }
        let began = Instant::now();
        let (mut room, counts) =
            join_room(args.users / args.room_size + worker, &args, destination).await?;
        observations.counts.merge(&counts);
        let ids: Vec<_> = room.peers.iter().map(|peer| peer.id).collect();
        {
            let mut live = audit.lock();
            for id in &ids {
                ensure!(live.insert(*id), "repeated live ID during churn");
            }
        }
        sequence += 1;
        ring(&mut room, sequence, args.payload_bytes, &mut observations).await?;
        observations.counts.merge(&retire(room).await?);
        {
            let mut live = audit.lock();
            for id in ids {
                ensure!(live.remove(&id), "missing live ID after churn");
            }
        }
        observations.counts.churn_cycles += 1;
        observations.lifecycle.record(began.elapsed());
        // Skip missed deadlines rather than replaying a burst of reconnects.
        due += Duration::from_millis(250);
        if due < Instant::now() {
            due = Instant::now() + Duration::from_millis(250);
        }
    }
    Ok(observations)
}

async fn prove_rooms(
    rooms: Vec<Room>,
    sequence: u64,
    payload_bytes: usize,
    concurrency: usize,
) -> Result<(Vec<Room>, Observations)> {
    let mut jobs = JoinSet::new();
    let mut checked = Vec::with_capacity(rooms.len());
    let mut total = Observations::default();
    for mut room in rooms {
        jobs.spawn(async move {
            let mut observations = Observations::default();
            ring(&mut room, sequence, payload_bytes, &mut observations).await?;
            Ok::<_, anyhow::Error>((room, observations))
        });
        if jobs.len() == concurrency {
            let (room, observations) = jobs.join_next().await.context("missing proof job")???;
            merge(&mut total, observations);
            checked.push(room);
        }
    }
    while let Some(job) = jobs.join_next().await {
        let (room, observations) = job??;
        merge(&mut total, observations);
        checked.push(room);
    }
    Ok((checked, total))
}

async fn run(args: Args, destination: SocketAddr) -> Result<()> {
    let args = std::sync::Arc::new(args);
    let mut cumulative = Counts::default();
    for wave in 1..=args.waves {
        cumulative = run_wave(args.clone(), destination, cumulative, wave).await?;
    }
    Ok(())
}

async fn run_wave(
    args: std::sync::Arc<Args>,
    destination: SocketAddr,
    cumulative: Counts,
    wave: usize,
) -> Result<Counts> {
    let room_count = args.users / args.room_size;
    let mut rooms = Vec::with_capacity(room_count);
    let mut jobs = JoinSet::new();
    let mut total = Observations {
        counts: cumulative,
        ..Observations::default()
    };
    let started = Instant::now();
    for index in 0..room_count {
        let setup_args = args.clone();
        jobs.spawn(async move { join_room(index, &setup_args, destination).await });
        if jobs.len() == args.concurrency {
            let (room, counts) = jobs
                .join_next()
                .await
                .context("missing setup job")??
                .context("joining room")?;
            total.counts.merge(&counts);
            rooms.push(room);
        }
    }
    while let Some(job) = jobs.join_next().await {
        let (room, counts) = job??;
        total.counts.merge(&counts);
        rooms.push(room);
    }
    let mut ids = HashSet::with_capacity(args.users);
    for room in &rooms {
        for peer in &room.peers {
            ensure!(ids.insert(peer.id), "repeated live welcome ID");
        }
    }
    ensure!(ids.len() == args.users, "population mismatch");
    gate(
        "connected",
        &total.counts,
        serde_json::json!({"users":ids.len(),"rooms":room_count,
        "setup_seconds":started.elapsed().as_secs_f64()}),
        wave,
    )?;
    let audit = std::sync::Arc::new(parking_lot::Mutex::new(ids));

    // Prove every socket usable before the timed workload, independent of activity.
    let (checked, proof) = prove_rooms(rooms, 0, args.payload_bytes, args.concurrency).await?;
    merge(&mut total, proof);
    gate(
        "ready",
        &total.counts,
        serde_json::json!({"users":args.users,"rooms":room_count,
        "all_connections_routed":true,"proof_round_trip":total.round_trip.report()}),
        wave,
    )?;

    let mut jobs = JoinSet::new();
    let start = Instant::now() + Duration::from_millis(250);
    let until = start + Duration::from_secs(args.seconds);
    let mut churn_jobs = JoinSet::new();
    for worker in 0..args.churn_workers {
        churn_jobs.spawn(churn(
            worker,
            args.clone(),
            destination,
            start,
            audit.clone(),
        ));
    }
    let active_rooms = (room_count * args.active_percent).div_ceil(100);
    for mut room in checked {
        let args = args.clone();
        jobs.spawn(async move {
            let mut observations = Observations::default();
            if room.index < active_rooms {
                if args.user_latency_output.is_some() {
                    for peer in &mut room.peers {
                        peer.latency = Some(Vec::with_capacity(
                            (args.seconds * args.messages_per_second) as usize,
                        ));
                    }
                }
                // Spread rooms over each period to avoid a generator-made burst.
                let period_ns = 1_000_000_000 / args.messages_per_second;
                let phase =
                    Duration::from_nanos(room.index as u64 * period_ns / active_rooms as u64);
                for sequence in 1..=args.seconds * args.messages_per_second {
                    let due = start + phase + Duration::from_nanos((sequence - 1) * period_ns);
                    sleep_until(due.into()).await;
                    observations
                        .schedule_lag
                        .record(Instant::now().saturating_duration_since(due));
                    observations.scheduled = Some(due);
                    ring(&mut room, sequence, args.payload_bytes, &mut observations).await?;
                }
            }
            wait_idle(&mut room, until).await?;
            Ok::<_, anyhow::Error>((room, observations))
        });
    }
    let mut rooms = Vec::with_capacity(room_count);
    let mut measured = Observations::default();
    while let Some(job) = jobs.join_next().await {
        let (room, observations) = job??;
        merge(&mut measured, observations);
        rooms.push(room);
    }
    let mut churn = Observations::default();
    while let Some(job) = churn_jobs.join_next().await {
        merge(&mut churn, job??);
    }
    measured.counts.merge(&churn.counts);
    ensure!(
        audit.lock().len() == args.users,
        "churn did not retire every temporary session"
    );
    total.counts.merge(&measured.counts);
    gate(
        "measured",
        &total.counts,
        serde_json::json!({"active_users":active_rooms*args.room_size,
        "scheduled_seconds":args.seconds,"elapsed_seconds":start.elapsed().as_secs_f64(),
        "counts":measured.counts,"round_trip":measured.round_trip.report(),
        "churn_lifecycle":churn.lifecycle.report(),"churn_round_trip":churn.round_trip.report(),
        "schedule_lag":measured.schedule_lag.report()}),
        wave,
    )?;

    if let Some(path) = &args.user_latency_output {
        use std::io::BufWriter;
        ensure!(args.waves == 1, "per-user diagnostic requires one wave");
        let mut output = BufWriter::new(std::fs::File::create(path)?);
        writeln!(
            output,
            "room,member,id,samples,p50_ms,p95_ms,p99_ms,max_ms,schedule_p99_ms,schedule_max_ms"
        )?;
        for room in &mut rooms {
            for (member, peer) in room.peers.iter_mut().enumerate() {
                let Some(samples) = peer.latency.take() else {
                    continue;
                };
                let mut latency: Vec<_> = samples.iter().map(|s| s.0).collect();
                let mut scheduled: Vec<_> = samples.iter().map(|s| s.1).collect();
                latency.sort_unstable();
                scheduled.sort_unstable();
                let percentile = |values: &[u64], p: usize| {
                    values[(values.len() * p).div_ceil(100) - 1] as f64 / 1e6
                };
                writeln!(
                    output,
                    "{},{},{},{},{},{},{},{},{},{}",
                    room.index,
                    member,
                    peer.id,
                    latency.len(),
                    percentile(&latency, 50),
                    percentile(&latency, 95),
                    percentile(&latency, 99),
                    percentile(&latency, 100),
                    percentile(&scheduled, 99),
                    percentile(&scheduled, 100)
                )?;
            }
        }
        output.flush()?;
    }

    // Idle heartbeat responses alone do not prove application routing survived.
    let (rooms, proof) = prove_rooms(
        rooms,
        args.seconds * args.messages_per_second + 1,
        args.payload_bytes,
        args.concurrency,
    )
    .await?;
    let proof_latency = proof.round_trip.report();
    merge(&mut total, proof);
    gate(
        "postcheck",
        &total.counts,
        serde_json::json!({"users":args.users,
        "all_connections_routed_again":true,"proof_round_trip":proof_latency}),
        wave,
    )?;

    let retired = Instant::now();
    let mut jobs = JoinSet::new();
    for room in rooms {
        jobs.spawn(retire(room));
        if jobs.len() == args.concurrency {
            total.counts.merge(
                &jobs
                    .join_next()
                    .await
                    .context("missing retirement job")???,
            );
        }
    }
    while let Some(job) = jobs.join_next().await {
        total.counts.merge(&job??);
    }
    gate(
        "done",
        &total.counts,
        serde_json::json!({"retirement_seconds":retired.elapsed().as_secs_f64()}),
        wave,
    )?;
    Ok(total.counts)
}
