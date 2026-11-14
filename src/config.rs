use std::{net::SocketAddr, time::Duration};

use anyhow::{Result, bail, ensure};
use clap::{CommandFactory, FromArgMatches, Parser};

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub enum PinWorkers {
    /// Pin workers when the allowed CPUs number exactly one per worker. That
    /// partitions only the CPUs the process was given. Under the historical
    /// closed-loop workload it raised private delivery 5-29% at equal CPU per
    /// delivery.
    Auto,
    #[value(alias = "true", alias = "1", alias = "yes")]
    On,
    #[value(alias = "false", alias = "0", alias = "no")]
    Off,
}

impl Config {
    /// CPUs for each data worker, or `None` to leave workers unpinned.
    pub fn worker_cores(&self, workers: usize) -> Result<Option<Vec<core_affinity::CoreId>>> {
        let wanted = match self.pin_workers {
            PinWorkers::Off => return Ok(None),
            PinWorkers::Auto => false,
            PinWorkers::On => true,
        };
        let Some(mut cores) = core_affinity::get_core_ids() else {
            ensure!(!wanted, "cannot discover allowed CPUs for worker affinity");
            return Ok(None);
        };
        if !wanted && cores.len() != workers {
            return Ok(None);
        }
        ensure!(
            cores.len() >= workers,
            "pin-workers needs one allowed CPU per worker: {} CPUs for {workers} workers",
            cores.len()
        );
        cores.truncate(workers);
        Ok(Some(cores))
    }
}

/// Resource limits apply to each process. Secrets are loaded separately and never formatted.
#[derive(Clone, Parser)]
#[command(version, about = "WebTransport Application Relay")]
pub struct Config {
    /// TLS certificate chain in PEM format.
    #[arg(long, env = "WAR_TLS_CERT", requires = "tls_key")]
    pub tls_cert: Option<std::path::PathBuf>,
    /// TLS private key in PEM format.
    #[arg(long, env = "WAR_TLS_KEY", requires = "tls_cert")]
    pub tls_key: Option<std::path::PathBuf>,
    /// Generate an ephemeral localhost certificate and log its public SHA256 pin.
    #[arg(long, env = "WAR_DEV_CERT", conflicts_with = "tls_cert")]
    pub dev_cert: bool,

    #[arg(long, env = "WAR_BIND", default_value = "127.0.0.1:1338")]
    pub bind: SocketAddr,
    #[arg(long, env = "WAR_ADMIN_BIND", default_value = "127.0.0.1:1339")]
    pub admin_bind: SocketAddr,
    #[arg(long, env = "WAR_NO_ADMIN")]
    pub no_admin: bool,
    /// Requested shared QUIC UDP receive buffer; zero keeps the OS default.
    #[arg(long, env = "WAR_UDP_RECEIVE_BUFFER_BYTES", default_value_t = 4 * 1024 * 1024)]
    pub udp_receive_buffer_bytes: usize,
    /// Requested shared QUIC UDP send buffer; zero keeps the OS default.
    #[arg(long, env = "WAR_UDP_SEND_BUFFER_BYTES", default_value_t = 0)]
    pub udp_send_buffer_bytes: usize,
    #[command(flatten)]
    pub origin_policy: crate::origin::OriginPolicy,
    #[arg(long, env = "WAR_MAX_CONNECTIONS", default_value_t = 10_000)]
    pub max_connections: usize,
    /// Maximum admitted sessions per direct peer IP; zero disables.
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
    /// Maximum backing storage for shared reliable-message slabs; zero disables.
    #[arg(long, env = "WAR_RECEIVE_ARENA_BYTES", default_value_t = 0)]
    pub receive_arena_bytes: usize,
    /// Minimum slab allocation on first use by a worker, shared across connections.
    #[arg(long, env = "WAR_RECEIVE_ARENA_PAGE_BYTES", default_value_t = 64 * 1024)]
    pub receive_arena_page_bytes: usize,
    /// Shared staging allowance for records being received, including growth headroom;
    /// zero disables admission.
    #[arg(long, env = "WAR_TOTAL_READ_BUFFER_BYTES", default_value_t = 1024 * 1024 * 1024)]
    pub total_read_buffer_bytes: usize,
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
    /// QUIC keep-alive interval; zero disables keep-alives and the idle timeout.
    #[arg(long, env = "WAR_PING_INTERVAL_SECS", default_value_t = 15)]
    pub ping_interval_secs: u64,
    /// QUIC idle timeout; a connection silent this long ends without a close code.
    #[arg(long, env = "WAR_PONG_TIMEOUT_SECS", default_value_t = 45)]
    pub pong_timeout_secs: u64,
    /// Joined sessions with no application input/output expire; zero disables.
    #[arg(long, env = "WAR_IDLE_TIMEOUT_SECS", default_value_t = 0)]
    pub idle_timeout_secs: u64,
    #[arg(long, env = "WAR_SHUTDOWN_TIMEOUT_SECS", default_value_t = 10)]
    pub shutdown_timeout_secs: u64,
    /// Disable the ability of re_globl members to broadcast to all users.
    #[arg(long, env = "WAR_DISABLE_GLOBAL_BROADCAST")]
    pub disable_global_broadcast: bool,
    #[arg(long, env = "WAR_WORKERS")]
    pub workers: Option<usize>,
    /// Pin data workers one per CPU from the process's allowed mask. `auto` pins
    /// WebTransport workers when there is exactly one allowed CPU per worker.
    #[arg(long, env = "WAR_PIN_WORKERS", value_enum, default_value_t = PinWorkers::Auto,
          num_args = 0..=1, require_equals = true, default_missing_value = "on")]
    pub pin_workers: PinWorkers,
    #[arg(skip)]
    pub auth_password: Option<String>,
    #[arg(skip)]
    pub admin_token: Option<String>,
}

impl Default for Config {
    /// Built-in defaults only. Unlike [`Config::parse`], this never reads the
    /// process environment, so embedded relays and tests are reproducible
    /// regardless of any `WAR_*` variables set in the calling shell.
    fn default() -> Self {
        let command = Self::command().mut_args(|arg| arg.env(None::<&str>));
        Self::from_arg_matches(&command.get_matches_from(["war-relay"]))
            .expect("built-in defaults parse")
    }
}

impl Config {
    /// Load UTF-8 environment secrets without formatting their contents.
    /// Missing or empty values disable the corresponding secret. On encoding
    /// failure, report the variable name and leave both existing fields intact.
    pub fn load_secrets(&mut self) -> Result<()> {
        fn read(name: &str) -> Result<Option<String>> {
            match std::env::var(name) {
                Ok(value) => Ok((!value.is_empty()).then_some(value)),
                Err(std::env::VarError::NotPresent) => Ok(None),
                Err(std::env::VarError::NotUnicode(_)) => {
                    bail!("{name} must contain valid UTF-8")
                }
            }
        }
        let auth_password = read("WAR_AUTH_PASSWORD")?;
        let admin_token = read("WAR_ADMIN_TOKEN")?;
        self.auth_password = auth_password;
        self.admin_token = admin_token;
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        self.origin_policy.validate()?;
        ensure!(
            self.receive_arena_page_bytes > 0
                && self.receive_arena_page_bytes <= isize::MAX as usize
                && self.receive_arena_bytes <= isize::MAX as usize
                && (self.receive_arena_bytes == 0
                    || self.receive_arena_bytes >= self.receive_arena_page_bytes),
            "receive-arena-bytes must be zero or at least receive-arena-page-bytes; both must fit the address space"
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
            self.total_read_buffer_bytes <= isize::MAX as usize,
            "total-read-buffer-bytes exceeds the addressable range"
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
        ensure!(
            self.admin_token.as_ref().is_none_or(|token| {
                // HTTP field values permit HTAB, visible bytes and obs-text.
                // HTTP/1 strips trailing optional whitespace before comparison.
                token
                    .bytes()
                    .all(|byte| byte == b'\t' || !byte.is_ascii_control())
                    && !token.ends_with(' ')
                    && !token.ends_with('\t')
            }),
            "WAR_ADMIN_TOKEN must be an HTTP header value without trailing space or tab"
        );
        Ok(())
    }

    pub fn write_timeout(&self) -> Duration {
        Duration::from_secs(self.write_timeout_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admin_token_bytes_follow_http_field_rules_without_restricting_relay_passwords() {
        for byte in 0u8..=127 {
            let value = format!("left{}right", char::from(byte));
            let representable = axum::http::HeaderValue::from_bytes(value.as_bytes()).is_ok();
            let config = Config {
                admin_token: Some(value.clone()),
                ..Config::default()
            };
            assert_eq!(
                config.validate().is_ok(),
                representable,
                "HTTP field byte {byte}"
            );
            Config {
                auth_password: Some(value),
                ..Config::default()
            }
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn admin_tokens_cannot_end_in_http_optional_whitespace() {
        for value in ["fixture ", "fixture\t"] {
            let error = Config {
                admin_token: Some(value.into()),
                ..Config::default()
            }
            .validate()
            .unwrap_err();
            assert!(error.to_string().contains("WAR_ADMIN_TOKEN"));
            assert!(!error.to_string().contains("fixture"));
        }
        for value in [
            "fixture",
            "two words",
            "inside\ttab",
            " \tleading",
            "unicode-π-世界",
            "trailing-\u{a0}",
        ] {
            Config {
                admin_token: Some(value.into()),
                ..Config::default()
            }
            .validate()
            .unwrap();
        }
    }

    #[test]
    fn pin_workers_defaults_to_auto_and_keeps_the_bare_flag() {
        let parse = |args: &[&str]| {
            Config::try_parse_from(["war-relay"].iter().chain(args))
                .map(|config| config.pin_workers)
                .ok()
        };
        assert_eq!(Config::default().pin_workers, PinWorkers::Auto);
        assert_eq!(parse(&["--pin-workers"]), Some(PinWorkers::On));
        assert_eq!(
            parse(&["--pin-workers", "--workers", "2"]),
            Some(PinWorkers::On)
        );
        assert_eq!(parse(&["--pin-workers=true"]), Some(PinWorkers::On));
        assert_eq!(parse(&["--pin-workers=off"]), Some(PinWorkers::Off));
        assert_eq!(parse(&["--pin-workers=sometimes"]), None);
        let unpinned = |pin_workers, workers| {
            Config {
                pin_workers,
                ..Config::default()
            }
            .worker_cores(workers)
            .unwrap()
            .is_none()
        };
        assert!(unpinned(PinWorkers::Off, 1));
        // Auto never claims fewer CPUs than it was given, and never fails.
        let allowed = core_affinity::get_core_ids().map_or(0, |cores| cores.len());
        if allowed > 1 {
            assert!(unpinned(PinWorkers::Auto, allowed - 1));
            assert!(unpinned(PinWorkers::Auto, allowed + 1));
            assert!(
                Config {
                    pin_workers: PinWorkers::On,
                    ..Config::default()
                }
                .worker_cores(allowed + 1)
                .is_err()
            );
        }
    }

    #[test]
    fn receive_arena_has_a_separate_backing_limit_and_page_hint() {
        let config = Config::try_parse_from([
            "war-relay",
            "--receive-arena-bytes",
            "131072",
            "--receive-arena-page-bytes",
            "32768",
        ])
        .unwrap();
        config.validate().unwrap();
        assert_eq!(config.receive_arena_bytes, 131072);
        assert_eq!(config.receive_arena_page_bytes, 32768);
        for (bytes, page) in [(1, 2), (0, 0), (1, usize::MAX)] {
            assert!(
                Config {
                    receive_arena_bytes: bytes,
                    receive_arena_page_bytes: page,
                    ..Config::default()
                }
                .validate()
                .is_err()
            );
        }
    }
}
