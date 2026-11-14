//! Times one client sending a file to another across a separate relay process.
//!
//! The sender splits the file into chunks addressed to the receiver's session.
//! With `--window N` the receiver acknowledges every chunk and the sender keeps
//! at most N unacknowledged; `--window 0` sends without waiting.
use anyhow::{Result, ensure};
use clap::Parser;
use std::{sync::Arc, time::Instant};
use tokio::sync::Semaphore;
use war_relay::webtransport::{read_record, write_record};
use wtransport::{Connection, Endpoint, RecvStream, SendStream, endpoint::endpoint_side::Client};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    url: String,
    #[arg(long)]
    certificate: String,
    #[arg(long, default_value_t = 50_000_000)]
    file_bytes: usize,
    #[arg(long, default_value_t = 1024 * 1024)]
    chunk_bytes: usize,
    #[arg(long, default_value_t = 0)]
    window: usize,
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    let hash = wtransport::tls::Sha256Digest::from_str_fmt(
        &args.certificate,
        wtransport::tls::Sha256DigestFmt::DottedHex,
    )?;
    let endpoint = Endpoint::client(
        wtransport::ClientConfig::builder()
            .with_bind_default()
            .with_server_certificate_hashes([hash])
            .build(),
    )?;
    let (_a, mut a_send, mut a_recv, a_id) = join(&endpoint, &args.url, "sender").await?;
    let (_b, mut b_send, mut b_recv, b_id) = join(&endpoint, &args.url, "receiver").await?;
    let chunks = args.file_bytes.div_ceil(args.chunk_bytes);
    let max_record = args.chunk_bytes + 64;

    let window = Arc::new(Semaphore::new(if args.window == 0 {
        Semaphore::MAX_PERMITS
    } else {
        args.window
    }));
    let acks = if args.window == 0 {
        None
    } else {
        let window = window.clone();
        Some(tokio::spawn(async move {
            for _ in 0..chunks {
                read_record(&mut a_recv, 64).await?;
                window.add_permits(1);
            }
            Ok::<_, anyhow::Error>(())
        }))
    };
    let acknowledge = args.window != 0;
    let receiver = tokio::spawn(async move {
        let mut received = 0usize;
        let ack = packet(a_id, &[])?;
        for index in 0..chunks {
            let record = read_record(&mut b_recv, max_record).await?;
            let mut decoder = minicbor::Decoder::new(&record);
            decoder.array()?;
            ensure!(decoder.u64()? == a_id, "unexpected sender");
            let data = decoder.bytes()?;
            ensure!(
                data.last() == Some(&(index as u8)),
                "chunk {index} out of order"
            );
            received += data.len();
            if acknowledge {
                write_record(&mut b_send, &ack).await?;
            }
        }
        Ok::<_, anyhow::Error>((received, Instant::now()))
    });

    let start = Instant::now();
    let mut sent = 0;
    for index in 0..chunks {
        let len = args.chunk_bytes.min(args.file_bytes - sent);
        window.acquire().await?.forget();
        write_record(&mut a_send, &packet(b_id, &vec![index as u8; len])?).await?;
        sent += len;
    }
    let written = start.elapsed().as_secs_f64();
    let (received, end) = receiver.await??;
    if let Some(acks) = acks {
        acks.await??;
    }
    ensure!(received == args.file_bytes, "received {received} bytes");
    let seconds = end.duration_since(start).as_secs_f64();
    println!(
        "{}",
        serde_json::json!({
            "file_bytes": args.file_bytes,
            "chunk_bytes": args.chunk_bytes,
            "window": args.window,
            "sender_done_seconds": written,
            "seconds": seconds,
            "mb_per_second": args.file_bytes as f64 / seconds / 1e6,
        })
    );
    Ok(())
}

async fn join(
    endpoint: &Endpoint<Client>,
    url: &str,
    room: &str,
) -> Result<(Connection, SendStream, RecvStream, u64)> {
    let connection = endpoint.connect(url).await?;
    let (mut send, mut recv) = connection.open_bi().await?.await?;
    let mut join = Vec::new();
    minicbor::Encoder::new(&mut join).str(room)?;
    write_record(&mut send, &join).await?;
    let id = minicbor::Decoder::new(&read_record(&mut recv, 1024).await?).u64()?;
    Ok((connection, send, recv, id))
}

/// Encodes chunk bytes as a CBOR byte-string addressed to `target`.
fn packet(target: u64, data: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(data.len() + 20);
    minicbor::Encoder::new(&mut out)
        .array(2)?
        .u64(target)?
        .bytes(data)?;
    Ok(out)
}
