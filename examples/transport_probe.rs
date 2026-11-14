//! Matched self-route probe. Run against a separate relay process.
use anyhow::{Result, ensure};
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::Barrier;
use war_relay::webtransport::{read_record, write_record};
#[derive(Parser, Clone)]
struct Args {
    #[arg(long)]
    url: String,
    #[arg(long, default_value = "stream")]
    mode: String,
    #[arg(long)]
    certificate: Option<String>,
    #[arg(long, default_value_t = 32)]
    clients: usize,
    #[arg(long, default_value_t = 1024)]
    bytes: usize,
    #[arg(long, default_value_t = 3)]
    seconds: u64,
}
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    let endpoint = if args.mode == "websocket" {
        None
    } else {
        let hash = wtransport::tls::Sha256Digest::from_str_fmt(
            args.certificate.as_ref().expect("--certificate"),
            wtransport::tls::Sha256DigestFmt::DottedHex,
        )?;
        Some(Arc::new(wtransport::Endpoint::client(
            wtransport::ClientConfig::builder()
                .with_bind_default()
                .with_server_certificate_hashes([hash])
                .build(),
        )?))
    };
    let barrier = Arc::new(Barrier::new(args.clients));
    let mut tasks = tokio::task::JoinSet::new();
    for index in 0..args.clients {
        let args = args.clone();
        let endpoint = endpoint.clone();
        let barrier = barrier.clone();
        tasks.spawn(async move {
            let payload = vec![37; args.bytes];
            let mut join = Vec::new();
            minicbor::Encoder::new(&mut join).str(&format!("probe{index}"))?;
            let mut samples = Vec::new();
            if let Some(endpoint) = endpoint {
                let connection = endpoint.connect(&args.url).await?;
                let (mut send, mut recv) = connection.open_bi().await?.await?;
                write_record(&mut send, &join).await?;
                let id = minicbor::Decoder::new(&read_record(&mut recv, 1024).await?).u64()?;
                let packet = packet(id, &payload)?;
                barrier.wait().await;
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(args.seconds) {
                    let tick = Instant::now();
                    let response = if args.mode == "datagram" {
                        connection.send_datagram(&packet)?;
                        tokio::time::timeout(Duration::from_secs(2), connection.receive_datagram())
                            .await??
                            .payload()
                    } else {
                        write_record(&mut send, &packet).await?;
                        read_record(&mut recv, args.bytes + 32).await?
                    };
                    samples.push(tick.elapsed().as_nanos() as u64);
                    ensure!(response.as_ref() == packet, "response mismatch");
                }
                connection.close(wtransport::VarInt::from_u32(0), b"done");
                Ok::<_, anyhow::Error>((samples, start.elapsed().as_secs_f64()))
            } else {
                let (mut ws, _) = tokio_tungstenite::connect_async(&args.url).await?;
                ws.send(tungstenite::Message::Binary(bytes::Bytes::from(join)))
                    .await?;
                let id = minicbor::Decoder::new(&ws.next().await.unwrap()?.into_data()).u64()?;
                let packet = bytes::Bytes::from(packet(id, &payload)?);
                barrier.wait().await;
                let start = Instant::now();
                while start.elapsed() < Duration::from_secs(args.seconds) {
                    let tick = Instant::now();
                    ws.send(tungstenite::Message::Binary(packet.clone()))
                        .await?;
                    let response = loop {
                        match ws.next().await.unwrap()? {
                            tungstenite::Message::Binary(bytes) => break bytes,
                            tungstenite::Message::Ping(_) => ws.flush().await?,
                            other => anyhow::bail!("unexpected message {other:?}"),
                        }
                    };
                    samples.push(tick.elapsed().as_nanos() as u64);
                    ensure!(
                        response == packet,
                        "response mismatch: got {:?}, expected {:?}",
                        &response[..response.len().min(20)],
                        &packet[..packet.len().min(20)]
                    );
                }
                ws.close(None).await?;
                Ok((samples, start.elapsed().as_secs_f64()))
            }
        });
    }
    let mut samples = Vec::new();
    let mut elapsed = 0f64;
    while let Some(result) = tasks.join_next().await {
        let (mut values, seconds) = result??;
        samples.append(&mut values);
        elapsed = elapsed.max(seconds);
    }
    samples.sort_unstable();
    let n = samples.len();
    println!(
        "{}",
        serde_json::json!({"mode":args.mode,"clients":args.clients,"payload_bytes":args.bytes,"deliveries":n,"elapsed_seconds":elapsed,"deliveries_per_second":n as f64/elapsed,"rtt_p50_us":samples[n/2] as f64/1000.,"rtt_p99_us":samples[n*99/100] as f64/1000.,"rtt_max_us":samples[n-1] as f64/1000.})
    );
    Ok(())
}
fn packet(id: u64, payload: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len() + 20);
    minicbor::Encoder::new(&mut out)
        .array(2)?
        .u64(id)?
        .bytes(payload)?;
    Ok(out)
}
