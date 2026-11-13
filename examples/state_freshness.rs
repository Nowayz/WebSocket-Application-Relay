//! Compare independently validated state updates sharing a connection with a
//! reliable bulk value. Run inside an isolated network namespace for impairment.
use anyhow::{Context, Result, ensure};
use bytes::{Bytes, BytesMut};
use clap::{Parser, ValueEnum};
use futures_util::{
    SinkExt, StreamExt,
    stream::{SplitSink, SplitStream},
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering::Relaxed},
    },
    time::{Duration, Instant},
};
use tokio::sync::mpsc;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use war_relay::{
    protocol,
    webtransport::{read_record, write_record},
};
use web_transport_quinn::{Client, RecvStream, SendStream, Session};

#[derive(Clone, Copy, Debug, ValueEnum, PartialEq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
enum Mode {
    Websocket,
    WebsocketTls,
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
    #[arg(long)]
    ca_certificate: Option<PathBuf>,
    #[arg(long, default_value_t = 10)]
    seconds: u64,
    #[arg(long, default_value_t = 60)]
    rate: u64,
    #[arg(long, default_value_t = 1024)]
    payload_bytes: usize,
    #[arg(long, default_value_t=1024*1024)]
    bulk_bytes: usize,
    /// QUIC packet-count loss threshold for this client's own sending. The default matches the
    /// relay (time-based detection only); 3 is the stock QUIC value.
    #[arg(long, default_value_t = u32::MAX)]
    packet_threshold: u32,
}
type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
enum SendSide {
    Ws(SplitSink<Ws, Message>),
    Wt(Box<Session>, SendStream),
}
enum ReadSide {
    Ws(SplitStream<Ws>),
    Wt(Box<Session>, RecvStream),
}
struct Peer {
    send: SendSide,
    read: ReadSide,
    id: u64,
}
async fn peer(args: &Args, endpoint: Option<&Client>) -> Result<Peer> {
    let join = protocol::text("freshness");
    if let Some(endpoint) = endpoint {
        let connection = endpoint.connect(url::Url::parse(&args.url)?).await?;
        let (mut send, mut read) = connection.open_bi().await?;
        write_record(&mut send, &join).await?;
        let id = minicbor::Decoder::new(&read_record(&mut read, 1024).await?).u64()?;
        Ok(Peer {
            send: SendSide::Wt(Box::new(connection.clone()), send),
            read: ReadSide::Wt(Box::new(connection), read),
            id,
        })
    } else {
        let connector = if args.mode == Mode::WebsocketTls {
            let cert = std::fs::read(
                args.ca_certificate
                    .as_ref()
                    .context("TLS comparison requires --ca-certificate")?,
            )?;
            let mut roots = tokio_rustls::rustls::RootCertStore::empty();
            roots.add(tokio_rustls::rustls::pki_types::CertificateDer::from(cert))?;
            let config = tokio_rustls::rustls::ClientConfig::builder_with_protocol_versions(&[
                &tokio_rustls::rustls::version::TLS13,
            ])
            .with_root_certificates(roots)
            .with_no_client_auth();
            Some(tokio_tungstenite::Connector::Rustls(Arc::new(config)))
        } else {
            None
        };
        let (mut ws, _) =
            tokio_tungstenite::connect_async_tls_with_config(&args.url, None, true, connector)
                .await?;
        ws.send(Message::Binary(join)).await?;
        let id =
            minicbor::Decoder::new(&ws.next().await.context("welcome EOF")??.into_data()).u64()?;
        let (send, read) = ws.split();
        Ok(Peer {
            send: SendSide::Ws(send),
            read: ReadSide::Ws(read),
            id,
        })
    }
}
fn endpoint(args: &Args) -> Result<Option<Client>> {
    if matches!(args.mode, Mode::Websocket | Mode::WebsocketTls) {
        return Ok(None);
    }
    let pin = args
        .certificate
        .as_ref()
        .context("WebTransport requires --certificate")?
        .parse::<wtransport::tls::Sha256Digest>()?;
    let config = wtransport::ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes([pin])
        .build();
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket2::SockRef::from(&socket).set_recv_buffer_size(4 * 1024 * 1024)?;
    socket.set_nonblocking(true)?;
    let endpoint = wtransport::quinn::Endpoint::new(
        config.quic_endpoint_config().clone(),
        None,
        socket,
        Arc::new(wtransport::quinn::TokioRuntime),
    )?;
    // This client is the sender on the upload hop, so by default it uses the relay's loss
    // detection; a browser brings its own.
    let mut quic = config.quic_config().clone();
    let mut transport = wtransport::quinn::TransportConfig::default();
    transport.packet_threshold(args.packet_threshold);
    quic.transport_config(Arc::new(transport));
    Ok(Some(Client::new(endpoint, quic)))
}
struct Observed {
    seen: Vec<bool>,
    unique: usize,
    duplicates: usize,
    reordered: usize,
    latest_sequence: Option<usize>,
    latest_generated_ns: Option<u64>,
    arrival_age_ms: Vec<f64>,
    updates_before_bulk: usize,
    bulk_arrival_ms: Option<f64>,
}
struct Check {
    state: Mutex<Observed>,
    offered_stamps: Vec<AtomicU64>,
    start: Instant,
    sender: u64,
    payload_bytes: usize,
    bulk_bytes: usize,
    reliable: bool,
}
impl Check {
    fn receive(&self, packet: &[u8]) -> Result<()> {
        let now = self.start.elapsed();
        let mut decoder = minicbor::Decoder::new(packet);
        ensure!(decoder.array()? == Some(2), "invalid relay envelope");
        ensure!(decoder.u64()? == self.sender, "sender mismatch");
        let bytes = decoder.bytes()?;
        ensure!(
            decoder.position() == packet.len(),
            "trailing envelope bytes"
        );
        let mut state = self.state.lock().unwrap();
        if bytes.len() == self.bulk_bytes {
            ensure!(
                bytes.iter().all(|&byte| byte == 0xa5),
                "bulk payload corrupted"
            );
            ensure!(state.bulk_arrival_ms.is_none(), "duplicate bulk delivery");
            state.bulk_arrival_ms = Some(now.as_secs_f64() * 1000.);
            return Ok(());
        }
        ensure!(
            bytes.len() == self.payload_bytes && bytes.len() >= 16,
            "state size mismatch"
        );
        ensure!(
            bytes[16..].iter().all(|&byte| byte == 0x5a),
            "state payload corrupted"
        );
        let sequence = usize::try_from(u64::from_be_bytes(bytes[..8].try_into()?))?;
        ensure!(sequence < state.seen.len(), "unknown sequence");
        let stamp = u64::from_be_bytes(bytes[8..16].try_into()?);
        ensure!(
            stamp == self.offered_stamps[sequence].load(Relaxed) && stamp > 0,
            "generation stamp mismatch"
        );
        ensure!(stamp <= now.as_nanos() as u64, "future generation stamp");
        if state.seen[sequence] {
            state.duplicates += 1;
            ensure!(!self.reliable, "duplicate reliable state");
            return Ok(());
        }
        if state.bulk_arrival_ms.is_none() {
            state.updates_before_bulk += 1;
            ensure!(
                !self.reliable,
                "reliable state overtook the preceding bulk value"
            );
        }
        state.seen[sequence] = true;
        state.unique += 1;
        state
            .arrival_age_ms
            .push((now.as_nanos() as u64 - stamp) as f64 / 1e6);
        if state.latest_sequence.is_none_or(|latest| sequence > latest) {
            state.latest_sequence = Some(sequence);
            state.latest_generated_ns = Some(stamp);
        } else {
            state.reordered += 1;
            ensure!(!self.reliable, "reordered reliable state");
        }
        Ok(())
    }
}
async fn receive(
    mut read: ReadSide,
    datagrams: bool,
    check: Arc<Check>,
    stop: tokio_util::sync::CancellationToken,
) -> Result<ReadSide> {
    let work = async {
        match &mut read {
            ReadSide::Ws(stream) => {
                while let Some(packet) = stream.next().await {
                    let Message::Binary(bytes) = packet? else {
                        anyhow::bail!("unexpected WebSocket frame");
                    };
                    check.receive(&bytes)?;
                }
                anyhow::bail!("unexpected receiver EOF")
            }
            ReadSide::Wt(connection, stream) => loop {
                // Preserve a partially read reliable record while datagrams arrive.
                let record = read_record(stream, check.bulk_bytes + 32);
                tokio::pin!(record);
                loop {
                    tokio::select! {
                        result=&mut record => {check.receive(&result?)?; break;},
                        result=connection.read_datagram(), if datagrams => check.receive(&result?)?,
                    }
                }
            },
        }
    };
    tokio::select! { result=work => { result?; }, _=stop.cancelled() => {} }
    Ok(read)
}
async fn publish(
    mut send: SendSide,
    mut updates: mpsc::UnboundedReceiver<Bytes>,
    bulk: Bytes,
    datagrams: bool,
) -> Result<SendSide> {
    match &mut send {
        SendSide::Ws(sink) => {
            sink.send(Message::Binary(bulk)).await?;
            while let Some(packet) = updates.recv().await {
                sink.send(Message::Binary(packet)).await?;
            }
        }
        SendSide::Wt(connection, stream) if datagrams => {
            let reliable = write_record(stream, &bulk);
            let states = async {
                while let Some(packet) = updates.recv().await {
                    connection.send_datagram(packet)?;
                }
                Ok::<(), anyhow::Error>(())
            };
            tokio::try_join!(reliable, states)?;
        }
        SendSide::Wt(_, stream) => {
            write_record(stream, &bulk).await?;
            while let Some(packet) = updates.recv().await {
                write_record(stream, &packet).await?;
            }
        }
    }
    Ok(send)
}
async fn close(mut send: SendSide, mut read: ReadSide, other: u64) -> Result<()> {
    let departure = protocol::envelope(protocol::BROADCAST, &protocol::uint(other));
    match (&mut send, &mut read) {
        (SendSide::Wt(connection, stream), ReadSide::Wt(_, receive)) => {
            write_record(stream, &[]).await?;
            loop {
                let packet = read_record(receive, 1024).await?;
                if packet.is_empty() {
                    break;
                }
                ensure!(packet == departure, "unexpected stream data during close");
            }
            connection.close(1000, b"freshness complete");
            wtransport::quinn::Connection::closed(connection).await;
        }
        (SendSide::Ws(sink), ReadSide::Ws(stream)) => {
            let frame = tokio_tungstenite::tungstenite::protocol::CloseFrame {
                code: 1000.into(),
                reason: "freshness".into(),
            };
            sink.send(Message::Close(Some(frame.clone()))).await?;
            let mut acknowledged = false;
            while let Some(packet) = stream.next().await {
                match packet? {
                    Message::Binary(bytes) => {
                        ensure!(bytes == departure, "unexpected data during close")
                    }
                    Message::Close(Some(reply)) => {
                        ensure!(reply == frame, "close acknowledgement mismatch");
                        acknowledged = true;
                        sink.flush().await?;
                    }
                    other => anyhow::bail!("unexpected close frame {other:?}"),
                }
            }
            ensure!(acknowledged, "missing close acknowledgement");
        }
        _ => anyhow::bail!("mismatched transport halves"),
    }
    Ok(())
}

fn distribution(mut values: Vec<f64>) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::Value::Null;
    }
    values.sort_unstable_by(f64::total_cmp);
    let n = values.len();
    serde_json::json!({"samples":n,"p50_ms":values[n/2],"p99_ms":values[n*99/100],"max_ms":values[n-1]})
}
#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        args.seconds > 0
            && args.rate > 0
            && args.payload_bytes >= 16
            && args.bulk_bytes > args.payload_bytes,
        "invalid workload shape"
    );
    let count = usize::try_from(
        args.seconds
            .checked_mul(args.rate)
            .context("workload count overflow")?,
    )?;
    let endpoint = endpoint(&args)?;
    let source = peer(&args, endpoint.as_ref()).await?;
    let target = peer(&args, endpoint.as_ref()).await?;
    // Prepare finite test storage and complete CBOR packets before measurement.
    let mut templates = Vec::with_capacity(count);
    for sequence in 0..count {
        let mut payload = vec![0x5a; args.payload_bytes];
        payload[..8].copy_from_slice(&(sequence as u64).to_be_bytes());
        let packet = protocol::envelope(target.id, &protocol::byte_string(&payload));
        templates.push(BytesMut::from(packet.as_ref()));
    }
    let bulk = protocol::envelope(
        target.id,
        &protocol::byte_string(&vec![0xa5; args.bulk_bytes]),
    );
    let check = Arc::new(Check {
        state: Mutex::new(Observed {
            seen: vec![false; count],
            unique: 0,
            duplicates: 0,
            reordered: 0,
            latest_sequence: None,
            latest_generated_ns: None,
            arrival_age_ms: Vec::with_capacity(count),
            updates_before_bulk: 0,
            bulk_arrival_ms: None,
        }),
        offered_stamps: (0..count).map(|_| AtomicU64::new(0)).collect(),
        start: Instant::now(),
        sender: source.id,
        payload_bytes: args.payload_bytes,
        bulk_bytes: args.bulk_bytes,
        reliable: args.mode != Mode::Datagram,
    });
    let (offers, updates) = mpsc::unbounded_channel();
    let stop = tokio_util::sync::CancellationToken::new();
    let received = tokio::spawn(receive(
        target.read,
        args.mode == Mode::Datagram,
        check.clone(),
        stop.clone(),
    ));
    let publisher = tokio::spawn(publish(
        source.send,
        updates,
        bulk,
        args.mode == Mode::Datagram,
    ));
    let generated = check.clone();
    let rate = args.rate;
    let generator = tokio::spawn(async move {
        let mut lag = Vec::with_capacity(count);
        for (sequence, mut packet) in templates.into_iter().enumerate() {
            let nominal =
                Duration::from_nanos((sequence as u128 * 1_000_000_000 / u128::from(rate)) as u64);
            tokio::time::sleep_until((generated.start + nominal).into()).await;
            let stamp = generated.start.elapsed().as_nanos() as u64;
            lag.push((Duration::from_nanos(stamp).saturating_sub(nominal)).as_secs_f64() * 1000.);
            generated.offered_stamps[sequence].store(stamp, Relaxed);
            let offset = packet.len() - generated.payload_bytes + 8;
            packet[offset..offset + 8].copy_from_slice(&stamp.to_be_bytes());
            offers.send(packet.freeze()).context("publisher stopped")?;
        }
        Ok::<_, anyhow::Error>(lag)
    });
    let mut ages = Vec::new();
    let mut no_state = 0;
    let mut interval = tokio::time::interval_at(check.start.into(), Duration::from_millis(20));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    while check.start.elapsed() < Duration::from_secs(args.seconds) {
        interval.tick().await;
        let latest = check.state.lock().unwrap().latest_generated_ns;
        if latest.is_none() {
            no_state += 1;
        }
        // Both modes begin with an initial state at t0. Keep probes before the
        // first update in the distribution so delayed delivery is not hidden.
        ages.push((check.start.elapsed().as_nanos() as u64 - latest.unwrap_or(0)) as f64 / 1e6);
    }
    let generation_lag = generator.await??;
    let published = tokio::time::timeout(Duration::from_secs(30), publisher).await???;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if received.is_finished() {
            let _ = received.await.context("receiver task failed")??;
            anyhow::bail!("receiver ended before validation completed");
        }
        let complete = {
            let state = check.state.lock().unwrap();
            state.bulk_arrival_ms.is_some()
                && (args.mode == Mode::Datagram || state.unique == count)
        };
        if complete {
            break;
        }
        ensure!(
            Instant::now() < deadline,
            "delivery drain deadline exceeded"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if args.mode == Mode::Datagram {
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
    let observation_end_seconds = check.start.elapsed().as_secs_f64();
    stop.cancel();
    let restored = received.await??;
    tokio::time::timeout(Duration::from_secs(30), async {
        tokio::try_join!(
            close(published, source.read, target.id),
            close(target.send, restored, source.id)
        )?;
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    let mut state = check.state.lock().unwrap();
    let report = serde_json::json!({"mode":args.mode,"offered":count,"received_unique":state.unique,"missing":count-state.unique,"duplicates":state.duplicates,"out_of_order":state.reordered,"offered_seconds":args.seconds,"rate":args.rate,"payload_bytes":args.payload_bytes,"bulk_bytes":args.bulk_bytes,"packet_threshold":args.packet_threshold,"bulk_arrival_ms":state.bulk_arrival_ms,"updates_before_bulk":state.updates_before_bulk,"arrival_age":distribution(std::mem::take(&mut state.arrival_age_ms)),"freshest_state_age":distribution(ages),"probes_before_first_state":no_state,"generation_lag":distribution(generation_lag),"elapsed_seconds":observation_end_seconds});
    println!("{report}");
    ensure!(
        state.unique > 0 && (!check.reliable || state.unique == count),
        "incomplete reliable state delivery"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(reliable: bool) -> Check {
        Check {
            state: Mutex::new(Observed {
                seen: vec![false; 2],
                unique: 0,
                duplicates: 0,
                reordered: 0,
                latest_sequence: None,
                latest_generated_ns: None,
                arrival_age_ms: Vec::new(),
                updates_before_bulk: 0,
                bulk_arrival_ms: None,
            }),
            offered_stamps: vec![AtomicU64::new(1000), AtomicU64::new(2000)],
            start: Instant::now() - Duration::from_secs(1),
            sender: 42,
            payload_bytes: 32,
            bulk_bytes: 64,
            reliable,
        }
    }
    fn state(sequence: u64, stamp: u64) -> Bytes {
        let mut payload = [0x5a; 32];
        payload[..8].copy_from_slice(&sequence.to_be_bytes());
        payload[8..16].copy_from_slice(&stamp.to_be_bytes());
        protocol::envelope(42, &protocol::byte_string(&payload))
    }
    #[test]
    fn reordered_datagrams_never_replace_the_freshest_state() {
        let check = fixture(false);
        check.receive(&state(1, 2000)).unwrap();
        check.receive(&state(0, 1000)).unwrap();
        let result = check.state.lock().unwrap();
        assert_eq!(result.latest_sequence, Some(1));
        assert_eq!(result.latest_generated_ns, Some(2000));
        assert_eq!(result.reordered, 1);
        assert_eq!(result.updates_before_bulk, 2);
    }
    #[test]
    fn reliable_order_and_independent_payload_stamp_checks_are_enforced() {
        let check = fixture(true);
        assert!(
            check.receive(&state(0, 1000)).is_err(),
            "reliable state cannot overtake bulk"
        );
        let check = fixture(true);
        check
            .receive(&protocol::envelope(42, &protocol::byte_string(&[0xa5; 64])))
            .unwrap();
        check.receive(&state(1, 2000)).unwrap();
        assert!(check.receive(&state(0, 1000)).is_err());
        assert!(fixture(false).receive(&state(0, 999)).is_err());
        assert!(
            fixture(false)
                .receive(&protocol::envelope(43, &protocol::byte_string(&[0; 32])))
                .is_err()
        );
    }
}
