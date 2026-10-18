//! Verified broadcast windows, with optional membership changes inside the room.
use super::*;
use tokio::{
    sync::{mpsc, watch},
    task::JoinSet,
};

pub(super) struct Fanout {
    sender: Socket,
    readers: Vec<(Socket, u64)>,
    packets: Arc<Vec<(Bytes, Bytes)>>,
    wire: Wire,
    global_observers: bool,
}

#[derive(Clone)]
enum Step {
    Messages(usize, Instant, Instant, usize),
    Departure(Bytes),
}

type ReaderResult = Result<((Socket, u64), Counts, Histogram, Option<ExactLatency>)>;

// Diagnostic client storage only. A paced run has a finite sample bound; reserve
// once and refuse overflow instead of allocating while recording deliveries.
struct ExactLatency {
    samples: Vec<(u64, u64)>,
    limit: usize,
}
impl ExactLatency {
    fn new(limit: usize) -> Self {
        Self {
            samples: Vec::with_capacity(limit),
            limit,
        }
    }
    fn record(&mut self, elapsed: Duration, scheduled: Duration) -> Result<()> {
        ensure!(
            self.samples.len() < self.limit,
            "exact fanout sample bound exceeded"
        );
        self.samples.push((
            elapsed.as_nanos().try_into()?,
            scheduled.as_nanos().try_into()?,
        ));
        Ok(())
    }
    fn write(&mut self, out: &mut impl Write, id: u64) -> Result<()> {
        ensure!(!self.samples.is_empty(), "missing exact fanout samples");
        let rank = (self.samples.len() * 99).div_ceil(100) - 1;
        self.samples.sort_unstable_by_key(|sample| sample.0);
        let p99 = self.samples[rank].0;
        let max = self.samples.last().unwrap().0;
        self.samples.sort_unstable_by_key(|sample| sample.1);
        writeln!(
            out,
            "{id},{},{p99},{max},{},{}",
            self.samples.len(),
            self.samples[rank].1,
            self.samples.last().unwrap().1
        )?;
        Ok(())
    }
}

async fn acknowledge(
    acks: &mut mpsc::Receiver<usize>,
    jobs: &mut JoinSet<ReaderResult>,
    seen: &mut [bool],
) -> Result<()> {
    seen.fill(false);
    for _ in 0..seen.len() {
        let index = timeout(Duration::from_secs(5), async {
            tokio::select! {
                index = acks.recv() => index.context("fanout readers stopped"),
                ended = jobs.join_next() => {
                    if let Some(job) = ended { job??; }
                    anyhow::bail!("fanout reader ended before final window")
                }
            }
        })
        .await
        .context("fanout window timed out")??;
        ensure!(
            index < seen.len() && !seen[index],
            "invalid fanout acknowledgement"
        );
        seen[index] = true;
    }
    Ok(())
}

fn departure(wire: Wire, id: u64) -> Bytes {
    let value = match wire {
        Wire::Cbor => protocol::uint(id),
        Wire::Cpp => Bytes::copy_from_slice(&id.to_le_bytes()),
    };
    wire.envelope(protocol::BROADCAST, &value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_latency_is_bounded_and_reports_nearest_rank() {
        let mut samples = ExactLatency::new(100);
        for ns in 1..=100 {
            samples
                .record(Duration::from_nanos(ns), Duration::from_nanos(ns + 10))
                .unwrap();
        }
        assert!(samples.record(Duration::ZERO, Duration::ZERO).is_err());
        let mut out = Vec::new();
        samples.write(&mut out, 42).unwrap();
        assert_eq!(String::from_utf8(out).unwrap(), "42,100,99,100,109,110\n");
        assert!(ExactLatency::new(1).write(&mut Vec::new(), 0).is_err());
    }

    #[tokio::test]
    async fn corruption_at_a_non_primary_reader_is_rejected() {
        check_corruption(false, false).await;
    }

    #[tokio::test]
    async fn a_changed_departure_at_a_non_primary_reader_is_rejected() {
        check_corruption(true, false).await;
    }

    #[tokio::test]
    async fn corruption_at_a_non_primary_global_observer_is_rejected() {
        check_corruption(false, true).await;
    }

    async fn check_corruption(bad_departure: bool, global_observers: bool) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let fake = tokio::spawn(async move {
            let mut peers = Vec::new();
            for id in 1..=3 {
                let (stream, _) = listener.accept().await?;
                let mut socket = tokio_tungstenite::accept_async(stream).await?;
                ensure!(matches!(socket.next().await, Some(Ok(Message::Binary(_)))));
                socket.send(Message::Binary(protocol::uint(id))).await?;
                if global_observers && id != 1 {
                    ensure!(matches!(socket.next().await, Some(Ok(Message::Binary(_)))));
                    socket
                        .send(Message::Binary(Bytes::from_static(b"\x82\0\x82\0\xf5")))
                        .await?;
                    ensure!(matches!(socket.next().await, Some(Ok(Message::Binary(_)))));
                    socket
                        .send(Message::Binary(Bytes::from_static(b"\x82\0\x82\x01\x01")))
                        .await?;
                }
                peers.push(socket);
            }
            let mut transient = if bad_departure {
                let (stream, _) = listener.accept().await?;
                let mut socket = tokio_tungstenite::accept_async(stream).await?;
                ensure!(matches!(socket.next().await, Some(Ok(Message::Binary(_)))));
                socket.send(Message::Binary(protocol::uint(4))).await?;
                Some(socket)
            } else {
                None
            };
            let Some(Ok(Message::Binary(request))) = peers[0].next().await else {
                anyhow::bail!("missing fake broadcast");
            };
            let (target, value) =
                protocol::parse(&request).map_err(|_| anyhow::anyhow!("invalid fake request"))?;
            ensure!(target == protocol::BROADCAST);
            let good = protocol::envelope(1, value);
            let mut corrupt = good.to_vec();
            *corrupt.last_mut().unwrap() ^= 1;
            peers[1].send(Message::Binary(good.clone())).await?;
            peers[2]
                .send(Message::Binary(if bad_departure {
                    good.clone()
                } else {
                    corrupt.into()
                }))
                .await?;
            if let Some(mut socket) = transient.take() {
                socket.send(Message::Binary(good)).await?;
                ensure!(matches!(
                    socket.next().await,
                    Some(Ok(Message::Close(Some(_))))
                ));
                socket.flush().await?;
                drop(socket);
                let notice = departure(Wire::Cbor, 4);
                for (index, peer) in peers.iter_mut().enumerate() {
                    let packet = if index == 2 {
                        departure(Wire::Cbor, 5)
                    } else {
                        notice.clone()
                    };
                    peer.send(Message::Binary(packet)).await?;
                }
            }
            // Keep all transports open: an EOF at the primary reader must not
            // mask the secondary reader's full-body validation failure.
            std::future::pending::<Result<()>>().await
        });
        let mut counts = Counts::default();
        let group = Fanout::new(&url, 2, Wire::Cbor, global_observers, &mut counts)
            .await
            .unwrap();
        let error = timeout(
            Duration::from_secs(2),
            group.run(
                Instant::now() + Duration::from_secs(1),
                url,
                if bad_departure { 2 } else { 0 },
                if bad_departure { 1 } else { WINDOW },
                0,
                None,
            ),
        )
        .await
        .unwrap()
        .err()
        .expect("corrupt non-primary delivery was accepted");
        let expected = if bad_departure {
            "reader 1: departure changed"
        } else {
            "reader 1: body/sender/FIFO changed"
        };
        assert!(error.to_string().contains(expected), "{error}");
        fake.abort();
        assert!(fake.await.unwrap_err().is_cancelled());
    }
}

impl Fanout {
    pub(super) async fn new(
        url: &str,
        readers: usize,
        wire: Wire,
        global_observers: bool,
        counts: &mut Counts,
    ) -> Result<Self> {
        let name = Bytes::from_static(b"fanout");
        let (sender, id) = join(url, &name, wire, counts).await?;
        let mut peers = Vec::with_capacity(readers);
        for _ in 0..readers {
            let reader_name = if global_observers {
                Bytes::from_static(protocol::GLOBAL_CHANNEL)
            } else {
                name.clone()
            };
            let (mut socket, id) = join(url, &reader_name, wire, counts).await?;
            if global_observers {
                authenticate(&mut socket, wire, counts).await?;
                ensure!(
                    exchange(
                        &mut socket,
                        protocol::request(1, &protocol::uint(u64::from(protocol::CHANNEL_MESSAGE))),
                        counts
                    )
                    .await?
                    .as_ref()
                        == b"\x82\0\x82\x01\x01",
                    "channel observer opt-in"
                );
            }
            peers.push((socket, id));
        }
        let mut payload = [0x5a; 1024];
        let packets = (0u64..64)
            .map(|n| {
                payload[..8].copy_from_slice(&n.to_le_bytes());
                let value = protocol::byte_string(&payload);
                (
                    wire.envelope(protocol::BROADCAST, &value),
                    wire.envelope(id, &value),
                )
            })
            .collect();
        Ok(Self {
            sender,
            readers: peers,
            packets: Arc::new(packets),
            wire,
            global_observers,
        })
    }

    pub(super) async fn run(
        self,
        until: Instant,
        url: String,
        churn_every: usize,
        window: usize,
        windows_per_second: u32,
        latency_output: Option<std::path::PathBuf>,
    ) -> Result<(Self, Counts, Histogram, Histogram)> {
        let Self {
            mut sender,
            readers,
            packets,
            wire,
            global_observers,
        } = self;
        ensure!(
            latency_output.is_none() || windows_per_second != 0,
            "exact fanout latency requires paced windows"
        );
        let sample_limit = (until.saturating_duration_since(Instant::now()).as_secs() as usize + 2)
            .checked_mul(windows_per_second as usize)
            .and_then(|n| n.checked_mul(window))
            .context("exact fanout sample bound overflow")?;
        let readers_len = readers.len();
        let (windows, initial) = watch::channel(None::<Step>);
        let (ack, mut acks) = mpsc::channel(readers_len);
        let mut jobs = JoinSet::new();
        for (index, (mut socket, id)) in readers.into_iter().enumerate() {
            let mut windows = initial.clone();
            let packets = packets.clone();
            let ack = ack.clone();
            let exact_enabled = latency_output.is_some();
            jobs.spawn(async move {
                let mut counts = Counts::default();
                let mut stats = Histogram::new();
                let mut exact = exact_enabled.then(|| ExactLatency::new(sample_limit));
                while windows.changed().await.is_ok() {
                    let step = windows
                        .borrow_and_update()
                        .clone()
                        .context("missing fanout window")?;
                    match step {
                        Step::Messages(first, stamp, scheduled, length) => {
                            for offset in 0..length {
                                let bytes = data(&mut socket).await?;
                                ensure!(
                                    bytes == packets[(first + offset) % 64].1,
                                    "fanout reader {index}: body/sender/FIFO changed"
                                );
                                counts.received(&bytes);
                                let now = Instant::now();
                                stats.record(now.duration_since(stamp));
                                if let Some(exact) = &mut exact {
                                    exact.record(
                                        now.duration_since(stamp),
                                        now.duration_since(scheduled),
                                    )?;
                                }
                            }
                        }
                        Step::Departure(expected) => {
                            let bytes = data(&mut socket).await?;
                            ensure!(
                                bytes == expected,
                                "fanout reader {index}: departure changed"
                            );
                            counts.received(&bytes);
                        }
                    }
                    ack.send(index).await.context("fanout controller stopped")?;
                }
                Ok::<_, anyhow::Error>(((socket, id), counts, stats, exact))
            });
        }
        drop(initial);
        drop(ack);
        let mut counts = Counts::default();
        let mut sequence = 0usize;
        let mut iteration = 0usize;
        let mut membership = Histogram::new();
        let mut seen = vec![false; readers_len];
        let mut pacing = (windows_per_second != 0).then(|| {
            let mut timer =
                tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(windows_per_second)));
            timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            timer
        });
        while Instant::now() < until {
            let scheduled = if let Some(timer) = &mut pacing {
                let scheduled = timer.tick().await.into_std();
                if Instant::now() >= until {
                    break;
                }
                scheduled
            } else {
                Instant::now()
            };
            let joined_at = Instant::now();
            let transient = if churn_every > 0 && iteration.is_multiple_of(churn_every) {
                Some(join(&url, &Bytes::from_static(b"fanout"), wire, &mut counts).await?)
            } else {
                None
            };
            windows.send_replace(Some(Step::Messages(
                sequence,
                Instant::now(),
                scheduled,
                window,
            )));
            for offset in 0..window {
                let packet = packets[(sequence + offset) % 64].0.clone();
                counts.sent(&packet);
                counts.value_bytes += 1027;
                sender.send(Message::Binary(packet)).await?;
            }
            acknowledge(&mut acks, &mut jobs, &mut seen).await?;
            if let Some((mut socket, id)) = transient {
                for offset in 0..window {
                    let bytes = data(&mut socket).await?;
                    ensure!(
                        bytes == packets[(sequence + offset) % 64].1,
                        "transient fanout body changed"
                    );
                    counts.received(&bytes);
                }
                close(&mut socket, &mut counts).await?;
                let expected = departure(wire, id);
                windows.send_replace(Some(Step::Departure(expected.clone())));
                let actual = data(&mut sender).await?;
                ensure!(actual == expected, "fanout sender departure changed");
                counts.received(&actual);
                acknowledge(&mut acks, &mut jobs, &mut seen).await?;
                membership.record(joined_at.elapsed());
            }
            sequence = (sequence + window) % 64;
            iteration += 1;
        }
        drop(windows);
        let mut stats = Histogram::new();
        let mut readers = Vec::with_capacity(readers_len);
        let mut exact_output = latency_output
            .map(|path| {
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)
            })
            .transpose()?
            .map(io::BufWriter::new);
        if let Some(out) = &mut exact_output {
            writeln!(
                out,
                "id,samples,p99_ns,max_ns,schedule_p99_ns,schedule_max_ns"
            )?;
        }
        while let Some(job) = jobs.join_next().await {
            let (peer, n, s, mut exact) = job??;
            if let (Some(out), Some(exact)) = (&mut exact_output, &mut exact) {
                exact.write(out, peer.1)?;
            }
            readers.push(peer);
            counts.merge(&n);
            stats.merge(&s);
        }
        if let Some(out) = &mut exact_output {
            out.flush()?;
        }
        ensure!(
            stats.n == counts.sent_messages * readers_len as u64,
            "incomplete fanout delivery matrix"
        );
        ensure!(
            counts.joins == membership.n
                && counts.normal_closes == membership.n
                && counts.deliveries == stats.n + membership.n * (readers_len + window + 2) as u64,
            "incomplete fanout membership delivery matrix"
        );
        Ok((
            Self {
                sender,
                readers,
                packets,
                wire,
                global_observers,
            },
            counts,
            stats,
            membership,
        ))
    }

    pub(super) async fn close(mut self, counts: &mut Counts, window: usize) -> Result<()> {
        // Close a bounded batch, draining earlier departures before closing its
        // next member. Survivors then verify the exact same ordered sequence.
        // This amortizes receive syscalls for large audiences without discarding
        // a notification, bypassing quotas, or changing measured traffic.
        while !self.readers.is_empty() {
            let mut expected = Vec::with_capacity(window);
            for _ in 0..window.min(self.readers.len()) {
                let (mut socket, id) = self.readers.pop().expect("nonempty reader batch");
                for departure in &expected {
                    ensure!(
                        data(&mut socket).await? == *departure,
                        "fanout departure changed"
                    );
                    counts.received(departure);
                }
                close(&mut socket, counts).await?;
                expected.push(departure(self.wire, id));
            }
            for (peer, _) in &mut self.readers {
                for departure in &expected {
                    ensure!(data(peer).await? == *departure, "fanout departure changed");
                    counts.received(departure);
                }
            }
            if !self.global_observers {
                for departure in &expected {
                    ensure!(
                        data(&mut self.sender).await? == *departure,
                        "fanout sender departure changed"
                    );
                    counts.received(departure);
                }
            }
        }
        close(&mut self.sender, counts).await
    }
}
