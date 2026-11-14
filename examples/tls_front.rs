//! Test-only TLS 1.3 front end for the legacy plaintext relay. Its extra TCP
//! hop, CPU and RSS must be included in encrypted deployment comparisons.
//! `--upstream-sources` spreads upstream connections over loopback source
//! addresses so one front end can hold more sessions than one address has ports.
use anyhow::{Context, Result};
use clap::Parser;
use std::{net::SocketAddr, path::PathBuf, sync::Arc};
use tokio::{
    io::copy_bidirectional_with_sizes,
    net::{TcpListener, TcpSocket},
    task::JoinSet,
};
use tokio_rustls::{
    TlsAcceptor,
    rustls::{
        self,
        pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer},
    },
};
#[derive(Parser)]
struct Args {
    #[arg(long)]
    bind: SocketAddr,
    #[arg(long)]
    upstream: SocketAddr,
    #[arg(long)]
    cert: PathBuf,
    #[arg(long)]
    key: PathBuf,
    #[arg(long, default_value_t = 4)]
    workers: usize,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u8).range(1..=254))]
    upstream_sources: u8,
    /// Per-direction relay buffer held for each connection's lifetime (Tokio's default is 8 KiB).
    #[arg(long, default_value_t = 8192, value_parser = clap::value_parser!(u32).range(1024..=1048576))]
    copy_buffer_bytes: u32,
}
fn main() -> Result<()> {
    let args = Args::parse();
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(args.workers)
        .enable_all()
        .build()?
        .block_on(run(args))
}
async fn run(args: Args) -> Result<()> {
    let identity = wtransport::Identity::load_pemfiles(args.cert, args.key).await?;
    let certs = identity
        .certificate_chain()
        .as_slice()
        .iter()
        .map(|cert| CertificateDer::from(cert.der().to_vec()))
        .collect();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(
        identity.private_key().secret_der().to_vec(),
    ));
    let config = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind(args.bind)
        .await
        .context("bind TLS fixture")?;
    let mut peers = JoinSet::new();
    let mut accepted = 0usize;
    loop {
        tokio::select! {
            Some(_) = peers.join_next(), if !peers.is_empty() => {},
            result = listener.accept() => {
                let (stream, _) = result?;
                stream.set_nodelay(true)?;
                let acceptor = acceptor.clone();
                let upstream = args.upstream;
                let source = 1 + (accepted % usize::from(args.upstream_sources)) as u8;
                let buffer = args.copy_buffer_bytes as usize;
                accepted += 1;
                peers.spawn(async move {
                    let mut tls = acceptor.accept(stream).await?;
                    let socket = TcpSocket::new_v4()?;
                    socket.bind(SocketAddr::from(([127, 0, 0, source], 0)))?;
                    let mut target = socket.connect(upstream).await?;
                    target.set_nodelay(true)?;
                    copy_bidirectional_with_sizes(&mut tls, &mut target, buffer, buffer).await?;
                    Ok::<_, anyhow::Error>(())
                });
            }
        }
    }
}
