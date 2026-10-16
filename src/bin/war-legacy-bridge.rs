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
    /// Maximum admitted TCP connections per direct peer IP; zero disables.
    #[arg(long, env = "WAR_MAX_CONNECTIONS_PER_IP", default_value_t = 0)]
    max_connections_per_ip: usize,
    #[command(flatten)]
    origin_policy: war_relay::origin::OriginPolicy,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    args.origin_policy.validate()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
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
    war_relay::legacy_bridge::serve_with_limits(
        listener,
        args.upstream,
        args.max_connections,
        args.max_connections_per_ip,
        args.origin_policy,
        shutdown,
    )
    .await
}
