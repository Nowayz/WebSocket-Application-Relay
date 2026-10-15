use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;

#[derive(Parser)]
#[command(about = "Optional original-WAR wire protocol to CBOR migration bridge")]
struct Args {
    #[arg(long, default_value = "127.0.0.1:1337")]
    bind: std::net::SocketAddr,
    #[arg(long, default_value = "ws://127.0.0.1:1338")]
    upstream: String,
    #[arg(long, default_value_t = 1000)]
    max_connections: usize,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    tracing_subscriber::fmt().with_env_filter("info").init();
    let listener = TcpListener::bind(args.bind)
        .await
        .context("cannot bind legacy bridge")?;
    tracing::info!(address = %listener.local_addr()?, "legacy bridge listening");
    let shutdown = CancellationToken::new();
    let token = shutdown.clone();
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut term =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("SIGTERM handler");
            tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        token.cancel();
    });
    war_relay::legacy_bridge::serve(listener, args.upstream, args.max_connections, shutdown).await
}
