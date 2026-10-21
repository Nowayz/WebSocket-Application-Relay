//! Give shared UDP receive dispatch independent progress under fanout load.
use anyhow::{Context, Result};
use tokio::sync::oneshot;
use wtransport::quinn::Endpoint;

/// Owns the endpoint's I/O runtime. Keep it alive through session shutdown.
/// Connection and application drivers still run on the caller's runtime.
/// This thread shares the process CPU allowance; it does not reserve another CPU.
pub struct EndpointIngress {
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl EndpointIngress {
    pub fn bind(
        config: &crate::Config,
        transport: wtransport::ServerConfig,
    ) -> Result<(Endpoint, Self)> {
        let config = config.clone();
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel(1);
        let (stop, stopped) = oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("war-ingress".into())
            .spawn(move || {
                match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime.block_on(async move {
                        let result = super::bind_endpoint(&config, &transport);
                        if ready_tx.send(result).is_ok() {
                            let _ = stopped.await;
                        }
                    }),
                    Err(error) => {
                        let _ = ready_tx.send(Err(error.into()));
                    }
                }
            })?;
        // Install ownership before receiving so startup errors also stop and join.
        let ingress = Self {
            stop: Some(stop),
            thread: Some(thread),
        };
        let endpoint = ready_rx.recv().context("QUIC ingress startup stopped")??;
        Ok((endpoint, ingress))
    }
}

impl Drop for EndpointIngress {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::warn!("QUIC ingress thread panicked");
        }
    }
}
