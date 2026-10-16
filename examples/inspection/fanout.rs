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
}

#[derive(Clone)]
enum Step {
    Messages(usize, Instant, usize),
    Departure(Bytes),
}

type ReaderResult = Result<((Socket, u64), Counts, Histogram)>;

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

    #[tokio::test]
    async fn corruption_at_a_non_primary_reader_is_rejected() {
        check_corruption(false).await;
    }

    #[tokio::test]
    async fn a_changed_departure_at_a_non_primary_reader_is_rejected() {
        check_corruption(true).await;
    }

    async fn check_corruption(bad_departure: bool) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}/", listener.local_addr().unwrap());
        let fake = tokio::spawn(async move {
            let mut peers = Vec::new();
            for id in 1..=3 {
                let (stream, _) = listener.accept().await?;
                let mut socket = tokio_tungstenite::accept_async(stream).await?;
                ensure!(matches!(socket.next().await, Some(Ok(Message::Binary(_)))));
                socket.send(Message::Binary(protocol::uint(id))).await?;
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
        let group = Fanout::new(&url, 2, Wire::Cbor, &mut counts).await.unwrap();
        let error = timeout(
            Duration::from_secs(2),
            group.run(
                Instant::now() + Duration::from_secs(1),
                url,
                if bad_departure { 2 } else { 0 },
                if bad_departure { 1 } else { WINDOW },
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
        counts: &mut Counts,
    ) -> Result<Self> {
        let name = Bytes::from_static(b"fanout");
        let (sender, id) = join(url, &name, wire, counts).await?;
        let mut peers = Vec::with_capacity(readers);
        for _ in 0..readers {
            peers.push(join(url, &name, wire, counts).await?);
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
        })
    }

    pub(super) async fn run(
        self,
        until: Instant,
        url: String,
        churn_every: usize,
        window: usize,
    ) -> Result<(Self, Counts, Histogram, Histogram)> {
        let Self {
            mut sender,
            readers,
            packets,
            wire,
        } = self;
        let readers_len = readers.len();
        let (windows, initial) = watch::channel(None::<Step>);
        let (ack, mut acks) = mpsc::channel(readers_len);
        let mut jobs = JoinSet::new();
        for (index, (mut socket, id)) in readers.into_iter().enumerate() {
            let mut windows = initial.clone();
            let packets = packets.clone();
            let ack = ack.clone();
            jobs.spawn(async move {
                let mut counts = Counts::default();
                let mut stats = Histogram::new();
                while windows.changed().await.is_ok() {
                    let step = windows
                        .borrow_and_update()
                        .clone()
                        .context("missing fanout window")?;
                    match step {
                        Step::Messages(first, stamp, length) => {
                            for offset in 0..length {
                                let bytes = data(&mut socket).await?;
                                ensure!(
                                    bytes == packets[(first + offset) % 64].1,
                                    "fanout reader {index}: body/sender/FIFO changed"
                                );
                                counts.received(&bytes);
                                stats.record(stamp.elapsed());
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
                Ok::<_, anyhow::Error>(((socket, id), counts, stats))
            });
        }
        drop(initial);
        drop(ack);
        let mut counts = Counts::default();
        let mut sequence = 0usize;
        let mut iteration = 0usize;
        let mut membership = Histogram::new();
        let mut seen = vec![false; readers_len];
        while Instant::now() < until {
            let joined_at = Instant::now();
            let transient = if churn_every > 0 && iteration.is_multiple_of(churn_every) {
                Some(join(&url, &Bytes::from_static(b"fanout"), wire, &mut counts).await?)
            } else {
                None
            };
            windows.send_replace(Some(Step::Messages(sequence, Instant::now(), window)));
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
        while let Some(job) = jobs.join_next().await {
            let (peer, n, s) = job??;
            readers.push(peer);
            counts.merge(&n);
            stats.merge(&s);
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
            },
            counts,
            stats,
            membership,
        ))
    }

    pub(super) async fn close(mut self, counts: &mut Counts) -> Result<()> {
        // One departure at a time, with every surviving peer reading its exact
        // notification before the next close. Keep the normal queue limits.
        while let Some((mut socket, id)) = self.readers.pop() {
            close(&mut socket, counts).await?;
            let expected = departure(self.wire, id);
            for (peer, _) in &mut self.readers {
                let actual = data(peer).await?;
                ensure!(actual == expected, "fanout departure changed");
                counts.received(&actual);
            }
            let actual = data(&mut self.sender).await?;
            ensure!(actual == expected, "fanout sender departure changed");
            counts.received(&actual);
        }
        close(&mut self.sender, counts).await
    }
}
