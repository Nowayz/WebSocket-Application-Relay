//! Benchmark transport adapter. Workload generation and delivery assertions stay
//! in the maintained clients.
use anyhow::{Context, Result, ensure};
use bytes::{Bytes, BytesMut};
use std::sync::OnceLock;
use web_transport_quinn::{Client, RecvStream, SendStream, Session as Connection};
static ENDPOINT: OnceLock<Client> = OnceLock::new();
pub fn initialize(pin: &str, receive_buffer_bytes: usize) -> Result<()> {
    let hash = pin.parse::<wtransport::tls::Sha256Digest>()?;
    let config = wtransport::ClientConfig::builder()
        .with_bind_default()
        .with_server_certificate_hashes([hash])
        .build();
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    // This one socket receives bursts from the complete connected population.
    if receive_buffer_bytes != 0 {
        socket2::SockRef::from(&socket).set_recv_buffer_size(receive_buffer_bytes)?;
    }
    socket.set_nonblocking(true)?;
    let endpoint = wtransport::quinn::Endpoint::new(
        config.quic_endpoint_config().clone(),
        None,
        socket,
        std::sync::Arc::new(wtransport::quinn::TokioRuntime),
    )?;
    let endpoint = web_transport_quinn::Client::new(endpoint, config.quic_config().clone());
    ensure!(
        ENDPOINT.set(endpoint).is_ok(),
        "transport already initialized"
    );
    Ok(())
}
pub struct Socket {
    connection: Connection,
    send: SendStream,
    receive: RecvStream,
    reader: Reader,
}
#[derive(Default)]
struct Reader {
    header: [u8; 4],
    header_used: usize,
    size: usize,
    body: BytesMut,
    used: usize,
}
impl Reader {
    // State survives cancellation by the multi-peer select in capacity.rs.
    async fn next(&mut self, receive: &mut RecvStream) -> Result<Bytes> {
        while self.header_used < 4 {
            let n = receive
                .read(&mut self.header[self.header_used..])
                .await?
                .context("record header EOF")?;
            self.header_used += n;
        }
        if self.size == 0 {
            self.size = u32::from_be_bytes(self.header) as usize;
            if self.size == 0 {
                self.header_used = 0;
                return Ok(Bytes::new());
            }
        }
        while self.used < self.size {
            let end = self.size.min(self.used + 4096);
            self.body.resize(end, 0);
            let n = receive
                .read(&mut self.body[self.used..end])
                .await?
                .context("record body EOF")?;
            self.used += n;
        }
        let result = self.body.split_to(self.size).freeze();
        self.header_used = 0;
        self.size = 0;
        self.used = 0;
        Ok(result)
    }
}
impl Socket {
    pub async fn connect(url: &str) -> Result<Self> {
        let connection = ENDPOINT
            .get()
            .context("WebTransport requires --certificate SHA256 pin")?
            .connect(url.parse::<url::Url>()?)
            .await?;
        let (send, receive) = connection.open_bi().await?;
        Ok(Self {
            connection,
            send,
            receive,
            reader: Reader::default(),
        })
    }
    pub async fn send(&mut self, bytes: &[u8]) -> Result<()> {
        war_relay::webtransport::write_record(&mut self.send, bytes).await
    }
    /// The next reliable record. State survives cancellation.
    pub async fn next(&mut self) -> Result<Bytes> {
        self.reader.next(&mut self.receive).await
    }
    #[allow(dead_code)] // Used by the datagram load client, not reliable-only fixtures.
    pub fn send_datagram(&self, bytes: Bytes) -> Result<()> {
        ensure!(
            bytes.len() <= self.connection.max_datagram_size(),
            "datagram exceeds negotiated maximum"
        );
        Ok(self.connection.send_datagram(bytes)?)
    }

    #[allow(dead_code)]
    pub async fn next_datagram_checked(&mut self) -> Result<Bytes> {
        tokio::select! {
            packet = self.connection.read_datagram() => Ok(packet?),
            record = self.reader.next(&mut self.receive) => {
                anyhow::bail!("unexpected reliable record on datagram load lane: {} bytes", record?.len())
            }
        }
    }

    /// Graceful departure: an empty record, the relay's empty acknowledgement,
    /// then a local QUIC close. Callers separately verify peer departure
    /// messages and the server's zero-resource gates.
    pub async fn close(&mut self) -> Result<()> {
        war_relay::webtransport::write_record(&mut self.send, &[]).await?;
        let ack = self.reader.next(&mut self.receive).await?;
        ensure!(
            ack.is_empty(),
            "unconsumed application message before close acknowledgement"
        );
        self.connection.close(1000, b"benchmark complete");
        self.connection.closed().await;
        Ok(())
    }
}
