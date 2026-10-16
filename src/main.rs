use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::info;
use war_relay::{Config, Relay, serve};

fn main() -> Result<()> {
    let mut config = Config::parse();
    config.load_secrets();
    config.validate()?;
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "war_relay=info".into()),
        )
        .init();
    let workers = config.workers.unwrap_or_else(|| {
        std::thread::available_parallelism()
            .map_or(2, usize::from)
            .min(16)
    });
    tokio::runtime::Builder::new_current_thread().enable_all().build()?.block_on(async move {
        let listener = TcpListener::bind(config.bind).await.context("cannot bind relay listener")?;
        let admin = if config.no_admin { None } else { Some(TcpListener::bind(config.admin_bind).await.context("cannot bind admin listener")?) };
        info!(address = %listener.local_addr()?, workers, protocol = "war.cbor.v1", "relay listening");
        if let Some(admin) = &admin { info!(address = %admin.local_addr()?, "admin listening"); }
        let relay = Relay::new(config)?;
        let shutdown = CancellationToken::new();
        let signal = shutdown.clone();
        tokio::spawn(async move {
            #[cfg(unix)] {
                let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("install SIGTERM handler");
                tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = term.recv() => {} }
            }
            #[cfg(not(unix))] { let _ = tokio::signal::ctrl_c().await; }
            signal.cancel();
        });
        serve(listener, admin, relay, shutdown).await
    })
}
