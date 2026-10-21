//! Benchmark transport adapter. Workload generation and delivery assertions stay
//! in the maintained clients. QUIC close is explicit; never fabricate a WS ACK.
use anyhow::{Context, Result, bail, ensure};
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::{
    path::Path,
    sync::{Arc, OnceLock},
};
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use web_transport_quinn::{Client, RecvStream, SendStream, Session as Connection};
static ENDPOINT: OnceLock<Client> = OnceLock::new();
static TLS_CONFIG: OnceLock<Arc<tokio_rustls::rustls::ClientConfig>> = OnceLock::new();
pub fn initialize(
    pin: Option<&str>,
    receive_buffer_bytes: usize,
    ca_certificate: Option<&Path>,
) -> Result<()> {
    if let Some(path) = ca_certificate {
        let mut roots = tokio_rustls::rustls::RootCertStore::empty();
        roots.add(tokio_rustls::rustls::pki_types::CertificateDer::from(
            std::fs::read(path)?,
        ))?;
        let tls = tokio_rustls::rustls::ClientConfig::builder_with_protocol_versions(&[
            &tokio_rustls::rustls::version::TLS13,
        ])
        .with_root_certificates(roots)
        .with_no_client_auth();
        ensure!(
            TLS_CONFIG.set(Arc::new(tls)).is_ok(),
            "TLS fixture already initialized"
        );
    }
    if let Some(pin) = pin {
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
    }
    Ok(())
}
pub enum Socket {
    WebSocket(Box<WebSocketStream<MaybeTlsStream<TcpStream>>>),
    WebTransport(Box<Transport>),
}
pub struct Transport {
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
    pub fn websocket(socket: WebSocketStream<MaybeTlsStream<TcpStream>>) -> Self {
        Self::WebSocket(Box::new(socket))
    }
    pub async fn connect_tls(url: &str, bound: Option<TcpStream>) -> Result<Self> {
        let config = TLS_CONFIG
            .get()
            .context("wss fixture requires --ca-certificate DER")?
            .clone();
        let connector = Some(tokio_tungstenite::Connector::Rustls(config));
        let options =
            Some(tungstenite::protocol::WebSocketConfig::default().read_buffer_size(4096));
        let (socket, _) = if let Some(stream) = bound {
            tokio_tungstenite::client_async_tls_with_config(url, stream, options, connector).await?
        } else {
            tokio_tungstenite::connect_async_tls_with_config(url, options, true, connector).await?
        };
        Ok(Self::websocket(socket))
    }
    pub async fn connect(url: &str) -> Result<Self> {
        let connection = ENDPOINT
            .get()
            .context("WebTransport requires --certificate SHA256 pin")?
            .connect(url::Url::parse(url)?)
            .await?;
        let (send, receive) = connection.open_bi().await?;
        Ok(Self::WebTransport(Box::new(Transport {
            connection,
            send,
            receive,
            reader: Reader::default(),
        })))
    }
    pub async fn send(&mut self, message: Message) -> Result<()> {
        match self {
            Self::WebSocket(socket) => Ok(socket.send(message).await?),
            Self::WebTransport(socket) => match message {
                Message::Binary(bytes) => {
                    war_relay::webtransport::write_record(&mut socket.send, &bytes).await
                }
                _ => bail!("use close_transport for WebTransport teardown"),
            },
        }
    }
    pub async fn next(&mut self) -> Option<Result<Message>> {
        match self {
            Self::WebSocket(socket) => socket.next().await.map(|result| result.map_err(Into::into)),
            Self::WebTransport(socket) => Some(
                socket
                    .reader
                    .next(&mut socket.receive)
                    .await
                    .map(Message::Binary),
            ),
        }
    }
    #[allow(dead_code)] // Used by the open-loop load client, not reliable-only fixtures.
    pub fn send_datagram(&self, bytes: Bytes) -> Result<()> {
        let Self::WebTransport(socket) = self else {
            bail!("datagrams require WebTransport")
        };
        ensure!(
            bytes.len() <= socket.connection.max_datagram_size(),
            "datagram exceeds negotiated maximum"
        );
        Ok(socket.connection.send_datagram(bytes)?)
    }

    #[allow(dead_code)]
    pub async fn next_datagram_checked(&mut self) -> Result<Bytes> {
        let Self::WebTransport(socket) = self else {
            bail!("datagrams require WebTransport")
        };
        tokio::select! {
            packet = socket.connection.read_datagram() => Ok(packet?),
            record = socket.reader.next(&mut socket.receive) => {
                bail!("unexpected reliable record on datagram load lane: {} bytes", record?.len())
            }
        }
    }

    pub async fn flush(&mut self) -> Result<()> {
        match self {
            Self::WebSocket(socket) => Ok(socket.flush().await?),
            Self::WebTransport(_) => Ok(()),
        }
    }
    /// Local QUIC closure is not a WebSocket Close exchange. Callers separately
    /// verify peer departure messages and the server's zero-resource gates.
    pub async fn close_transport(&mut self) -> Result<bool> {
        if let Self::WebTransport(socket) = self {
            war_relay::webtransport::write_record(&mut socket.send, &[]).await?;
            let ack = socket.reader.next(&mut socket.receive).await?;
            ensure!(
                ack.is_empty(),
                "unconsumed application message before close acknowledgement"
            );
            socket.connection.close(1000, b"benchmark complete");
            socket.connection.closed().await;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
