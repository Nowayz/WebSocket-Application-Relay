//! Benchmark transport adapter. Workload generation and delivery assertions stay
//! in the maintained clients. QUIC close is explicit; never fabricate a WS ACK.
use anyhow::{Context, Result, bail, ensure};
use bytes::{Bytes, BytesMut};
use futures_util::{SinkExt, StreamExt};
use std::sync::OnceLock;
use tokio::net::TcpStream;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};
use wtransport::{
    Connection, Endpoint, RecvStream, SendStream, VarInt, endpoint::endpoint_side::Client,
};
static ENDPOINT: OnceLock<Endpoint<Client>> = OnceLock::new();
pub fn initialize(pin: Option<&str>) -> Result<()> {
    if let Some(pin) = pin {
        let hash = pin.parse::<wtransport::tls::Sha256Digest>()?;
        let endpoint = Endpoint::client(
            wtransport::ClientConfig::builder()
                .with_bind_default()
                .with_server_certificate_hashes([hash])
                .build(),
        )?;
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
    pub async fn connect(url: &str) -> Result<Self> {
        let connection = ENDPOINT
            .get()
            .context("WebTransport requires --certificate SHA256 pin")?
            .connect(url)
            .await?;
        let (send, receive) = connection.open_bi().await?.await?;
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
            socket
                .connection
                .close(VarInt::from_u32(1000), b"benchmark complete");
            socket.connection.closed().await;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}
