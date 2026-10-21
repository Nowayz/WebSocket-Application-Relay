//! A blocked encrypted UDP transmit must survive packet construction for another
//! connection on the same runtime thread without changing a single wire byte.
use bytes::Bytes;
use std::{
    io::{self, IoSliceMut},
    net::{SocketAddr, UdpSocket},
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering::SeqCst},
    },
    task::{Context, Poll, Waker},
    time::Duration,
};
use wtransport::quinn::{self, AsyncUdpSocket, Runtime, UdpPoller};

#[derive(Debug, Default)]
struct Pause {
    phase: AtomicUsize,
    packet: Mutex<Option<(SocketAddr, Vec<u8>)>>,
    waiters: Mutex<Vec<Waker>>,
    verified: AtomicBool,
}
#[derive(Debug)]
struct Socket {
    inner: Arc<dyn AsyncUdpSocket>,
    pause: Arc<Pause>,
}
#[derive(Debug)]
struct Poller {
    inner: Pin<Box<dyn UdpPoller>>,
    pause: Arc<Pause>,
}
impl UdpPoller for Poller {
    fn poll_writable(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        if this.pause.phase.load(SeqCst) == 2 {
            let mut waiters = this.pause.waiters.lock().unwrap();
            if !waiters.iter().any(|w| w.will_wake(cx.waker())) {
                waiters.push(cx.waker().clone());
            }
            return Poll::Pending;
        }
        this.inner.as_mut().poll_writable(cx)
    }
}
impl AsyncUdpSocket for Socket {
    fn create_io_poller(self: Arc<Self>) -> Pin<Box<dyn UdpPoller>> {
        Box::pin(Poller {
            inner: self.inner.clone().create_io_poller(),
            pause: self.pause.clone(),
        })
    }
    fn try_send(&self, transmit: &quinn::udp::Transmit<'_>) -> io::Result<()> {
        if self
            .pause
            .phase
            .compare_exchange(1, 4, SeqCst, SeqCst)
            .is_ok()
        {
            *self.pause.packet.lock().unwrap() =
                Some((transmit.destination, transmit.contents.to_vec()));
            self.pause.phase.store(2, SeqCst);
            return Err(io::ErrorKind::WouldBlock.into());
        }
        if self.pause.phase.load(SeqCst) == 3 {
            let packet = self.pause.packet.lock().unwrap();
            if let Some((destination, bytes)) = packet.as_ref()
                && *destination == transmit.destination
                && !self.pause.verified.swap(true, SeqCst)
            {
                assert_eq!(
                    transmit.contents, bytes,
                    "blocked ciphertext changed while another connection built a packet"
                );
            }
        }
        self.inner.try_send(transmit)
    }
    fn poll_recv(
        &self,
        cx: &mut Context<'_>,
        bufs: &mut [IoSliceMut<'_>],
        meta: &mut [quinn::udp::RecvMeta],
    ) -> Poll<io::Result<usize>> {
        self.inner.poll_recv(cx, bufs, meta)
    }
    fn local_addr(&self) -> io::Result<SocketAddr> {
        self.inner.local_addr()
    }
    fn max_transmit_segments(&self) -> usize {
        self.inner.max_transmit_segments()
    }
    fn max_receive_segments(&self) -> usize {
        self.inner.max_receive_segments()
    }
    fn may_fragment(&self) -> bool {
        self.inner.may_fragment()
    }
}

#[tokio::test(flavor = "current_thread")]
async fn blocked_ciphertext_survives_other_connection_construction() -> anyhow::Result<()> {
    tokio::time::timeout(Duration::from_secs(5), async {
        let identity = wtransport::Identity::self_signed(["localhost"])?;
        let hash = identity.certificate_chain().as_slice()[0].hash();
        let config = wtransport::ServerConfig::builder()
            .with_bind_default(0)
            .with_identity(identity)
            .build();
        let pause = Arc::new(Pause::default());
        let socket = Socket {
            inner: quinn::TokioRuntime.wrap_udp_socket(UdpSocket::bind("127.0.0.1:0")?)?,
            pause: pause.clone(),
        };
        let server = quinn::Endpoint::new_with_abstract_socket(
            config.quic_endpoint_config().clone(),
            Some(config.quic_config().clone()),
            Arc::new(socket),
            Arc::new(quinn::TokioRuntime),
        )?;
        let client_config = wtransport::ClientConfig::builder()
            .with_bind_default()
            .with_server_certificate_hashes([hash])
            .build();
        let mut clients = Vec::new();
        let mut peers = Vec::new();
        let mut accepted = Vec::new();
        for _ in 0..2 {
            let mut client = quinn::Endpoint::client("127.0.0.1:0".parse()?)?;
            client.set_default_client_config(client_config.quic_config().clone());
            let (peer, incoming) =
                tokio::join!(client.connect(server.local_addr()?, "localhost")?, async {
                    server.accept().await.unwrap().await
                });
            peers.push(peer?);
            accepted.push(incoming?);
            clients.push(client);
        }
        pause.phase.store(1, SeqCst);
        let first = Bytes::from(vec![0x37; 900]);
        accepted[0].send_datagram(first.clone())?;
        while pause.phase.load(SeqCst) != 2 {
            tokio::task::yield_now().await;
        }
        let before = accepted[1].stats().udp_tx.bytes;
        let second = Bytes::from(vec![0xb5; 600]);
        accepted[1].send_datagram(second.clone())?;
        while accepted[1].stats().udp_tx.bytes <= before {
            tokio::task::yield_now().await;
        }
        // Both packets have been constructed on this one thread while the first
        // socket send is still pending. Resume and inspect its exact ciphertext.
        pause.phase.store(3, SeqCst);
        let waiters = std::mem::take(&mut *pause.waiters.lock().unwrap());
        for waker in waiters {
            waker.wake();
        }
        assert_eq!(peers[0].read_datagram().await?, first);
        assert_eq!(peers[1].read_datagram().await?, second);
        assert!(pause.verified.load(SeqCst));
        server.close(0u32.into(), b"done");
        for client in &clients {
            client.close(0u32.into(), b"done");
        }
        server.wait_idle().await;
        for client in &clients {
            client.wait_idle().await;
        }
        Ok::<(), anyhow::Error>(())
    })
    .await??;
    Ok(())
}
