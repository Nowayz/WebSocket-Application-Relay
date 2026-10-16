//! One stable broadcast room alongside the inspection probe's other work.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn corruption_at_a_non_primary_reader_is_rejected() {
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
            let Some(Ok(Message::Binary(request))) = peers[0].next().await else {
                anyhow::bail!("missing fake broadcast");
            };
            let (target, value) =
                protocol::parse(&request).map_err(|_| anyhow::anyhow!("invalid fake request"))?;
            ensure!(target == protocol::BROADCAST);
            let good = protocol::envelope(1, value);
            let mut corrupt = good.to_vec();
            *corrupt.last_mut().unwrap() ^= 1;
            peers[1].send(Message::Binary(good)).await?;
            peers[2].send(Message::Binary(corrupt.into())).await?;
            // Keep all transports open: an EOF at the primary reader must not
            // mask the secondary reader's full-body validation failure.
            std::future::pending::<Result<()>>().await
        });
        let mut counts = Counts::default();
        let group = Fanout::new(&url, 2, Wire::Cbor, &mut counts).await.unwrap();
        let error = timeout(
            Duration::from_secs(2),
            group.run(Instant::now() + Duration::from_secs(1)),
        )
        .await
        .unwrap()
        .err()
        .expect("corrupt non-primary delivery was accepted");
        assert!(
            error
                .to_string()
                .contains("reader 1: body/sender/FIFO changed"),
            "{error}"
        );
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

    pub(super) async fn run(self, until: Instant) -> Result<(Self, Counts, Histogram)> {
        let Self {
            mut sender,
            readers,
            packets,
            wire,
        } = self;
        let readers_len = readers.len();
        let (windows, initial) = watch::channel(None::<(usize, Instant)>);
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
                    let (first, stamp) =
                        (*windows.borrow_and_update()).context("missing fanout window")?;
                    for offset in 0..WINDOW {
                        let bytes = data(&mut socket).await?;
                        ensure!(
                            bytes == packets[(first + offset) % 64].1,
                            "fanout reader {index}: body/sender/FIFO changed"
                        );
                        counts.received(&bytes);
                        stats.record(stamp.elapsed());
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
        let mut seen = vec![false; readers_len];
        while Instant::now() < until {
            windows.send_replace(Some((sequence, Instant::now())));
            for offset in 0..WINDOW {
                let packet = packets[(sequence + offset) % 64].0.clone();
                counts.sent(&packet);
                counts.value_bytes += 1027;
                sender.send(Message::Binary(packet)).await?;
            }
            seen.fill(false);
            for _ in 0..readers_len {
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
                ensure!(!seen[index], "duplicate fanout window acknowledgement");
                seen[index] = true;
            }
            sequence = (sequence + WINDOW) % 64;
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
        Ok((
            Self {
                sender,
                readers,
                packets,
                wire,
            },
            counts,
            stats,
        ))
    }

    pub(super) async fn close(mut self, counts: &mut Counts) -> Result<()> {
        // One departure at a time, with every surviving peer reading its exact
        // notification before the next close. Keep the normal queue limits.
        while let Some((mut socket, id)) = self.readers.pop() {
            close(&mut socket, counts).await?;
            let value = match self.wire {
                Wire::Cbor => protocol::uint(id),
                Wire::Cpp => Bytes::copy_from_slice(&id.to_le_bytes()),
            };
            let expected = self.wire.envelope(protocol::BROADCAST, &value);
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
