//! Independent real-socket echo and policy-refused TCP admission load.
use anyhow::{Context, Result, ensure};
use bytes::Bytes;
use clap::Parser;
use futures_util::{SinkExt, StreamExt};
use std::{
    io::{self, Write},
    net::SocketAddr,
    time::{Duration, Instant},
};
use tokio::{io::AsyncReadExt, net::TcpStream, task::JoinSet, time::timeout};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async_with_config,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        protocol::{CloseFrame, WebSocketConfig},
    },
};
use war_relay::protocol;

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;
const WINDOW: usize = 16;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    url: String,
    #[arg(long)]
    tcp: SocketAddr,
    #[arg(long, default_value_t = 6)]
    seconds: u64,
    #[arg(long, default_value_t = 256)]
    flood_workers: usize,
    /// Let an external verifier own the established session for shutdown probes.
    #[arg(long)]
    flood_only: bool,
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
        serde_json::json!({"verified":self.n,"per_second":self.n as f64/seconds,
            "p50_upper_ms":percentile(50),"p99_upper_ms":percentile(99),"max_ms":self.maximum as f64/1e6})
    }
}

fn gate(event: &str, details: serde_json::Value) -> Result<()> {
    println!("{}", serde_json::json!({"event":event,"details":details}));
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    ensure!(input.trim() == "continue", "controller stopped at {event}");
    Ok(())
}

async fn binary(socket: &mut Socket) -> Result<Bytes> {
    match timeout(Duration::from_secs(5), socket.next())
        .await?
        .context("missing data")??
    {
        Message::Binary(bytes) => Ok(bytes),
        other => anyhow::bail!("unexpected {other:?}"),
    }
}

async fn healthy(url: &str) -> Result<(Socket, Vec<Bytes>, usize)> {
    let mut request = url.into_client_request()?;
    request
        .headers_mut()
        .insert("sec-websocket-protocol", protocol::SUBPROTOCOL.parse()?);
    let (mut socket, response) = timeout(
        Duration::from_secs(5),
        connect_async_with_config(
            request,
            Some(WebSocketConfig::default().read_buffer_size(4096)),
            true,
        ),
    )
    .await??;
    ensure!(
        response
            .headers()
            .get("sec-websocket-protocol")
            .is_some_and(|value| value == protocol::SUBPROTOCOL),
        "CBOR subprotocol was not negotiated"
    );
    socket
        .send(Message::Binary(protocol::byte_string(b"admission")))
        .await?;
    let welcome = binary(&mut socket).await?;
    let mut decoder = minicbor::Decoder::new(&welcome);
    let id = decoder.u64()?;
    ensure!(
        decoder.position() == welcome.len() && id > 0 && id != protocol::BROADCAST,
        "invalid welcome"
    );
    let mut payload = [0x5a; 1024];
    let packets = (0u64..64)
        .map(|n| {
            payload[..8].copy_from_slice(&n.to_le_bytes());
            protocol::envelope(id, &protocol::byte_string(&payload))
        })
        .collect();
    Ok((socket, packets, welcome.len()))
}

async fn traffic(
    mut socket: Socket,
    packets: Vec<Bytes>,
    until: Instant,
) -> Result<(Socket, Histogram)> {
    let mut stats = Histogram::new();
    let mut sequence = 0;
    while Instant::now() < until {
        let stamp = Instant::now();
        for offset in 0..WINDOW {
            socket
                .send(Message::Binary(packets[(sequence + offset) % 64].clone()))
                .await?;
        }
        for offset in 0..WINDOW {
            let bytes = binary(&mut socket).await?;
            ensure!(
                bytes == packets[(sequence + offset) % 64],
                "echo body/sender/FIFO changed"
            );
            stats.record(stamp.elapsed());
        }
        sequence = (sequence + WINDOW) % 64;
    }
    Ok((socket, stats))
}

fn refused(result: io::Result<usize>) -> Result<()> {
    match result {
        Ok(0) => Ok(()),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ) =>
        {
            Ok(())
        }
        other => anyhow::bail!("policy-refused TCP must end without bytes: {other:?}"),
    }
}

async fn flood(address: SocketAddr, until: Instant) -> Result<Histogram> {
    let mut stats = Histogram::new();
    while Instant::now() < until {
        let stamp = Instant::now();
        let mut stream = timeout(Duration::from_secs(5), TcpStream::connect(address)).await??;
        let mut byte = [0];
        refused(timeout(Duration::from_secs(5), stream.read(&mut byte)).await?)?;
        stats.record(stamp.elapsed());
    }
    Ok(stats)
}

async fn close(socket: &mut Socket) -> Result<()> {
    let frame = CloseFrame {
        code: 1000.into(),
        reason: "admission".into(),
    };
    socket.send(Message::Close(Some(frame.clone()))).await?;
    ensure!(
        timeout(Duration::from_secs(5), socket.next())
            .await?
            .context("missing Close")??
            == Message::Close(Some(frame)),
        "Close acknowledgement changed"
    );
    socket.flush().await?;
    ensure!(
        timeout(Duration::from_secs(5), socket.next())
            .await?
            .is_none(),
        "missing Close EOF"
    );
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        (1..=60).contains(&args.seconds)
            && args.flood_workers <= 1024
            && (!args.flood_only || args.flood_workers > 0),
        "invalid workload bounds"
    );
    let session = if args.flood_only {
        None
    } else {
        Some(healthy(&args.url).await?)
    };
    let welcome_bytes = session.as_ref().map_or(0, |(_, _, n)| *n);
    let packet_bytes = session
        .as_ref()
        .map_or(0, |(_, packets, _)| packets[0].len());
    gate(
        "ready",
        serde_json::json!({"joins":u64::from(session.is_some()),"welcome_bytes":welcome_bytes,"packet_bytes":packet_bytes}),
    )?;
    let started = Instant::now();
    let until = started + Duration::from_secs(args.seconds);
    let active = session.map(|(socket, packets, _)| tokio::spawn(traffic(socket, packets, until)));
    let mut jobs = JoinSet::new();
    for _ in 0..args.flood_workers {
        jobs.spawn(flood(args.tcp, until));
    }
    let mut refusals = Histogram::new();
    while let Some(job) = jobs.join_next().await {
        refusals.merge(&job??);
    }
    let (mut socket, stats) = if let Some(task) = active {
        let (socket, stats) = task.await??;
        (Some(socket), stats)
    } else {
        (None, Histogram::new())
    };
    let elapsed = started.elapsed().as_secs_f64();
    gate(
        "measured",
        serde_json::json!({"elapsed_seconds":elapsed,"healthy":stats.report(elapsed),
        "refusals":refusals.report(elapsed),"messages":stats.n,"message_bytes":stats.n * packet_bytes as u64,
        "value_bytes":stats.n * 1027,"normal_closes":0}),
    )?;
    if let Some(socket) = &mut socket {
        close(socket).await?;
    }
    gate(
        "done",
        serde_json::json!({"normal_closes":u64::from(socket.is_some())}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_refusal_requires_eof_or_reset_without_application_bytes() {
        assert!(refused(Ok(0)).is_ok());
        assert!(refused(Err(io::ErrorKind::ConnectionReset.into())).is_ok());
        assert!(refused(Err(io::ErrorKind::ConnectionAborted.into())).is_ok());
        assert!(refused(Ok(1)).is_err());
        assert!(refused(Err(io::ErrorKind::TimedOut.into())).is_err());
    }
}
