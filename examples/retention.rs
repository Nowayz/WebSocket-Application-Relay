//! Retain tiny output beside repeated large unknown-target input on real sockets.
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::Parser;
use futures_util::{SinkExt, StreamExt, future::try_join_all};
use tokio::{
    net::{TcpSocket, TcpStream},
    sync::Barrier,
    task::JoinSet,
    time::{sleep, timeout},
};
use tokio_tungstenite::{
    WebSocketStream, client_async_with_config,
    tungstenite::{Message, protocol::WebSocketConfig},
};
use war_relay::protocol;

type Socket = WebSocketStream<TcpStream>;
const FILLERS: usize = 8;
const FILLER_BYTES: usize = 1024 * 1024;

#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "127.0.0.1:1338")]
    address: SocketAddr,
    #[arg(long, default_value_t = 64)]
    cycles: usize,
    #[arg(long, default_value_t = 4 * 1024 * 1024)]
    primer_bytes: usize,
    #[arg(long, default_value_t = 1)]
    producers: usize,
    /// Observe connected idle storage after output drains; zero skips the phase.
    #[arg(long, default_value_t = 0)]
    idle_secs: u64,
    /// After the idle observation, require the server's application-idle Close.
    #[arg(long)]
    expect_idle_close: bool,
}

#[tokio::main(worker_threads = 2)]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        (16..=512).contains(&args.cycles),
        "cycles must be in 16..=512"
    );
    ensure!(
        (64 * 1024..=8 * 1024 * 1024).contains(&args.primer_bytes),
        "primer must be in 64 KiB..=8 MiB"
    );
    ensure!(
        (1..=64).contains(&args.producers),
        "producers must be in 1..=64"
    );
    ensure!(
        args.idle_secs <= 10,
        "idle observation cannot exceed 10 seconds"
    );
    ensure!(
        !args.expect_idle_close || args.idle_secs > 0,
        "idle Close verification requires an idle observation"
    );
    ensure!(
        args.producers as u128 * args.cycles as u128 * args.primer_bytes as u128
            <= 8 * 1024 * 1024 * 1024u128,
        "primer traffic cannot exceed 8 GiB; reduce producers, cycles, or primer bytes"
    );
    let mut sources = Vec::with_capacity(args.producers);
    let mut next_sequence = HashMap::with_capacity(args.producers);
    for index in 0..args.producers {
        // Separate source channels keep departure notifications out of the
        // data/order checks. Private routing intentionally spans channels.
        let (sender, sender_id) = join(args.address, &format!("retain-src{index:x}")).await?;
        ensure!(
            next_sequence.insert(sender_id, 0usize).is_none(),
            "duplicate producer ID"
        );
        sources.push((sender, sender_id));
    }
    let sender_id = sources[0].1;
    let (mut receiver, receiver_id) = join(args.address, "retain-dest").await?;
    let unknown = (1..)
        .find(|id| !next_sequence.contains_key(id) && *id != receiver_id)
        .unwrap();
    let primer = protocol::envelope(
        unknown,
        &protocol::byte_string(&vec![0x5a; args.primer_bytes]),
    );
    let fill_value = protocol::byte_string(&vec![0xa5; FILLER_BYTES]);
    let filler = protocol::envelope(receiver_id, &fill_value);
    let expected_filler = protocol::envelope(sender_id, &fill_value);
    let values: Vec<_> = (1..=args.cycles)
        .map(|sequence| protocol::uint(sequence as u64))
        .collect();
    let packets: Arc<[_]> = values
        .iter()
        .map(|value| protocol::envelope(receiver_id, value))
        .collect::<Vec<_>>()
        .into();
    let value_bytes = FILLERS * fill_value.len()
        + 1
        + args.producers * values.iter().map(|v| v.len() * 2).sum::<usize>();
    phase("ready", 0);
    let sender = &mut sources[0].0;
    for _ in 0..FILLERS {
        sender.send(Message::Binary(filler.clone())).await?;
    }
    sender
        .send(Message::Binary(protocol::envelope(sender_id, &[0])))
        .await?;
    ensure!(
        data(sender).await? == protocol::envelope(sender_id, &[0]),
        "initial barrier changed"
    );
    phase("prefilled", 0);
    let start = Arc::new(Barrier::new(args.producers));
    let mut tasks = JoinSet::new();
    for (producer, (mut sender, id)) in sources.into_iter().enumerate() {
        let barriers: Vec<_> = values
            .iter()
            .map(|value| protocol::envelope(id, value))
            .collect();
        let packets = packets.clone();
        let primer = primer.clone();
        let start = start.clone();
        tasks.spawn(async move {
            start.wait().await;
            for (index, barrier) in barriers.iter().enumerate() {
                sender.send(Message::Binary(primer.clone())).await?;
                sender.send(Message::Binary(packets[index].clone())).await?;
                sender.send(Message::Binary(barrier.clone())).await?;
                // The self-message proves this connection processed preceding
                // input while every producer feeds the same unread recipient.
                ensure!(
                    data(&mut sender).await? == *barrier,
                    "producer {producer} barrier changed at {index}"
                );
                if (index + 1).is_multiple_of(16) {
                    println!(
                        "{}",
                        serde_json::json!({"event":"priming","producer":producer,"cycles":index+1})
                    );
                }
            }
            Ok::<_, anyhow::Error>(sender)
        });
    }
    let mut senders = Vec::with_capacity(args.producers);
    while let Some(completed) = tasks.join_next().await {
        senders.push(completed??);
    }
    phase("holding", args.cycles);
    sleep(Duration::from_secs(2)).await;
    phase("draining", args.cycles);
    for _ in 0..FILLERS {
        ensure!(
            data(&mut receiver).await? == expected_filler,
            "filler changed"
        );
    }
    for _ in 0..args.producers * args.cycles {
        let packet = data(&mut receiver).await?;
        let (id, value) = protocol::parse(&packet)
            .map_err(|e| anyhow::anyhow!("invalid received envelope: {e:?}"))?;
        let next = next_sequence.get_mut(&id).context("unknown producer")?;
        ensure!(
            *next < args.cycles && value == values[*next],
            "producer {id} output is duplicated, missing, or out of order"
        );
        *next += 1;
    }
    ensure!(
        next_sequence.values().all(|next| *next == args.cycles),
        "producer matrix incomplete"
    );
    if args.idle_secs > 0 {
        phase("idle", args.cycles);
        sleep(Duration::from_secs(args.idle_secs)).await;
    }
    senders.push(receiver);
    if args.expect_idle_close {
        phase("evicting", args.cycles);
        try_join_all(senders.iter_mut().map(idle_close)).await?;
    } else {
        for sender in &mut senders {
            close(sender).await?;
        }
    }
    if args.idle_secs > 0 {
        phase("retired", args.cycles);
        sleep(Duration::from_secs(2)).await;
    }
    println!(
        "{}",
        serde_json::json!({
            "event":"result", "cycles":args.cycles, "primer_bytes":args.primer_bytes,
            "producers":args.producers,"joins":args.producers+1,"idle_secs":args.idle_secs,
            "server_idle_close_verified":args.expect_idle_close,
            "unknown_input_bytes":args.producers as u64 * args.cycles as u64 * primer.len() as u64,
            "incoming_messages":FILLERS + args.producers * args.cycles * 3 + 1,
            "verified_deliveries":FILLERS + args.producers * args.cycles * 2 + 1, "routed_value_bytes":value_bytes,
            "receiver_buffer_requested_bytes":65536, "filler_messages":FILLERS,
            "filler_value_bytes":fill_value.len(), "clean_close_handshakes":args.producers+1,
        })
    );
    Ok(())
}

fn phase(event: &str, cycles: usize) {
    println!("{}", serde_json::json!({"event":event,"cycles":cycles}));
}

async fn join(address: SocketAddr, channel: &str) -> Result<(Socket, u64)> {
    let socket = if address.is_ipv4() {
        TcpSocket::new_v4()?
    } else {
        TcpSocket::new_v6()?
    };
    // Bound the unread peer's kernel receive window as well as its application
    // reads; eight MiB of output then reliably reaches transport backpressure.
    socket.set_recv_buffer_size(65536)?;
    let stream = socket.connect(address).await?;
    stream.set_nodelay(true)?;
    let config = WebSocketConfig::default().read_buffer_size(4096);
    let (mut ws, _) =
        client_async_with_config(format!("ws://{address}"), stream, Some(config)).await?;
    ws.send(Message::Binary(protocol::text(channel))).await?;
    let id = minicbor::Decoder::new(&data(&mut ws).await?).u64()?;
    Ok((ws, id))
}

async fn data(ws: &mut Socket) -> Result<Bytes> {
    timeout(Duration::from_secs(10), async {
        loop {
            match ws.next().await.context("unexpected EOF")?? {
                Message::Binary(bytes) => return Ok(bytes),
                Message::Ping(_) => ws.flush().await?,
                other => anyhow::bail!("unexpected frame: {other:?}"),
            }
        }
    })
    .await?
}

async fn close(ws: &mut Socket) -> Result<()> {
    timeout(Duration::from_secs(3), async {
        ws.close(None).await?;
        while let Some(frame) = ws.next().await {
            match frame? {
                Message::Close(None) => {
                    ws.flush().await?;
                    return Ok(());
                }
                Message::Ping(_) => ws.flush().await?,
                other => anyhow::bail!("unexpected closing frame: {other:?}"),
            }
        }
        anyhow::bail!("Close acknowledgement missing")
    })
    .await?
}

async fn idle_close(ws: &mut Socket) -> Result<()> {
    timeout(Duration::from_secs(12), async {
        loop {
            match ws.next().await.context("Close acknowledgement missing")?? {
                Message::Close(Some(frame))
                    if u16::from(frame.code) == 1001 && frame.reason == "Application Idle" =>
                {
                    ws.flush().await?;
                    return Ok(());
                }
                Message::Ping(_) => ws.flush().await?,
                other => anyhow::bail!("unexpected idle-policy frame: {other:?}"),
            }
        }
    })
    .await?
}
