//! Retain tiny output beside repeated large unknown-target input on real sockets.
use std::{net::SocketAddr, time::Duration};

use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use tokio::{
    net::{TcpSocket, TcpStream},
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
    let (mut sender, sender_id) = join(args.address, "retain-source").await?;
    let (mut receiver, receiver_id) = join(args.address, "retain-dest").await?;
    let unknown = (1..)
        .find(|id| *id != sender_id && *id != receiver_id)
        .unwrap();
    let primer = protocol::envelope(
        unknown,
        &protocol::byte_string(&vec![0x5a; args.primer_bytes]),
    );
    let fill_value = protocol::byte_string(&vec![0xa5; FILLER_BYTES]);
    let filler = protocol::envelope(receiver_id, &fill_value);
    let expected_filler = protocol::envelope(sender_id, &fill_value);
    let templates: Vec<_> = (1..=args.cycles)
        .map(|sequence| {
            let value = protocol::uint(sequence as u64);
            (
                protocol::envelope(receiver_id, &value),
                protocol::envelope(sender_id, &value),
                protocol::envelope(sender_id, &value),
                value.len(),
            )
        })
        .collect();
    let value_bytes =
        FILLERS * fill_value.len() + 1 + templates.iter().map(|t| t.3 * 2).sum::<usize>();
    phase("ready", 0);
    for _ in 0..FILLERS {
        sender.send(Message::Binary(filler.clone())).await?;
    }
    sender
        .send(Message::Binary(protocol::envelope(sender_id, &[0])))
        .await?;
    ensure!(
        data(&mut sender).await? == protocol::envelope(sender_id, &[0]),
        "initial barrier changed"
    );
    phase("prefilled", 0);
    for (index, (small, expected, barrier, _)) in templates.iter().enumerate() {
        sender.send(Message::Binary(primer.clone())).await?;
        sender.send(Message::Binary(small.clone())).await?;
        sender.send(Message::Binary(barrier.clone())).await?;
        // This self-message proves the same connection processed its preceding
        // unknown primer and tiny routed value while the other peer stays unread.
        ensure!(
            data(&mut sender).await? == *expected,
            "sender barrier changed at {index}"
        );
        if (index + 1).is_multiple_of(16) {
            phase("priming", index + 1);
        }
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
    for (index, (_, expected, _, _)) in templates.iter().enumerate() {
        ensure!(
            data(&mut receiver).await? == *expected,
            "queued value changed at {index}"
        );
    }
    close(&mut sender).await?;
    close(&mut receiver).await?;
    println!(
        "{}",
        serde_json::json!({
            "event":"result", "cycles":args.cycles, "primer_bytes":args.primer_bytes,
            "unknown_input_bytes":args.cycles * primer.len(), "incoming_messages":FILLERS + args.cycles * 3 + 1,
            "verified_deliveries":FILLERS + args.cycles * 2 + 1, "routed_value_bytes":value_bytes,
            "receiver_buffer_requested_bytes":65536, "filler_messages":FILLERS,
            "filler_value_bytes":fill_value.len(), "clean_close_handshakes":2,
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
