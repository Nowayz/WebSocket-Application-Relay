use std::{net::SocketAddr, time::Duration};

use anyhow::{Result, ensure};
use clap::Parser;

pub(crate) const MAX_HTTP_UPGRADE_BYTES: usize = 16 * 1024;

/// Resource limits apply to each process. Secrets are loaded separately and never formatted.
#[derive(Clone, Parser)]
#[command(version, about = "WebSocket Application Relay (Rust)")]
pub struct Config {
    #[arg(long, env = "WAR_BIND", default_value = "127.0.0.1:1338")]
    pub bind: SocketAddr,
    #[arg(long, env = "WAR_ADMIN_BIND", default_value = "127.0.0.1:1339")]
    pub admin_bind: SocketAddr,
    #[arg(long, env = "WAR_NO_ADMIN")]
    pub no_admin: bool,
    /// Requested pending TCP queue per native listener; the OS may cap it.
    #[arg(long, env = "WAR_LISTEN_BACKLOG", default_value_t = 1024, value_parser = clap::value_parser!(u16).range(1..))]
    pub listen_backlog: u16,
    #[command(flatten)]
    pub origin_policy: crate::origin::OriginPolicy,
    #[arg(long, env = "WAR_MAX_CONNECTIONS", default_value_t = 10_000)]
    pub max_connections: usize,
    /// Maximum admitted TCP connections per direct peer IP; zero disables.
    #[arg(long, env = "WAR_MAX_CONNECTIONS_PER_IP", default_value_t = 0)]
    pub max_connections_per_ip: usize,
    #[arg(long, env = "WAR_MAX_CHANNELS", default_value_t = 10_000)]
    pub max_channels: usize,
    #[arg(long, env = "WAR_MAX_CHANNEL_MEMBERS", default_value_t = 10_000)]
    pub max_channel_members: usize,
    #[arg(long, env = "WAR_MAX_MESSAGE_BYTES", default_value_t = 16 * 1024 * 1024)]
    pub max_message_bytes: usize,
    #[arg(long, env = "WAR_QUEUE_MESSAGES", default_value_t = 64)]
    pub queue_messages: usize,
    #[arg(long, env = "WAR_QUEUE_BYTES", default_value_t = 32 * 1024 * 1024)]
    pub queue_bytes: usize,
    #[arg(long, env = "WAR_TOTAL_QUEUE_BYTES", default_value_t = 256 * 1024 * 1024)]
    pub total_queue_bytes: usize,
    #[arg(long, env = "WAR_READ_BUFFER_BYTES", default_value_t = 4 * 1024)]
    pub read_buffer_bytes: usize,
    /// Shared decoder staging allowance, including growth headroom; zero disables admission.
    #[arg(long, env = "WAR_TOTAL_READ_BUFFER_BYTES", default_value_t = 1024 * 1024 * 1024)]
    pub total_read_buffer_bytes: usize,
    /// Reclaim enlarged decoder storage after this many seconds without large input; zero disables.
    #[arg(long, env = "WAR_READ_BUFFER_IDLE_SECS", default_value_t = 5)]
    pub read_buffer_idle_secs: u64,
    #[arg(long, env = "WAR_MAX_VARIABLES", default_value_t = 256)]
    pub max_variables: usize,
    #[arg(long, env = "WAR_MAX_VARIABLE_BYTES", default_value_t = 1024 * 1024)]
    pub max_variable_bytes: usize,
    #[arg(long, env = "WAR_TOTAL_VARIABLE_BYTES", default_value_t = 64 * 1024 * 1024)]
    pub total_variable_bytes: usize,
    /// Per-connection token bucket; zero disables the corresponding rate limit.
    #[arg(long, env = "WAR_MESSAGES_PER_SECOND", default_value_t = 1000)]
    pub messages_per_second: u64,
    #[arg(long, env = "WAR_BYTES_PER_SECOND", default_value_t = 16 * 1024 * 1024)]
    pub bytes_per_second: u64,
    #[arg(long, env = "WAR_HANDSHAKE_TIMEOUT_SECS", default_value_t = 5)]
    pub handshake_timeout_secs: u64,
    #[arg(long, env = "WAR_JOIN_TIMEOUT_SECS", default_value_t = 10)]
    pub join_timeout_secs: u64,
    #[arg(long, env = "WAR_WRITE_TIMEOUT_SECS", default_value_t = 5)]
    pub write_timeout_secs: u64,
    /// Zero disables server heartbeats.
    #[arg(long, env = "WAR_PING_INTERVAL_SECS", default_value_t = 15)]
    pub ping_interval_secs: u64,
    #[arg(long, env = "WAR_PONG_TIMEOUT_SECS", default_value_t = 45)]
    pub pong_timeout_secs: u64,
    /// Joined sessions with no application input/output expire; zero disables.
    #[arg(long, env = "WAR_IDLE_TIMEOUT_SECS", default_value_t = 0)]
    pub idle_timeout_secs: u64,
    #[arg(long, env = "WAR_SHUTDOWN_TIMEOUT_SECS", default_value_t = 10)]
    pub shutdown_timeout_secs: u64,
    /// Disable the legacy ability of re_globl members to broadcast to all users.
    #[arg(long, env = "WAR_DISABLE_GLOBAL_BROADCAST")]
    pub disable_global_broadcast: bool,
    #[arg(long, env = "WAR_WORKERS")]
    pub workers: Option<usize>,
    #[arg(skip)]
    pub auth_password: Option<String>,
    #[arg(skip)]
    pub admin_token: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self::parse_from(["war-relay"])
    }
}

impl Config {
    pub fn load_secrets(&mut self) {
        self.auth_password = std::env::var("WAR_AUTH_PASSWORD")
            .ok()
            .filter(|s| !s.is_empty());
        self.admin_token = std::env::var("WAR_ADMIN_TOKEN")
            .ok()
            .filter(|s| !s.is_empty());
    }

    pub fn validate(&self) -> Result<()> {
        self.origin_policy.validate()?;
        ensure!(
            self.listen_backlog > 0,
            "listen-backlog must be in 1..=65535"
        );
        ensure!(
            self.max_connections_per_ip <= u32::MAX as usize,
            "max-connections-per-ip must be in 0..=4294967295"
        );
        for (name, value) in [
            ("max-connections", self.max_connections),
            ("max-channels", self.max_channels),
            ("max-channel-members", self.max_channel_members),
            ("queue-messages", self.queue_messages),
            ("read-buffer-bytes", self.read_buffer_bytes),
            ("max-variables", self.max_variables),
            ("max-variable-bytes", self.max_variable_bytes),
        ] {
            ensure!(
                value > 0 && value <= u32::MAX as usize,
                "{name} must be in 1..=4294967295"
            );
        }
        ensure!(
            (17..=64 * 1024 * 1024).contains(&self.max_message_bytes),
            "max-message-bytes must be in 17..=67108864"
        );
        ensure!(
            self.queue_bytes >= self.max_message_bytes + 10,
            "queue-bytes must fit one maximum-sized message plus a 10-byte CBOR routing header"
        );
        ensure!(
            self.queue_bytes <= isize::MAX as usize,
            "queue-bytes exceeds the addressable range"
        );
        ensure!(
            self.total_queue_bytes >= self.queue_bytes
                && self.total_queue_bytes <= isize::MAX as usize,
            "total-queue-bytes must fit one session budget and the addressable range"
        );
        ensure!(
            self.total_variable_bytes >= self.max_variable_bytes
                && self.total_variable_bytes <= isize::MAX as usize,
            "total-variable-bytes must fit one channel budget and the addressable range"
        );
        ensure!(
            self.read_buffer_bytes <= 1024 * 1024,
            "read-buffer-bytes cannot exceed 1048576"
        );
        ensure!(
            self.total_read_buffer_bytes == 0
                || (self.total_read_buffer_bytes >= self.handshake_read_reservation()
                    && self.total_read_buffer_bytes <= isize::MAX as usize),
            "total-read-buffer-bytes must be zero or fit one handshake staging reservation and the addressable range"
        );
        ensure!(
            self.max_connections <= tokio::sync::Semaphore::MAX_PERMITS,
            "max-connections exceeds semaphore capacity"
        );
        ensure!(
            self.queue_messages <= tokio::sync::Semaphore::MAX_PERMITS,
            "queue-messages exceeds semaphore capacity"
        );
        ensure!(
            self.workers.is_none_or(|n| n > 0),
            "workers must be positive"
        );
        for value in [
            self.handshake_timeout_secs,
            self.join_timeout_secs,
            self.write_timeout_secs,
            self.shutdown_timeout_secs,
        ] {
            ensure!(
                (1..=86_400).contains(&value),
                "timeouts must be in 1..=86400 seconds"
            );
        }
        ensure!(
            self.ping_interval_secs <= 86_400,
            "ping interval exceeds one day"
        );
        ensure!(
            self.idle_timeout_secs <= 86_400,
            "idle timeout exceeds one day"
        );
        ensure!(
            self.read_buffer_idle_secs <= 86_400,
            "read buffer idle interval exceeds one day"
        );
        ensure!(
            (1..=86_400).contains(&self.pong_timeout_secs),
            "pong timeout must be in 1..=86400 seconds"
        );
        ensure!(
            self.no_admin || self.admin_bind.ip().is_loopback() || self.admin_token.is_some(),
            "a non-loopback admin listener requires WAR_ADMIN_TOKEN"
        );
        ensure!(
            self.auth_password.as_ref().is_none_or(|s| s.len() <= 4096),
            "WAR_AUTH_PASSWORD exceeds 4096 bytes"
        );
        ensure!(
            self.admin_token
                .as_ref()
                .is_none_or(|s| !s.is_empty() && s.len() <= 4096),
            "WAR_ADMIN_TOKEN must contain 1..=4096 bytes"
        );
        Ok(())
    }

    pub fn write_timeout(&self) -> Duration {
        Duration::from_secs(self.write_timeout_secs)
    }

    pub(crate) fn handshake_read_reservation(&self) -> usize {
        // The HTTP reader retains a bounded Vec plus a 4 KiB chunk. Two times
        // their sum covers growth and transfer into the decoder's BytesMut.
        2 * self.read_buffer_bytes.max(MAX_HTTP_UPGRADE_BYTES + 4096)
    }
}

#[cfg(test)]
mod backlog_tests {
    use super::*;

    #[test]
    fn listen_backlog_is_bounded_for_cli_and_embedding() {
        assert_eq!(Config::default().listen_backlog, 1024);
        for value in ["0", "65536"] {
            assert!(Config::try_parse_from(["war-relay", "--listen-backlog", value]).is_err());
        }
        for backlog in [1, u16::MAX] {
            let config = Config {
                listen_backlog: backlog,
                ..Config::default()
            };
            config.validate().unwrap();
        }
        assert!(
            Config {
                listen_backlog: 0,
                ..Config::default()
            }
            .validate()
            .is_err()
        );
    }
}
