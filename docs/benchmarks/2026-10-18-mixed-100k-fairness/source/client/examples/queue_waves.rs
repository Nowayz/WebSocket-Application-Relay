//! Recover from repeated output bursts while keeping the same sockets connected.
use std::{
    collections::HashSet,
    io::{self, Read, Write},
    net::TcpStream,
    thread,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::Parser;
use serde::Serialize;
use tokio_tungstenite::tungstenite::{
    Error, Message, WebSocket, connect,
    protocol::{CloseFrame, frame::coding::CloseCode},
    stream::MaybeTlsStream,
};
use war_relay::protocol;

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    url: String,
    #[arg(long, default_value_t = 32)]
    receivers: usize,
    #[arg(long, default_value_t = 48)]
    messages: usize,
    #[arg(long, default_value_t = 3)]
    waves: usize,
    #[arg(long, default_value_t = 8 * 1024 * 1024)]
    payload_bytes: usize,
}

#[derive(Default, Serialize)]
struct Counts {
    joins: usize,
    normal_closes: usize,
    incoming_messages: usize,
    incoming_bytes: usize,
    deliveries: usize,
    delivered_bytes: usize,
    value_bytes: usize,
}

fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        (1..=64).contains(&args.receivers),
        "receivers must be 1..=64"
    );
    ensure!((1..=48).contains(&args.messages), "messages must be 1..=48");
    ensure!((1..=8).contains(&args.waves), "waves must be 1..=8");
    ensure!(
        (1024 * 1024..=8 * 1024 * 1024).contains(&args.payload_bytes),
        "payload must be 1..=8 MiB"
    );
    let mut counts = Counts::default();
    let (mut sender, sender_id) = join(&args.url, &mut counts)?;
    let mut ids = HashSet::from([sender_id]);
    let mut receivers = Vec::with_capacity(args.receivers);
    for _ in 0..args.receivers {
        let peer = join(&args.url, &mut counts)?;
        ensure!(ids.insert(peer.1), "duplicate welcome ID");
        receivers.push(peer);
    }
    gate("ready", 0, &counts, 0.0, 0.0)?;
    for wave in 0..args.waves {
        let value = protocol::byte_string(&vec![0xa0 + wave as u8; args.payload_bytes]);
        let packet = protocol::envelope(protocol::BROADCAST, &value);
        let expected = protocol::envelope(sender_id, &value);
        counts.value_bytes += value.len();
        drop(value);
        let mut packets = Vec::with_capacity(args.messages);
        let mut expected_small = Vec::with_capacity(args.messages);
        for index in 0..args.messages {
            let value = protocol::uint((wave * args.messages + index) as u64);
            counts.value_bytes += value.len();
            packets.push(protocol::envelope(protocol::BROADCAST, &value));
            expected_small.push(protocol::envelope(sender_id, &value));
        }
        let marker = protocol::uint((1u64 << 40) + wave as u64);
        let marker_packet = protocol::envelope(sender_id, &marker);
        counts.value_bytes += marker.len();
        counts.incoming_messages += 2 + packets.len();
        counts.incoming_bytes +=
            packet.len() + packets.iter().map(Bytes::len).sum::<usize>() + marker_packet.len();
        counts.deliveries += args.receivers * (1 + expected_small.len()) + 1;
        counts.delivered_bytes += args.receivers
            * (expected.len() + expected_small.iter().map(Bytes::len).sum::<usize>())
            + marker_packet.len();
        let start = Instant::now();
        sender.send(Message::Binary(packet))?;
        for packet in packets {
            sender.send(Message::Binary(packet))?;
        }
        sender.send(Message::Binary(marker_packet.clone()))?;
        ensure!(
            data(&mut sender)? == marker_packet,
            "publication marker changed"
        );
        let published_ms = start.elapsed().as_secs_f64() * 1000.0;
        gate("queued", wave, &counts, published_ms, 0.0)?;
        let start = Instant::now();
        thread::scope(|scope| -> Result<()> {
            let mut readers = Vec::with_capacity(receivers.len());
            for (socket, _) in &mut receivers {
                let expected = &expected;
                let expected_small = &expected_small;
                readers.push(scope.spawn(move || -> Result<()> {
                    ensure!(data(socket)? == *expected, "large frame changed");
                    for packet in expected_small {
                        ensure!(data(socket)? == *packet, "small frame/order changed");
                    }
                    Ok(())
                }));
            }
            for reader in readers {
                reader
                    .join()
                    .map_err(|_| anyhow::anyhow!("reader panicked"))??;
            }
            Ok(())
        })?;
        gate(
            "drained",
            wave,
            &counts,
            published_ms,
            start.elapsed().as_secs_f64() * 1000.0,
        )?;
    }
    gate("idle", args.waves, &counts, 0.0, 0.0)?;
    while let Some((mut socket, id)) = receivers.pop() {
        close(&mut socket)?;
        counts.normal_closes += 1;
        let expected = protocol::envelope(protocol::BROADCAST, &protocol::uint(id));
        ensure!(
            data(&mut sender)? == expected,
            "publisher departure changed"
        );
        for (socket, _) in &mut receivers {
            ensure!(data(socket)? == expected, "receiver departure changed");
        }
        counts.deliveries += 1 + receivers.len();
        counts.delivered_bytes += (1 + receivers.len()) * expected.len();
    }
    close(&mut sender)?;
    counts.normal_closes += 1;
    gate("done", args.waves, &counts, 0.0, 0.0)
}

fn join(url: &str, counts: &mut Counts) -> Result<(Socket, u64)> {
    let (mut socket, _) = connect(url)?;
    match socket.get_mut() {
        MaybeTlsStream::Plain(stream) => {
            stream.set_nodelay(true)?;
            stream.set_read_timeout(Some(Duration::from_secs(30)))?;
            stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        }
        _ => anyhow::bail!("queue probe requires plain loopback TCP"),
    }
    socket.send(Message::Binary(protocol::text("queue-waves")))?;
    let welcome = data(&mut socket)?;
    let id = minicbor::Decoder::new(&welcome).u64()?;
    ensure!(
        id != 0 && id != protocol::BROADCAST && welcome == protocol::uint(id),
        "invalid welcome"
    );
    counts.joins += 1;
    counts.deliveries += 1;
    counts.delivered_bytes += welcome.len();
    Ok((socket, id))
}

fn data(socket: &mut Socket) -> Result<Bytes> {
    match socket.read().context("missing data frame")? {
        Message::Binary(value) => Ok(value),
        other => anyhow::bail!("unexpected frame: {other:?}"),
    }
}

fn close(socket: &mut Socket) -> Result<()> {
    let frame = CloseFrame {
        code: CloseCode::Normal,
        reason: "queue-waves".into(),
    };
    socket.close(Some(frame.clone()))?;
    ensure!(
        socket.read()? == Message::Close(Some(frame)),
        "Close changed"
    );
    ensure!(
        matches!(socket.read(), Err(Error::ConnectionClosed)),
        "expected closed WebSocket"
    );
    let mut byte = [0];
    ensure!(socket.get_mut().read(&mut byte)? == 0, "expected TCP EOF");
    Ok(())
}

fn gate(event: &str, wave: usize, counts: &Counts, published_ms: f64, drain_ms: f64) -> Result<()> {
    println!(
        "{}",
        serde_json::json!({"event":event,"wave":wave,"counts":counts,"published_ms":published_ms,"drain_ms":drain_ms})
    );
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    ensure!(line == "continue\n", "expected harness continuation");
    Ok(())
}
