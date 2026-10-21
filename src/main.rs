use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tokio_util::sync::CancellationToken;
use tracing::info;
use war_relay::{Config, Relay, serve};

fn main() -> Result<()> {
    let mut config = Config::parse();
    config.load_secrets()?;
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
    build_runtime(&config, workers)?.block_on(async move {
        let listener = if config.legacy_websocket { Some(TcpListener::bind(config.bind).await.context("cannot bind relay listener")?) } else { None };
        let admin = if config.no_admin { None } else { Some(TcpListener::bind(config.admin_bind).await.context("cannot bind admin listener")?) };
        info!(address = %config.bind, workers, listen_backlog = config.listen_backlog, protocol = "war.cbor.v1", "relay listening");
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
        if let Some(listener) = listener { return serve(listener, admin, relay, shutdown).await; }
        let identity = match (&relay.config.tls_cert, &relay.config.tls_key) {
            (Some(cert), Some(key)) => wtransport::Identity::load_pemfiles(cert, key).await?,
            _ if relay.config.dev_cert => wtransport::Identity::self_signed(["localhost", "127.0.0.1", "::1"] )?,
            _ => anyhow::bail!("WebTransport requires --tls-cert and --tls-key (or --dev-cert for localhost)"),
        };
        if relay.config.dev_cert {
            info!(certificate_sha256 = %identity.certificate_chain().as_slice()[0].hash(), "development certificate pin");
        }
        let transport = wtransport::ServerConfig::builder().with_bind_address(relay.config.bind).with_identity(identity)
            .keep_alive_interval((relay.config.ping_interval_secs != 0).then(|| std::time::Duration::from_secs(relay.config.ping_interval_secs)))
            .max_idle_timeout((relay.config.ping_interval_secs != 0).then(|| std::time::Duration::from_secs(relay.config.pong_timeout_secs)))?
            .build();
        let (endpoint, _ingress) = war_relay::webtransport::EndpointIngress::bind(&relay.config, transport)
            .context("cannot bind relay listener")?;
        let admin_task = admin.map(|listener| {
            let router = war_relay::server::admin_router(relay.clone());
            let stop = shutdown.clone();
            tokio::spawn(async move { axum::serve(listener, router).with_graceful_shutdown(stop.cancelled_owned()).await })
        });
        let outcome = war_relay::webtransport::serve(endpoint, relay, shutdown.clone()).await;
        shutdown.cancel();
        if let Some(task) = admin_task { task.await??; }
        outcome
    })
}

fn build_runtime(config: &Config, workers: usize) -> Result<tokio::runtime::Runtime> {
    if config.legacy_websocket {
        return Ok(tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?);
    }
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder
        .worker_threads(workers)
        .enable_all()
        .event_interval(1);
    if !config.pin_workers {
        return Ok(builder.build()?);
    }
    let cores = core_affinity::get_core_ids().context("cannot discover allowed CPUs")?;
    anyhow::ensure!(
        cores.len() >= workers,
        "pin-workers needs one allowed CPU per worker"
    );
    let next = std::sync::atomic::AtomicUsize::new(0);
    builder.thread_name_fn(move || {
        let index = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if index < workers {
            format!("war-io-{index}")
        } else {
            format!("war-helper-{index}")
        }
    });
    let (started, ready) = std::sync::mpsc::channel();
    builder.on_thread_start(move || {
        let thread = std::thread::current();
        if let Some(index) = thread
            .name()
            .and_then(|name| name.strip_prefix("war-io-"))
            .and_then(|index| index.parse::<usize>().ok())
        {
            let core = cores[index];
            let pinned = core_affinity::set_for_current(core);
            let _ = started.send((index, core.id, pinned));
        }
    });
    let runtime = builder.build()?;
    for _ in 0..workers {
        let (worker, cpu, pinned) = ready
            .recv()
            .context("WebTransport worker startup stopped")?;
        anyhow::ensure!(
            pinned,
            "cannot pin WebTransport worker {worker} to CPU {cpu}"
        );
        info!(worker, cpu, "pinned WebTransport worker");
    }
    Ok(runtime)
}
