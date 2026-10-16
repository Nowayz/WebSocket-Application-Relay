//! Verify routing before/after a transient population in separate or shared rooms.
//! Every reported event waits for the harness to sample and send `continue`.
//! This blocking control gate is outside timed traffic, on the CLI main thread.
use std::{
    io::{self, Write},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::{Parser, ValueEnum};
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

#[derive(Clone, Copy, ValueEnum)]
enum Scope {
    Global,
    Room,
    Private,
    Channels,
}

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
    /// Global broadcast with separate rooms, ordinary room broadcast, or private
    /// delivery through a populated global-room listener index. Channels queries
    /// inspect a retained channel-name table after separate rooms depart.
    #[arg(long, value_enum, default_value = "global")]
    scope: Scope,
    /// Also measure private delivery while the global-room population is live.
    #[arg(long)]
    measure_populated: bool,
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
        !args.measure_populated || matches!(args.scope, Scope::Private),
        "measure-populated requires private scope"
    );
    ensure!(
        (8..=65536).contains(&args.payload_bytes),
        "payload-bytes must be in 8..=65536"
    );
    let same_room = matches!(args.scope, Scope::Room | Scope::Private);
    let queries = matches!(args.scope, Scope::Channels);
    let room = if matches!(args.scope, Scope::Room) {
        "peak-room"
    } else {
        "re_globl"
    };
    let (mut sender, id, first_welcome) = join(&args.url, room).await?;
    let (mut receiver, receiver_id, second_welcome) = join(&args.url, room).await?;
    let target = if matches!(args.scope, Scope::Private) {
        receiver_id
    } else {
        protocol::BROADCAST
    };
    let mut welcome_bytes = first_welcome + second_welcome;
    let mut payload = vec![0x5a; args.payload_bytes];
    let mut packets: Vec<_> = (0u64..64)
        .map(|i| {
            payload[..8].copy_from_slice(&i.to_le_bytes());
            let value = protocol::byte_string(&payload);
            (
                protocol::envelope(target, &value),
                protocol::envelope(id, &value),
            )
        })
        .collect();
    let mut setup_reply_bytes = 0;
    let mut setup_request_bytes = 0;
    if queries {
        let request = protocol::request(0, &protocol::text("registry-peak-benchmark"));
        setup_request_bytes = request.len();
        sender.send(Message::Binary(request)).await?;
        let reply = data(&mut sender).await?;
        ensure!(
            reply.as_ref() == b"\x82\x00\x82\x00\xf5",
            "authentication failed"
        );
        setup_reply_bytes = reply.len();
        // Independent known bytes: [0, [2, [[h'72655f676c6f626c', 2]]]].
        packets = vec![(
            Bytes::from_static(b"\x82\x00\x82\x02\xf6"),
            Bytes::from_static(b"\x82\x00\x82\x02\x81\x82\x48re_globl\x02"),
        )];
    }
    gate("traffic_ready", 0, serde_json::json!({"sessions":2}))?;
    gate(
        "traffic_done",
        0,
        traffic(&mut sender, &mut receiver, &packets, args.seconds, queries).await?,
    )?;
    let mut fillers = Vec::with_capacity(args.peak_clients);
    let mut departure_deliveries = 0u64;
    let mut departure_bytes = 0u64;
    for wave in 1..=args.waves {
        for i in 0..args.peak_clients {
            let channel = if same_room {
                room.into()
            } else {
                format!("peak{i:x}")
            };
            let (socket, id, bytes) = join(&args.url, &channel).await?;
            welcome_bytes += bytes;
            fillers.push((socket, id));
        }
        gate(
            "population_ready",
            wave,
            serde_json::json!({"sessions":args.peak_clients+2,"channels":match args.scope {Scope::Global | Scope::Channels =>args.peak_clients+1, Scope::Room=>2, Scope::Private=>1}}),
        )?;
        if args.measure_populated {
            gate(
                "population_traffic_ready",
                wave,
                serde_json::json!({"sessions":args.peak_clients+2}),
            )?;
            gate(
                "population_traffic_done",
                wave,
                traffic(&mut sender, &mut receiver, &packets, args.seconds, false).await?,
            )?;
        }
        if same_room {
            let (next_sender, next_receiver, deliveries, bytes) =
                depart_room(sender, receiver, &mut fillers).await?;
            sender = next_sender;
            receiver = next_receiver;
            departure_deliveries += deliveries;
            departure_bytes += bytes;
        } else {
            for (mut socket, _) in fillers.drain(..) {
                close(&mut socket).await?;
            }
        }
        gate("traffic_ready", wave, serde_json::json!({"sessions":2}))?;
        gate(
            "traffic_done",
            wave,
            traffic(&mut sender, &mut receiver, &packets, args.seconds, queries).await?,
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
        serde_json::json!({"joins":args.peak_clients*args.waves+2,"normal_closes":args.peak_clients*args.waves+2,"welcome_bytes":welcome_bytes,"setup_deliveries":usize::from(queries),"setup_request_bytes":setup_request_bytes,"setup_reply_bytes":setup_reply_bytes,"disconnect_bytes":disconnect.len(),"disconnects":1,"departure_deliveries":departure_deliveries,"departure_bytes":departure_bytes}),
    )?;
    Ok(())
}

async fn depart_room(
    sender: Socket,
    receiver: Socket,
    fillers: &mut Vec<(Socket, u64)>,
) -> Result<(Socket, Socket, u64, u64)> {
    let count = fillers.len();
    let departures: Arc<[Bytes]> = fillers
        .iter()
        .map(|(_, id)| protocol::envelope(protocol::BROADCAST, &protocol::uint(*id)))
        .collect();
    let progress: Arc<[AtomicUsize]> = (0..count + 2).map(|_| AtomicUsize::new(0)).collect();
    let (allowed, _) = tokio::sync::watch::channel(0usize);
    let mut tasks = Vec::with_capacity(count);
    let source = tokio::spawn(read_departures(
        sender,
        departures.clone(),
        progress.clone(),
        count,
        count,
    ));
    let target = tokio::spawn(read_departures(
        receiver,
        departures.clone(),
        progress.clone(),
        count + 1,
        count,
    ));
    for (index, (socket, _)) in fillers.drain(..).enumerate() {
        let departures = departures.clone();
        let progress = progress.clone();
        let mut allowed = allowed.subscribe();
        tasks.push(tokio::spawn(async move {
            let mut socket = read_departures(socket, departures, progress, index, index).await?;
            loop {
                let ready = *allowed.borrow_and_update() > index;
                if ready {
                    break;
                }
                allowed.changed().await?;
            }
            close(&mut socket).await
        }));
    }
    // Pace 16 departures at a time. Every remaining peer must read the complete
    // prefix before more closes start, keeping default 64-message queues safe.
    // These shared counters/encoded events are allocated once, outside traffic.
    for start in (0..count).step_by(16) {
        let end = (start + 16).min(count);
        allowed.send_replace(end);
        timeout(Duration::from_secs(10), async {
            while progress[end..]
                .iter()
                .any(|n| n.load(Ordering::Acquire) < end)
            {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .context("room departure window stalled")?;
    }
    for task in tasks {
        task.await??;
    }
    let deliveries = count as u64 * (count as u64 + 3) / 2;
    let bytes = departures
        .iter()
        .enumerate()
        .map(|(i, bytes)| (count - i + 1) as u64 * bytes.len() as u64)
        .sum();
    Ok((source.await??, target.await??, deliveries, bytes))
}

async fn read_departures(
    mut socket: Socket,
    expected: Arc<[Bytes]>,
    progress: Arc<[AtomicUsize]>,
    slot: usize,
    count: usize,
) -> Result<Socket> {
    for (index, bytes) in expected.iter().take(count).enumerate() {
        ensure!(
            data(&mut socket).await? == *bytes,
            "departure ID, order or bytes changed"
        );
        progress[slot].store(index + 1, Ordering::Release);
    }
    Ok(socket)
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
    queries: bool,
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
            let received = if queries {
                data(sender).await?
            } else {
                data(receiver).await?
            };
            ensure!(received == *expected, "payload, sender or FIFO changed");
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
        serde_json::json!({"sent":count,"verified":count,"elapsed_seconds":elapsed,"deliveries_per_second":count as f64/elapsed,"p99_upper_ms":(1u128<<(p99+1)) as f64/1e6,"max_latency_ms":maximum as f64/1e6,"sent_encoded_bytes":count*packets[0].0.len() as u64,"delivered_encoded_bytes":count*packets[0].1.len() as u64,"value_bytes":if queries {0} else {count*protocol::parse(&packets[0].0).unwrap().1.len() as u64}}),
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
