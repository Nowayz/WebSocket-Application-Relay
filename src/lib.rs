//! WebTransport Application Relay: wire protocol, bounded routing, and service runtime.
mod admin {
    //! HTTP management for the relay.
    use std::{num::NonZeroUsize, sync::Arc};

    use axum::{
        Router,
        extract::{Path, Query, State, rejection::PathRejection},
        http::{HeaderMap, StatusCode, Uri},
        response::{IntoResponse, Response},
        routing::{get, post},
    };
    use base64::{Engine, engine::general_purpose::STANDARD};
    use bytes::{BufMut, BytesMut};
    use serde::Deserialize;
    use tracing::info;

    use crate::{Relay, protocol, relay::MAX_CHANNEL_PAGE_SIZE};

    #[derive(Deserialize)]
    struct ChannelQuery {
        limit: Option<NonZeroUsize>,
        after: Option<String>,
    }

    fn cursor_key(cursor: &str) -> Result<[u8; 24], StatusCode> {
        let invalid = StatusCode::BAD_REQUEST;
        if cursor.len() > 24 {
            return Err(invalid);
        }
        let mut raw = [0; 18];
        let len = STANDARD
            .decode_slice(cursor, &mut raw)
            .map_err(|_| invalid)?;
        if len > 16 {
            return Err(invalid);
        }
        let mut key = [0; 24];
        key[..cursor.len()].copy_from_slice(cursor.as_bytes());
        Ok(key)
    }

    pub fn admin_router(relay: Arc<Relay>) -> Router {
        Router::new()
            .route("/healthz", get(|| async { "ok\n" }))
            .route(
                "/readyz",
                get(|State(relay): State<Arc<Relay>>| async move {
                    if relay.is_draining() {
                        StatusCode::SERVICE_UNAVAILABLE
                    } else {
                        StatusCode::OK
                    }
                }),
            )
            .route("/metrics", get(metrics))
            .route("/v1/channels", get(channels))
            .route("/v1/drain", post(drain))
            .route("/v1/resume", post(resume))
            .route("/v1/sessions/{id}/disconnect", post(disconnect))
            .with_state(relay)
    }

    fn authorized(relay: &Relay, headers: &HeaderMap, metrics: bool) -> bool {
        if metrics && relay.config.admin_token.is_none() {
            return true;
        }
        headers
            .get("authorization")
            .and_then(|h| h.as_bytes().strip_prefix(b"Bearer "))
            .is_some_and(|s| protocol::secret_matches(relay.config.admin_token.as_deref(), s))
    }

    async fn metrics(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> Response {
        if !authorized(&relay, &headers, true) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let (sessions, channels) = relay.counts();
        (
            [("content-type", "text/plain; version=0.0.4; charset=utf-8")],
            relay
                .metrics
                .render(sessions, channels, relay.is_draining()),
        )
            .into_response()
    }

    async fn channels(State(relay): State<Arc<Relay>>, headers: HeaderMap, uri: Uri) -> Response {
        if !authorized(&relay, &headers, false) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let channels = if uri.query().is_some_and(|query| !query.is_empty()) {
            // Authorize before parsing, so malformed queries expose no inspection
            // behavior to callers without the management token.
            let Query(query) = match Query::<ChannelQuery>::try_from_uri(&uri) {
                Ok(query) => query,
                Err(error) => return error.into_response(),
            };
            match query.limit {
                Some(limit) if limit.get() <= MAX_CHANNEL_PAGE_SIZE => {
                    let after = match query.after.as_deref().map(cursor_key).transpose() {
                        Ok(after) => after,
                        Err(status) => return status.into_response(),
                    };
                    relay.channel_page(limit, after)
                }
                Some(_) => {
                    return (StatusCode::BAD_REQUEST, "limit must be in 1..=1000").into_response();
                }
                None if query.after.is_some() => {
                    return (StatusCode::BAD_REQUEST, "after requires limit").into_response();
                }
                None => relay.channel_listing(),
            }
        } else {
            relay.channel_listing()
        };
        // A normal bounded-name row fits in about 128 bytes. This is a capacity
        // hint, not a limit: escaped names and large counters may grow the buffer.
        // Snapshot and sort-key storage do not borrow the registry during encoding.
        let capacity = channels.len().saturating_mul(128).saturating_add(2);
        let mut buffer = BytesMut::with_capacity(capacity).writer();
        match serde_json::to_writer(&mut buffer, &channels) {
            Ok(()) => (
                [("content-type", "application/json")],
                buffer.into_inner().freeze(),
            )
                .into_response(),
            Err(error) => (StatusCode::INTERNAL_SERVER_ERROR, error.to_string()).into_response(),
        }
    }

    async fn drain(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> StatusCode {
        set_drain(relay, headers, true)
    }
    async fn resume(State(relay): State<Arc<Relay>>, headers: HeaderMap) -> StatusCode {
        set_drain(relay, headers, false)
    }

    fn set_drain(relay: Arc<Relay>, headers: HeaderMap, draining: bool) -> StatusCode {
        if !authorized(&relay, &headers, false) {
            return StatusCode::UNAUTHORIZED;
        }
        if !relay.set_draining(draining) {
            return StatusCode::CONFLICT;
        }
        info!(draining, "administrator changed admission state");
        StatusCode::NO_CONTENT
    }

    async fn disconnect(
        State(relay): State<Arc<Relay>>,
        headers: HeaderMap,
        id: Result<Path<u64>, PathRejection>,
    ) -> Response {
        if !authorized(&relay, &headers, false) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        let Path(id) = match id {
            Ok(id) => id,
            Err(error) => return error.into_response(),
        };
        let status = if relay.disconnect(id) {
            info!(id, "administrator disconnected session");
            StatusCode::NO_CONTENT
        } else {
            StatusCode::NOT_FOUND
        };
        status.into_response()
    }

    #[cfg(test)]
    mod tests {
        include!("tests/admin.rs");
    }
}
mod common {
    //! Transport-independent relay IDs, listener flags, and close reasons.
    pub const BROADCAST: u64 = u64::MAX;
    pub const RELAY: u64 = 0;
    pub const GLOBAL_CHANNEL: &[u8] = b"re_globl";
    pub const CHANNEL_MESSAGE: u8 = 1;
    pub const PRIVATE_MESSAGE: u8 = 2;
    pub const DISCONNECT_MESSAGE: u8 = 4;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub struct Disconnect {
        pub code: u16,
        pub reason: &'static str,
    }

    impl Disconnect {
        pub const PROTOCOL: Self = Self {
            code: 1002,
            reason: "Protocol Violation",
        };
        pub const CHANNEL_LENGTH: Self = Self {
            code: 1002,
            reason: "Channel Length Exceeded",
        };
        pub const OVERLOADED: Self = Self {
            code: 1013,
            reason: "Try Again Later",
        };
        pub const TAKEN: Self = Self {
            code: 4001,
            reason: "UserID Taken",
        };
        pub const SHUTDOWN: Self = Self {
            code: 1001,
            reason: "Server Shutdown",
        };
        pub const IDLE: Self = Self {
            code: 1001,
            reason: "Application Idle",
        };
        pub const RESERVED: Self = Self {
            code: 1008,
            reason: "Reserved UserID",
        };
    }

    pub fn secret_matches(expected: Option<&str>, actual: &[u8]) -> bool {
        use subtle::ConstantTimeEq;
        expected.is_some_and(|secret| bool::from(secret.as_bytes().ct_eq(actual)))
    }
}
pub mod config {
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
        #[arg(long, env = "WAR_MAX_VARIABLE_BYTES", default_value_t = 1024 * 1024)]
        pub max_variable_bytes: usize,
        #[arg(long, env = "WAR_TOTAL_VARIABLE_BYTES", default_value_t = 64 * 1024 * 1024)]
        pub total_variable_bytes: usize,
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
            for (name, value) in [
                ("queue-messages", self.queue_messages),
                ("max-variable-bytes", self.max_variable_bytes),
            ] {
                ensure!(
                    value > 0 && value <= u32::MAX as usize,
                    "{name} must be in 1..=4294967295"
                );
            }
            // Records carry a u32 length; the relay adds up to 10 bytes of routing header.
            ensure!(
                (17..=u32::MAX as usize - 10).contains(&self.max_message_bytes),
                "max-message-bytes must be in 17..=4294967285"
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
        include!("tests/config.rs");
    }
}
mod frame {
    //! An outgoing message keeps its small routing prefix separate from its body.
    //!
    //! Receive storage becomes immutable before publication. Queue clones share the
    //! body, and the server writes the slices without assembling another message.
    use bytes::{Buf, Bytes};
    use minicbor::{Encoder, encode::write::Cursor};

    #[derive(Clone)]
    pub(crate) struct Frame {
        pub(crate) unreliable: bool,
        prefix: [u8; 14],
        prefix_len: u8,
        payload: Bytes,
    }

    impl Frame {
        fn encoded(payload: Bytes) -> Self {
            Self {
                unreliable: false,
                prefix: [0; 14],
                prefix_len: 0,
                payload,
            }
        }

        fn prefixed(payload: Bytes, encode: impl FnOnce(&mut Encoder<Cursor<&mut [u8]>>)) -> Self {
            let mut frame = Self::encoded(payload);
            let mut encoder = Encoder::new(Cursor::new(frame.prefix.as_mut_slice()));
            encode(&mut encoder);
            frame.prefix_len = encoder.writer().position() as u8;
            frame
        }

        /// Welcome IDs fit completely in the existing inline prefix.
        pub(crate) fn integer(value: u64) -> Self {
            Self::prefixed(Bytes::new(), |e| {
                e.u64(value).expect("uint64 fits in nine bytes");
            })
        }

        /// Wrap an already encoded value without copying its body.
        pub(crate) fn envelope(sender: u64, value: Bytes) -> Self {
            Self::prefixed(value, |e| {
                e.array(2)
                    .and_then(|e| e.u64(sender))
                    .expect("array and uint64 fit in ten bytes");
            })
        }

        pub(crate) fn reply(op: u8, value: Bytes) -> Self {
            Self::prefixed(value, |e| {
                e.array(2)
                    .and_then(|e| e.u8(0))
                    .and_then(|e| e.array(2))
                    .and_then(|e| e.u8(op))
                    .expect("control prefix fits in five bytes");
            })
        }

        pub(crate) fn reply_bool(op: u8, value: bool) -> Self {
            Self::reply(
                op,
                Bytes::from_static(if value { b"\xf5" } else { b"\xf4" }),
            )
        }

        pub(crate) fn reply_uint(op: u8, value: u64) -> Self {
            // The longest control prefix plus uint64 is exactly fourteen bytes.
            let mut frame = Self::reply(op, Bytes::new());
            let mut e = Encoder::new(Cursor::new(frame.prefix.as_mut_slice()));
            e.writer_mut().set_position(usize::from(frame.prefix_len));
            e.u64(value)
                .expect("control prefix and uint64 fit in fourteen bytes");
            frame.prefix_len = e.writer().position() as u8;
            frame
        }

        pub(crate) fn variable(value: Option<Bytes>) -> Self {
            let found = value.is_some();
            Self::prefixed(value.unwrap_or_else(|| Bytes::from_static(b"\xf6")), |e| {
                e.array(2)
                    .and_then(|e| e.u8(0))
                    .and_then(|e| e.array(2))
                    .and_then(|e| e.u8(200))
                    .and_then(|e| e.array(2))
                    .and_then(|e| e.bool(found))
                    .expect("variable prefix fits in seven bytes");
            })
        }

        /// `value_start` comes from validation of this complete CBOR envelope.
        pub(crate) fn routed(sender: u64, mut input: Bytes, value_start: usize) -> Self {
            // Transfer the input owner directly into the body. Advancing this
            // owned view needs no additional shared-buffer reference.
            input.advance(value_start);
            Self::envelope(sender, input)
        }

        pub(crate) fn prefix(&self) -> &[u8] {
            &self.prefix[..usize::from(self.prefix_len)]
        }

        #[cfg(test)]
        pub(crate) fn has_unique_payload(&self) -> bool {
            self.payload.is_unique()
        }

        pub(crate) fn payload(&self) -> &[u8] {
            &self.payload
        }

        pub(crate) fn shared_payload(&self) -> Bytes {
            self.payload.clone()
        }

        pub(crate) fn len(&self) -> usize {
            usize::from(self.prefix_len) + self.payload.len()
        }

        #[cfg(test)]
        pub(crate) fn to_vec(&self) -> Vec<u8> {
            [self.prefix(), self.payload()].concat()
        }
    }

    #[cfg(test)]
    mod tests {
        include!("tests/frame.rs");
    }
}
pub mod metrics {
    use crate::read_budget::ReadBudget;
    use std::{
        fmt::Write,
        sync::{
            Arc,
            atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
        },
    };

    #[derive(Default)]
    pub struct Metrics {
        pub webtransport_endpoint: std::sync::OnceLock<Arc<wtransport::quinn::Endpoint>>,
        pub datagrams_received: AtomicU64,
        pub datagrams_submitted: AtomicU64,
        pub datagram_bytes_submitted: AtomicU64,
        pub datagrams_dropped: AtomicU64,
        pub datagrams_oversized: AtomicU64,
        pub read_budget: Option<Arc<ReadBudget>>,
        pub receive_arena_budget: Option<Arc<ReadBudget>>,
        pub accepted: AtomicU64,
        pub rejected: AtomicU64,
        pub handshake_errors: AtomicU64,
        pub joined: AtomicU64,
        pub disconnected: AtomicU64,
        pub incoming_messages: AtomicU64,
        pub incoming_bytes: AtomicU64,
        pub deliveries: AtomicU64,
        pub delivered_bytes: AtomicU64,
        pub slow_consumers: AtomicU64,
        pub idle_timeouts: AtomicU64,
        /// Exact global admission reservations, also exposed as a gauge. Includes
        /// in-flight messages and charged fanout membership snapshots.
        pub queued_bytes: AtomicUsize,
        pub connections: AtomicUsize,
        pub variable_bytes: AtomicUsize,
        pub global_pressure: AtomicU64,
        pub write_timeouts: AtomicU64,
        pub write_errors: AtomicU64,
        pub origin_rejected: AtomicU64,
        pub output_queue_capacity_bytes: AtomicUsize,
    }

    // Build immutable exposition text at compile time; only values need formatting.
    macro_rules! sample {
        ($name:literal, $kind:literal, $value:expr, $help:literal $(,)?) => {
            (
                concat!(
                    "# HELP war_",
                    $name,
                    " ",
                    $help,
                    "\n# TYPE war_",
                    $name,
                    " ",
                    $kind,
                    "\nwar_",
                    $name,
                    " "
                ),
                $value,
            )
        };
    }

    impl Metrics {
        pub fn render(&self, sessions: usize, channels: usize, draining: bool) -> String {
            // Reserve for the common exposition. Wider values or future metrics
            // can grow the buffer normally; this is a capacity hint, not a limit.
            let mut text = String::with_capacity(8192);
            let budget = self.read_budget.as_deref();
            for (prefix, value) in [
                sample!(
                    "receive_arena_reserved_bytes",
                    "gauge",
                    self.receive_arena_budget
                        .as_ref()
                        .map_or(0, |b| b.reserved()) as u64,
                    "Backing bytes held by reliable receive arena pages, including worker cache and published messages."
                ),
                sample!(
                    "receive_arena_peak_reserved_bytes",
                    "gauge",
                    self.receive_arena_budget.as_ref().map_or(0, |b| b.peak()) as u64,
                    "Peak reliable receive arena backing reservation."
                ),
                sample!(
                    "quic_connections",
                    "gauge",
                    self.webtransport_endpoint
                        .get()
                        .map_or(0, |endpoint| endpoint.open_connections())
                        as u64,
                    "Open QUIC connections reported independently by the transport endpoint."
                ),
                sample!(
                    "datagrams_received",
                    "counter",
                    self.datagrams_received.load(Relaxed),
                    "Unreliable application datagrams received from QUIC."
                ),
                sample!(
                    "datagrams_submitted",
                    "counter",
                    self.datagrams_submitted.load(Relaxed),
                    "Datagrams accepted by local QUIC output; not confirmed remote delivery."
                ),
                sample!(
                    "datagram_bytes_submitted",
                    "counter",
                    self.datagram_bytes_submitted.load(Relaxed),
                    "Application datagram bytes accepted by local QUIC output; not confirmed remote delivery."
                ),
                sample!(
                    "datagrams_dropped",
                    "counter",
                    self.datagrams_dropped.load(Relaxed),
                    "Application datagram outputs dropped by size, pressure or transport error; excludes network losses and QUIC queue eviction."
                ),
                sample!(
                    "datagrams_oversized",
                    "counter",
                    self.datagrams_oversized.load(Relaxed),
                    "Datagram outputs larger than the recipient negotiated maximum."
                ),
                sample!(
                    "read_buffer_reserved_bytes",
                    "gauge",
                    budget.map_or(0, ReadBudget::reserved) as u64,
                    "Receive staging allowance reserved for records in progress, including two-times growth headroom; excludes published payloads and allocator overhead."
                ),
                sample!(
                    "read_buffer_peak_reserved_bytes",
                    "gauge",
                    budget.map_or(0, ReadBudget::peak) as u64,
                    "Peak successfully reserved receive staging allowance."
                ),
                sample!(
                    "read_buffer_limit_bytes",
                    "gauge",
                    budget.map_or(0, ReadBudget::limit) as u64,
                    "Shared receive staging allowance; zero disables admission."
                ),
                sample!(
                    "read_buffer_reservations_rejected_total",
                    "counter",
                    budget.map_or(0, ReadBudget::rejected),
                    "Record staging reservations refused before allocation."
                ),
            ] {
                text.push_str(prefix);
                writeln!(text, "{value}").expect("String write");
            }
            for (prefix, value) in [
                sample!(
                    "idle_timeouts_total",
                    "counter",
                    self.idle_timeouts.load(Relaxed),
                    "Joined sessions closed by the configured application idle timeout."
                ),
                sample!(
                    "write_timeouts_total",
                    "counter",
                    self.write_timeouts.load(Relaxed),
                    "Stream output operations exceeding their deadline."
                ),
                sample!(
                    "write_errors_total",
                    "counter",
                    self.write_errors.load(Relaxed),
                    "Stream output operations failing with transport errors."
                ),
                sample!(
                    "global_pressure_total",
                    "counter",
                    self.global_pressure.load(Relaxed),
                    "Output reservations refused by the process-wide byte budget."
                ),
                sample!(
                    "accepted_total",
                    "counter",
                    self.accepted.load(Relaxed),
                    "Accepted QUIC connections, including failed handshakes."
                ),
                sample!(
                    "rejected_total",
                    "counter",
                    self.rejected.load(Relaxed),
                    "Connections or joins refused by browser-origin policy or drain."
                ),
                sample!(
                    "origin_rejected_total",
                    "counter",
                    self.origin_rejected.load(Relaxed),
                    "WebTransport session requests refused by the configured browser-origin policy; also included in rejected_total."
                ),
                sample!(
                    "handshake_errors_total",
                    "counter",
                    self.handshake_errors.load(Relaxed),
                    "Failed or timed out QUIC or WebTransport handshakes."
                ),
                sample!(
                    "joined_total",
                    "counter",
                    self.joined.load(Relaxed),
                    "Successful channel subscriptions."
                ),
                sample!(
                    "disconnected_total",
                    "counter",
                    self.disconnected.load(Relaxed),
                    "Removed channel subscriptions."
                ),
                sample!(
                    "incoming_messages_total",
                    "counter",
                    self.incoming_messages.load(Relaxed),
                    "Received data messages after subscription."
                ),
                sample!(
                    "incoming_bytes_total",
                    "counter",
                    self.incoming_bytes.load(Relaxed),
                    "Received data bytes after subscription."
                ),
                sample!(
                    "deliveries_total",
                    "counter",
                    self.deliveries.load(Relaxed),
                    "Data messages successfully written, including relay replies."
                ),
                sample!(
                    "delivered_bytes_total",
                    "counter",
                    self.delivered_bytes.load(Relaxed),
                    "Data bytes successfully written."
                ),
                sample!(
                    "slow_consumers_total",
                    "counter",
                    self.slow_consumers.load(Relaxed),
                    "Recipient queue capacity-close signals, including races with peer-initiated Close."
                ),
            ] {
                text.push_str(prefix);
                writeln!(text, "{value}").expect("String write");
            }
            for (prefix, value) in [
                sample!(
                    "variable_bytes",
                    "gauge",
                    self.variable_bytes.load(Relaxed),
                    "Retained channel variable key and encoded-value bytes."
                ),
                sample!(
                    "connections",
                    "gauge",
                    self.connections.load(Relaxed),
                    "Open transport connections admitted to the relay, including handshakes."
                ),
                sample!(
                    "sessions",
                    "gauge",
                    sessions,
                    "Registered sessions, including those pending close."
                ),
                sample!(
                    "channels",
                    "gauge",
                    channels,
                    "Channels, including the persistent re_globl channel."
                ),
                sample!(
                    "queued_bytes",
                    "gauge",
                    self.queued_bytes.load(Relaxed),
                    "Logical output reservations used for global admission, including in-flight messages and active fanout membership snapshots; shared bodies may be charged per recipient."
                ),
                sample!(
                    "draining",
                    "gauge",
                    usize::from(draining),
                    "Whether admission is disabled."
                ),
                sample!(
                    "output_queue_capacity_bytes",
                    "gauge",
                    self.output_queue_capacity_bytes.load(Relaxed),
                    "Allocated session output queue slots, including unused retained capacity; excludes bodies, allocator overhead and kernel buffers."
                ),
            ] {
                text.push_str(prefix);
                writeln!(text, "{value}").expect("String write");
            }
            text
        }
    }
}
pub mod origin {
    //! Optional browser-origin admission, evaluated once per WebTransport session request.
    use anyhow::{Context, Result, ensure};
    use clap::Args;
    use http::{HeaderMap, Uri, header};

    /// Exact serialized browser origins. Missing Origin remains allowed for native
    /// clients; this browser boundary is not authentication of non-browser clients.
    #[derive(Clone, Default, Args)]
    pub struct OriginPolicy {
        /// Allowed browser Origin (repeat or comma-separate); unset allows all.
        #[arg(
            long = "allowed-origin",
            env = "WAR_ALLOWED_ORIGINS",
            value_delimiter = ','
        )]
        pub allowed: Vec<String>,
    }

    impl OriginPolicy {
        pub fn validate(&self) -> Result<()> {
            ensure!(
                self.allowed.len() <= 64,
                "at most 64 allowed origins are supported"
            );
            for origin in &self.allowed {
                ensure!(
                    !origin.is_empty() && origin.len() <= 1024,
                    "allowed origins must contain 1..=1024 bytes"
                );
                if origin == "null" {
                    continue;
                }
                let uri: Uri = origin
                    .parse()
                    .context("allowed-origin must be http(s)://host[:port] or null")?;
                let authority = uri.authority().context("allowed-origin needs a host")?;
                let scheme = uri
                    .scheme_str()
                    .context("allowed-origin needs http or https")?;
                let suffix = &authority.as_str()[authority.host().len()..];
                ensure!(
                    matches!(scheme, "http" | "https")
                        && !authority.host().is_empty()
                        && !authority.as_str().contains('@')
                        && !authority.host().contains('*')
                        && (suffix.is_empty()
                            || suffix.strip_prefix(':').is_some_and(|port| {
                                !port.is_empty()
                                    && port.bytes().all(|b| b.is_ascii_digit())
                                    && port.parse::<u16>().is_ok()
                            }))
                        && format!("{scheme}://{authority}") == *origin,
                    "allowed-origin must be an exact http(s) origin without credentials, wildcard, path, query or fragment"
                );
            }
            Ok(())
        }

        /// Borrow header bytes and compare to the bounded configuration. No parsing
        /// or allocation is needed per request; no policy work occurs per message.
        pub fn allows(&self, headers: &HeaderMap) -> bool {
            if self.allowed.is_empty() {
                return true;
            }
            let mut origins = headers.get_all(header::ORIGIN).iter();
            let Some(origin) = origins.next() else {
                return true;
            };
            origins.next().is_none()
                && self
                    .allowed
                    .iter()
                    .any(|allowed| allowed.as_bytes() == origin.as_bytes())
        }
    }

    #[cfg(test)]
    mod tests {
        include!("tests/origin.rs");
    }
}
pub mod protocol {
    //! WAR CBOR v1: join with a CBOR string, then send `[target: uint64, value: any]`.
    //!
    //! Only definite-length CBOR is accepted. Values are validated with bounded stack
    //! storage and forwarded verbatim; routing never materializes application objects.
    use bytes::Bytes;
    use minicbor::{Decoder, Encoder, data::Type};

    pub use crate::common::{
        BROADCAST, CHANNEL_MESSAGE, DISCONNECT_MESSAGE, Disconnect, GLOBAL_CHANNEL,
        PRIVATE_MESSAGE, RELAY, secret_matches,
    };
    pub const SUBPROTOCOL: &str = "war.cbor.v1";
    pub const MAX_DEPTH: usize = 64;
    pub const MAX_ITEMS: u64 = 65_536;

    fn bad<T>(_: T) -> Disconnect {
        Disconnect::PROTOCOL
    }

    // Called only for a definite text item established by datatype(). Keep short
    // canonical strings on the existing decoder; no scratch is needed for them.
    fn validate_text(decoder: &mut Decoder<'_>) -> Result<(), Disconnect> {
        let start = decoder.position();
        let input = decoder.input();
        let info = input[start] & 0x1f;
        if info < 24 {
            decoder.str().map_err(bad)?;
            return Ok(());
        }
        let header_len = match info {
            24 => 2,
            25 => 3,
            26 => 5,
            27 => 9,
            _ => return Err(Disconnect::PROTOCOL),
        };
        let body_start = start.checked_add(header_len).ok_or(Disconnect::PROTOCOL)?;
        // Definite string lengths use CBOR's unsigned argument encoding. Translate
        // only the major type in at most nine header bytes, reusing minicbor's
        // length decoder rather than copying or materializing the string body.
        let mut header = [0; 9];
        header[..header_len]
            .copy_from_slice(input.get(start..body_start).ok_or(Disconnect::PROTOCOL)?);
        header[0] = info;
        let length = usize::try_from(Decoder::new(&header[..header_len]).u64().map_err(bad)?)
            .map_err(bad)?;
        let end = body_start.checked_add(length).ok_or(Disconnect::PROTOCOL)?;
        let body = input.get(body_start..end).ok_or(Disconnect::PROTOCOL)?;
        simdutf8::compat::from_utf8(body).map_err(bad)?;
        decoder.set_position(end);
        Ok(())
    }

    /// Validate the next value without recursion or heap allocations.
    ///
    /// Advances `decoder` past that value; following bytes are left for the caller.
    /// [`parse`] additionally rejects bytes after its complete routing envelope.
    pub fn validate_value(decoder: &mut Decoder<'_>) -> Result<(), Disconnect> {
        let mut pending = [0u64; MAX_DEPTH + 1];
        pending[0] = 1;
        let mut depth = 0;
        let mut items = 0;
        loop {
            if pending[depth] == 0 {
                if depth == 0 {
                    return Ok(());
                }
                depth -= 1;
                continue;
            }
            pending[depth] -= 1;
            items += 1;
            if items > MAX_ITEMS {
                return Err(Disconnect::PROTOCOL);
            }
            let children = match decoder.datatype().map_err(bad)? {
                Type::U8 | Type::U16 | Type::U32 | Type::U64 => {
                    decoder.u64().map_err(bad)?;
                    0
                }
                Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Int => {
                    decoder.int().map_err(bad)?;
                    0
                }
                Type::Bytes => {
                    decoder.bytes().map_err(bad)?;
                    0
                }
                Type::String => {
                    validate_text(decoder)?;
                    0
                }
                Type::Array => decoder.array().map_err(bad)?.ok_or(Disconnect::PROTOCOL)?,
                Type::Map => decoder
                    .map()
                    .map_err(bad)?
                    .ok_or(Disconnect::PROTOCOL)?
                    .checked_mul(2)
                    .ok_or(Disconnect::PROTOCOL)?,
                Type::Tag => {
                    decoder.tag().map_err(bad)?;
                    1
                }
                Type::F16 | Type::F32 | Type::F64 => {
                    decoder.f64().map_err(bad)?;
                    0
                }
                Type::Bool => {
                    decoder.bool().map_err(bad)?;
                    0
                }
                Type::Null => {
                    decoder.null().map_err(bad)?;
                    0
                }
                Type::Undefined => {
                    decoder.undefined().map_err(bad)?;
                    0
                }
                Type::Simple => {
                    // minicbor exposes even reserved expanded values. RFC 8949
                    // section 3.3 forbids f8 followed by a value below 32.
                    let extended = decoder.input()[decoder.position()] == 0xf8;
                    let value = decoder.simple().map_err(bad)?;
                    if extended && value < 32 {
                        return Err(Disconnect::PROTOCOL);
                    }
                    0
                }
                _ => return Err(Disconnect::PROTOCOL),
            };
            if children > 0 {
                if depth == MAX_DEPTH
                    || children > MAX_ITEMS - items
                    || children > (decoder.input().len() - decoder.position()) as u64
                {
                    return Err(Disconnect::PROTOCOL);
                }
                depth += 1;
                pending[depth] = children;
            }
        }
    }

    /// Validate a complete join message and borrow its channel-name bytes.
    ///
    /// Accepts a CBOR text or byte string of at most 16 bytes, without trailing data.
    /// The returned slice shares the input's lifetime; no channel buffer is allocated.
    pub fn join(bytes: &[u8]) -> Result<&[u8], Disconnect> {
        let mut d = Decoder::new(bytes);
        let channel = string_bytes(&mut d)?;
        if d.position() != bytes.len() {
            return Err(Disconnect::PROTOCOL);
        }
        if channel.len() > 16 {
            return Err(Disconnect::CHANNEL_LENGTH);
        }
        Ok(channel)
    }

    /// Validate a complete `[target, value]` envelope and borrow its encoded value.
    ///
    /// The returned value slice includes its original CBOR header and shares the
    /// input's lifetime. Validation does not allocate, materialize application
    /// objects or reencode their bytes; trailing data is rejected.
    pub fn parse(bytes: &[u8]) -> Result<(u64, &[u8]), Disconnect> {
        let mut d = Decoder::new(bytes);
        if d.array().map_err(bad)? != Some(2) {
            return Err(Disconnect::PROTOCOL);
        }
        let target = d.u64().map_err(bad)?;
        let start = d.position();
        validate_value(&mut d)?;
        if d.position() != bytes.len() {
            return Err(Disconnect::PROTOCOL);
        }
        Ok((target, &bytes[start..]))
    }

    fn string_bytes<'a>(d: &mut Decoder<'a>) -> Result<&'a [u8], Disconnect> {
        match d.datatype().map_err(bad)? {
            Type::String => d.str().map(str::as_bytes).map_err(bad),
            Type::Bytes => d.bytes().map_err(bad),
            _ => Err(Disconnect::PROTOCOL),
        }
    }

    /// Build a contiguous owned envelope containing `sender` and an encoded value.
    ///
    /// `value` must contain exactly one valid definite-length CBOR value. This helper
    /// copies those encoded bytes into the result without validating or transcoding
    /// them. Cloning the returned [`Bytes`] shares its buffer. Relay forwarding
    /// instead keeps the routing prefix separate and borrows the shared value body.
    pub fn envelope(sender: u64, value: &[u8]) -> Bytes {
        let mut e = Encoder::new(Vec::with_capacity(1 + uint_len(sender) + value.len()));
        e.array(2)
            .and_then(|e| e.u64(sender))
            .expect("Vec encoding");
        let mut bytes = e.into_writer();
        bytes.extend_from_slice(value);
        bytes.into()
    }

    /// Definite array headers use the same argument width as unsigned integers.
    pub(crate) fn uint_len(value: u64) -> usize {
        match value {
            0..=23 => 1,
            24..=255 => 2,
            256..=65_535 => 3,
            65_536..=4_294_967_295 => 5,
            _ => 9,
        }
    }

    /// Encode an unsigned integer into an owned CBOR value.
    pub fn uint(value: u64) -> Bytes {
        let mut e = Encoder::new(Vec::with_capacity(9));
        e.u64(value).expect("Vec encoding");
        e.into_writer().into()
    }

    /// Copy raw bytes into an owned definite-length CBOR byte string.
    ///
    /// The input is the string's contents, not a preencoded CBOR value. The result
    /// owns its storage; later clones share the encoded buffer.
    pub fn byte_string(value: &[u8]) -> Bytes {
        // Exact storage lets Bytes defer its shared-ownership allocation until cloning.
        let mut e = Encoder::new(Vec::with_capacity(
            uint_len(value.len() as u64) + value.len(),
        ));
        e.bytes(value).expect("Vec encoding");
        e.into_writer().into()
    }

    /// Copy UTF-8 text into an owned definite-length CBOR text string.
    ///
    /// The CBOR length counts UTF-8 bytes. The result does not borrow `value`;
    /// later clones share the encoded buffer.
    pub fn text(value: &str) -> Bytes {
        let mut e = Encoder::new(Vec::with_capacity(
            uint_len(value.len() as u64) + value.len(),
        ));
        e.str(value).expect("Vec encoding");
        e.into_writer().into()
    }

    /// Build an owned control reply, `[0, [op, value]]`.
    ///
    /// `value` must already contain exactly one valid definite-length CBOR value.
    /// Its bytes are copied verbatim, without validation or transcoding, into the
    /// contiguous result. Cloning the result shares its buffer.
    pub fn reply(op: u8, value: &[u8]) -> Bytes {
        let mut e = Encoder::new(Vec::with_capacity(
            3 + uint_len(u64::from(op)) + value.len(),
        ));
        e.array(2)
            .and_then(|e| e.u64(0))
            .and_then(|e| e.array(2))
            .and_then(|e| e.u8(op))
            .expect("Vec encoding");
        let mut bytes = e.into_writer();
        bytes.extend_from_slice(value);
        bytes.into()
    }

    /// Build an owned relay command with the same encoding contract as [`reply`].
    pub fn request(op: u8, value: &[u8]) -> Bytes {
        reply(op, value)
    }

    pub enum Command<'a> {
        Authenticate(&'a [u8]),
        Listen(u8),
        Channels,
        Assign(u64),
        SetVariable { key: &'a [u8], value: &'a [u8] },
        GetVariable(&'a [u8]),
    }

    pub fn command(bytes: &[u8]) -> Result<Command<'_>, Disconnect> {
        let mut d = Decoder::new(bytes);
        if d.array().map_err(bad)? != Some(2) {
            return Err(Disconnect::PROTOCOL);
        }
        let op = d.u8().map_err(bad)?;
        let command = match op {
            0 => Command::Authenticate(string_bytes(&mut d)?),
            1 => Command::Listen(d.u8().map_err(bad)?),
            2 => {
                d.null().map_err(bad)?;
                Command::Channels
            }
            3 => Command::Assign(d.u64().map_err(bad)?),
            4 => {
                if d.array().map_err(bad)? != Some(2) {
                    return Err(Disconnect::PROTOCOL);
                }
                let key = string_bytes(&mut d)?;
                if key.is_empty() || key.len() > 255 {
                    return Err(Disconnect::PROTOCOL);
                }
                let start = d.position();
                validate_value(&mut d)?;
                Command::SetVariable {
                    key,
                    value: &bytes[start..d.position()],
                }
            }
            5 => {
                let key = string_bytes(&mut d)?;
                if key.is_empty() || key.len() > 255 {
                    return Err(Disconnect::PROTOCOL);
                }
                Command::GetVariable(key)
            }
            _ => return Err(Disconnect::PROTOCOL),
        };
        if d.position() != bytes.len() {
            return Err(Disconnect::PROTOCOL);
        }
        Ok(command)
    }

    #[cfg(test)]
    mod tests {
        include!("tests/protocol.rs");
    }
}
pub mod read_budget {
    //! Shared admission for receive staging, independent of published message owners.
    use std::sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
    };

    /// A process-wide staging allowance. This is not an allocator or an RSS limit.
    ///
    /// Records in progress reserve twice their declared size, covering old and new
    /// staging allocations while growing. Published messages are owned by their
    /// consumers and are outside this allowance.
    #[derive(Debug)]
    pub struct ReadBudget {
        limit: usize,
        reserved: AtomicUsize,
        peak: AtomicUsize,
        rejected: AtomicU64,
    }

    /// The process-wide read budget is exhausted.
    #[derive(Debug)]
    pub struct ReadBudgetExceeded;

    impl std::fmt::Display for ReadBudgetExceeded {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("read buffer budget exceeded")
        }
    }

    impl std::error::Error for ReadBudgetExceeded {}

    impl ReadBudget {
        /// Create an empty budget. A zero limit admits only zero-sized leases.
        pub fn new(limit: usize) -> Arc<Self> {
            Arc::new(Self {
                limit,
                reserved: AtomicUsize::new(0),
                peak: AtomicUsize::new(0),
                rejected: AtomicU64::new(0),
            })
        }

        /// Reserve before allocating. Dropping the lease releases its allowance.
        pub fn try_lease(self: &Arc<Self>, bytes: usize) -> Result<ReadLease, ReadBudgetExceeded> {
            let mut current = self.reserved.load(Relaxed);
            loop {
                let Some(next) = current.checked_add(bytes).filter(|&n| n <= self.limit) else {
                    self.rejected.fetch_add(1, Relaxed);
                    return Err(ReadBudgetExceeded);
                };
                match self
                    .reserved
                    .compare_exchange_weak(current, next, Relaxed, Relaxed)
                {
                    Ok(_) => {
                        self.peak.fetch_max(next, Relaxed);
                        return Ok(ReadLease {
                            budget: self.clone(),
                            reserved: bytes,
                        });
                    }
                    Err(observed) => current = observed,
                }
            }
        }

        /// Configured allowance in bytes.
        pub fn limit(&self) -> usize {
            self.limit
        }

        /// Current reserved staging allowance, including growth headroom.
        pub fn reserved(&self) -> usize {
            self.reserved.load(Relaxed)
        }

        /// Largest successfully reserved allowance.
        pub fn peak(&self) -> usize {
            self.peak.load(Relaxed)
        }

        /// Failed admission attempts.
        pub fn rejected(&self) -> u64 {
            self.rejected.load(Relaxed)
        }
    }

    /// One non-clonable staging reservation.
    #[derive(Debug)]
    pub struct ReadLease {
        budget: Arc<ReadBudget>,
        reserved: usize,
    }

    impl Drop for ReadLease {
        fn drop(&mut self) {
            self.budget.reserved.fetch_sub(self.reserved, Relaxed);
        }
    }

    #[cfg(test)]
    mod tests {
        include!("tests/read_budget.rs");
    }
}
mod receive_arena {
    //! Bounded slabs shared by received records; published slices retain their slab.
    use crate::read_budget::{ReadBudget, ReadLease};
    use bytes::{Bytes, BytesMut};
    use std::{cell::RefCell, sync::Arc};

    thread_local! {
        static PAGE: RefCell<Option<Page>> = const { RefCell::new(None) };
        static SPARE: RefCell<Option<Page>> = const { RefCell::new(None) };
    }

    pub(crate) struct ReceiveArena {
        pub(crate) budget: Arc<ReadBudget>,
        page_bytes: usize,
    }

    struct Page {
        budget: Arc<ReadBudget>,
        storage: BytesMut,
        lease: Arc<PageLease>,
    }

    struct PageLease {
        _reservation: ReadLease,
        budget: Arc<ReadBudget>,
        bytes: usize,
    }

    pub(crate) struct ArenaBuffer {
        storage: BytesMut,
        _lease: Arc<PageLease>,
    }

    impl AsRef<[u8]> for ArenaBuffer {
        fn as_ref(&self) -> &[u8] {
            &self.storage
        }
    }

    impl Drop for ArenaBuffer {
        fn drop(&mut self) {
            // The last slice can recover the whole allocation without touching live views.
            let _ = SPARE.try_with(|slot| {
                let Ok(mut spare) = slot.try_borrow_mut() else {
                    return;
                };
                if spare.is_none()
                    && self
                        .storage
                        .try_reclaim(self._lease.bytes - self.storage.len())
                {
                    self.storage.resize(self._lease.bytes, 0);
                    *spare = Some(Page {
                        budget: self._lease.budget.clone(),
                        storage: std::mem::take(&mut self.storage),
                        lease: self._lease.clone(),
                    });
                }
            });
        }
    }

    impl ArenaBuffer {
        pub(crate) fn as_mut(&mut self) -> &mut [u8] {
            &mut self.storage
        }

        pub(crate) fn publish(self) -> Bytes {
            Bytes::from_owner(self)
        }
    }

    impl ReceiveArena {
        pub(crate) fn new(bytes: usize, page_bytes: usize) -> Self {
            Self {
                budget: ReadBudget::new(bytes),
                page_bytes,
            }
        }

        pub(crate) fn allocate(&self, size: usize) -> Option<ArenaBuffer> {
            if size == 0 || size > self.page_bytes || self.budget.limit() == 0 {
                return None;
            }
            PAGE.with(|slot| {
                let mut cached = slot.borrow_mut();
                if cached
                    .as_ref()
                    .is_some_and(|p| !Arc::ptr_eq(&p.budget, &self.budget))
                {
                    *cached = None;
                }
                if let Some(page) = cached.as_mut()
                    && page.storage.len() < size
                    && page
                        .storage
                        .try_reclaim(self.page_bytes - page.storage.len())
                {
                    page.storage.resize(self.page_bytes, 0);
                }
                if cached.as_ref().is_none_or(|p| p.storage.len() < size) {
                    // Release an exhausted tail before testing the process-wide cap.
                    *cached = None;
                    *cached = SPARE.with(|slot| slot.borrow_mut().take()).filter(|p| {
                        Arc::ptr_eq(&p.budget, &self.budget) && p.storage.len() == self.page_bytes
                    });
                    if cached.is_none() {
                        let lease = self.budget.try_lease(self.page_bytes).ok()?;
                        *cached = Some(Page {
                            budget: self.budget.clone(),
                            storage: BytesMut::zeroed(self.page_bytes),
                            lease: Arc::new(PageLease {
                                _reservation: lease,
                                budget: self.budget.clone(),
                                bytes: self.page_bytes,
                            }),
                        });
                    }
                }
                let page = cached.as_mut().expect("allocated page");
                Some(ArenaBuffer {
                    storage: page.storage.split_to(size),
                    _lease: page.lease.clone(),
                })
            })
        }
    }

    #[cfg(test)]
    mod tests {
        include!("tests/receive_arena.rs");
    }
}
pub mod relay {
    //! Routing registry with fine-grained locking.
    //!
    //! No single lock covers the whole relay. Session ownership is a lock-free
    //! concurrent map, each channel owns its own membership lock and departure
    //! queue, global observers are an atomically swapped snapshot, and admission
    //! counters are atomics. No socket write ever happens while a registry lock is
    //! held: output is either submitted after the guard drops or queued for the
    //! owning worker.
    use std::{
        collections::{HashMap, VecDeque},
        hash::{DefaultHasher, Hash, Hasher},
        sync::{
            Arc, OnceLock, Weak,
            atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering::Relaxed},
        },
    };

    use anyhow::{Context, Result};
    use arc_swap::{ArcSwap, ArcSwapOption};
    use base64::{Engine, engine::general_purpose::STANDARD};
    use bytes::Bytes;
    use crossbeam_queue::SegQueue;
    use minicbor::Encoder;
    use parking_lot::{Mutex, RwLock};
    use serde::Serialize;
    #[cfg(test)]
    use tokio::sync::{mpsc, watch};

    use crate::{
        Config,
        frame::Frame,
        metrics::Metrics,
        protocol::{
            self, BROADCAST, CHANNEL_MESSAGE, Command, DISCONNECT_MESSAGE, Disconnect,
            GLOBAL_CHANNEL, PRIVATE_MESSAGE,
        },
    };

    mod listing {
        use base64::{Engine, engine::general_purpose::STANDARD};
        use std::num::NonZeroUsize;

        use super::{ChannelSnapshot, MAX_CHANNEL_PAGE_SIZE, Relay};

        impl Relay {
            /// Select bounded batches from a lock-free walk of the index: a channel
            /// present throughout the walk is seen once, one created or retired
            /// meanwhile may or may not appear. Each partition discards rows that
            /// cannot enter the smallest page. Scratch space holds at most two pages
            /// of keys and owned metadata, never peer/value bodies.
            pub(crate) fn channel_page(
                &self,
                limit: NonZeroUsize,
                after: Option<[u8; 24]>,
            ) -> Vec<ChannelSnapshot> {
                let limit = limit.get().min(MAX_CHANNEL_PAGE_SIZE);
                let channels = self.live_channels();
                let capacity = (2 * limit).min(channels.len());
                let mut selected = Vec::new();
                for channel in &channels {
                    // Cursor filtering needs only the borrowed name's sort key.
                    // Construct owned row metadata only after that filter succeeds.
                    let mut key = [0; 24];
                    STANDARD
                        .encode_slice(&channel.name, &mut key)
                        .expect("16-byte channel names fit in 24 base64 bytes");
                    if after.is_some_and(|after| key <= after) {
                        continue;
                    }
                    let Some(snapshot) = ChannelSnapshot::of(channel) else {
                        continue;
                    };
                    if selected.is_empty() {
                        selected.reserve_exact(capacity);
                    }
                    selected.push((key, snapshot));
                    // Keep one page of candidates and make room for the next batch.
                    if selected.len() == 2 * limit {
                        selected.select_nth_unstable_by_key(limit, |row| row.0);
                        selected.truncate(limit);
                    }
                }
                // Ordering and output conversion need no registry guard.
                selected.sort_unstable_by_key(|row| row.0);
                selected.truncate(limit);
                selected.into_iter().map(|(_, snapshot)| snapshot).collect()
            }
        }
    }
    mod messages {
        //! Large observer publications are owned and advanced by their source worker.
        //! The index is a lock-free map used only for registration, retirement and
        //! revocation; worker turns lock one publication, never any shared structure,
        //! and direct (unqueued) deliveries never touch the index at all: each
        //! observer re-checks the revocation mark under its own output lock.
        use super::*;

        type ObserverView = Vec<Arc<Peer>>;

        pub(super) struct MessageFanouts {
            /// Numbers publications; a barrier inspects them in ascending order.
            sequence: AtomicU64,
            /// Live large observer publications by number. Registration, retirement
            /// and a barrier's walk never wait on each other. Pre-sized so that it
            /// does not resize in ordinary operation; a walk would otherwise help
            /// finish an in-progress resize first.
            jobs: papaya::HashMap<u64, Weak<Mutex<RoomFanout>>>,
        }

        impl Default for MessageFanouts {
            fn default() -> Self {
                Self {
                    sequence: AtomicU64::new(0),
                    jobs: papaya::HashMap::with_capacity(4096),
                }
            }
        }

        pub(super) struct Publication {
            registry: Arc<MessageFanouts>,
            work: Arc<Mutex<RoomFanout>>,
            sequence: Option<u64>,
        }

        impl Publication {
            pub(super) fn new(registry: Arc<MessageFanouts>) -> Self {
                Self {
                    registry,
                    work: Arc::new(Mutex::new(RoomFanout::default())),
                    sequence: None,
                }
            }

            pub(super) fn begin(
                &mut self,
                source: &Peer,
                recipients: MessageRecipients,
                frame: Frame,
                generation: u64,
            ) -> Result<(), Disconnect> {
                debug_assert!(!self.active());
                // The recorded generation covers a capture that raced a revocation
                // and registers only after its scan: the job then skips the observer.
                let sequence = self
                    .registry
                    .sequence
                    .fetch_update(Relaxed, Relaxed, |sequence| sequence.checked_add(1))
                    .map_err(|_| Disconnect::OVERLOADED)?
                    + 1;
                let mut work = self.work.lock();
                work.begin(source, FanoutRecipients::Message(recipients), frame, 0)?;
                work.generation = generation;
                drop(work);
                self.registry
                    .jobs
                    .pin()
                    .insert(sequence, Arc::downgrade(&self.work));
                self.sequence = Some(sequence);
                Ok(())
            }

            pub(super) fn active(&self) -> bool {
                self.sequence.is_some()
            }

            pub(super) fn detach(&mut self) {
                self.work.lock().tolerant = true;
            }

            pub(super) fn resume(&mut self, quantum: usize) -> Result<(), RoutingFailure> {
                let Some(sequence) = self.sequence else {
                    return Ok(());
                };
                let mut work = self.work.lock();
                let result = work.resume_with(quantum, Some(sequence));
                if result.is_err() {
                    work.clear();
                }
                let finished = !work.active();
                drop(work);
                if finished {
                    self.registry.jobs.pin().remove(&sequence);
                    self.sequence = None;
                }
                result
            }

            pub(super) fn clear(&mut self) {
                self.work.lock().clear();
                if let Some(sequence) = self.sequence.take() {
                    self.registry.jobs.pin().remove(&sequence);
                }
            }
        }

        impl Drop for Publication {
            fn drop(&mut self) {
                self.clear();
            }
        }

        impl MessageFanouts {
            /// Collect the observer's pending captured messages and advance its
            /// watermarks. The caller submits them after releasing every lock. The
            /// revoked generation is stored before the caller enqueues the
            /// acknowledgement, and direct deliveries re-check it under the
            /// observer's own output lock, so none of them can follow the ACK.
            pub(super) fn flush(&self, observer: &Peer, revoked_at: u64) -> Vec<Frame> {
                let mut pending = Vec::new();
                let previous = observer.message_barrier.load(Relaxed);
                // The map is lock-free and unordered: collect, then inspect in ascending
                // sequence. That order is essential, since a watermark must never
                // suppress a lower-sequence job that this barrier has not inspected
                // yet. A job registered during the walk may be missed; its generation
                // then makes it skip the observer, as a capture concurrent with the
                // revocation may.
                let mut jobs: Vec<(u64, Weak<Mutex<RoomFanout>>)> = self
                    .jobs
                    .pin()
                    .iter()
                    .filter(|(sequence, _)| **sequence > previous)
                    .map(|(sequence, job)| (*sequence, job.clone()))
                    .collect();
                jobs.sort_unstable_by_key(|(sequence, _)| *sequence);
                let mut searched: Option<(Weak<ObserverView>, Option<usize>)> = None;
                for (sequence, weak) in jobs {
                    let Some(job) = weak.upgrade() else {
                        continue;
                    };
                    let work = job.lock();
                    // Captured after the revocation: the observer is not in its view.
                    if work.generation > revoked_at {
                        continue;
                    }
                    if let (Some(FanoutRecipients::Message(message)), Some(packet)) =
                        (&work.recipients, &work.packet)
                    {
                        let view = &message.observers;
                        let position = match &searched {
                            Some((old, position)) if old.as_ptr() == Arc::as_ptr(view) => *position,
                            _ => {
                                let position =
                                    view.iter().position(|peer| std::ptr::eq(&**peer, observer));
                                // A weak identity avoids keeping the handle buffer alive
                                // after a completing job has returned its queue credit.
                                searched = Some((Arc::downgrade(view), position));
                                position
                            }
                        };
                        if position.is_some_and(|position| {
                            message.room().len().saturating_add(position) >= work.cursor
                        }) && observer.key != message.excluded_observer
                        {
                            pending.push(packet.frame.clone());
                        }
                    }
                    // Store while this job is locked: its source cannot deliver this
                    // message to the observer once the watermark is visible.
                    observer.message_barrier.store(sequence, Relaxed);
                }
                observer
                    .message_barrier
                    .store(self.sequence.load(Relaxed), Relaxed);
                // A publication captured before the revocation but registered after
                // this walk skips the observer instead of arriving after its ACK.
                observer.message_revoked.store(revoked_at, Relaxed);
                pending
            }

            #[cfg(test)]
            pub(super) fn jobs_len(&self) -> usize {
                self.jobs.pin().len()
            }

            #[cfg(test)]
            pub(super) fn set_sequence(&self, sequence: u64) {
                self.sequence.store(sequence, Relaxed);
            }
        }

        #[cfg(test)]
        mod tests {
            include!("tests/relay_messages.rs");
        }
    }
    use messages::{MessageFanouts, Publication};

    pub(crate) const FANOUT_QUANTUM: usize = 1024;
    // Bound ordinary synchronous removal by the same visit budget as native
    // retirement progress, while keeping live message fanout independently sized.
    pub(crate) const RETIREMENT_QUANTUM: usize = 128;
    /// Jobs a revocation barrier inspects per hold of one observer shard's lock.
    const BARRIER_QUANTUM: usize = 128;
    /// Turns a synchronous fallback runs per hold of one departure queue's lock.
    const FLUSH_TURNS_PER_HOLD: usize = 8;

    pub(crate) const MAX_CHANNEL_PAGE_SIZE: usize = 1000;

    /// An immutable recipient view shared between the cache and in-flight work.
    type Recipients = Arc<Vec<Arc<Peer>>>;

    #[repr(u8)]
    enum Admission {
        Open,
        Draining,
        Stopping,
    }

    /// The relay. Every field is independently synchronized; see the module notes.
    pub struct Relay {
        pub config: Config,
        pub metrics: Arc<Metrics>,
        pub(crate) receive_arena: crate::receive_arena::ReceiveArena,
        admission: AtomicU8,
        users: Users,
        /// Lock-free name-to-channel index. Lookup never blocks; creation and
        /// retirement are single-key writes that never block a reader.
        channels: papaya::HashMap<Bytes, Arc<Channel>>,
        /// Live channels, including the global room. Counted before a channel is
        /// inserted, so a concurrent reader never sees more channels than counted.
        channel_count: AtomicUsize,
        /// Incremented on every membership change. A global view built against an
        /// older generation is used once and never cached.
        membership_generation: AtomicU64,
        global_view: ArcSwapOption<(u64, Recipients)>,
        /// Immutable snapshot, swapped atomically. Routing loads it without
        /// locking; a flag change or an observer departure publishes the next
        /// snapshot with a compare-and-swap loop, so writers never lock either.
        listeners: ArcSwap<Listeners>,
        /// Keyed by listener generation, so a stale snapshot is never reused.
        disconnect_snapshot: ArcSwapOption<(u64, Weak<RetirementSnapshot>)>,
        retirements: Retirements,
        /// Disconnect-observer notices and `re_globl` room departures: both reach
        /// `re_globl` members. Spread over shards by the departing session's
        /// room, so departures and worker turns take one shard's lock while a
        /// barrier scans every shard.
        observer_departures: [Arc<DepartureQueue>; OBSERVER_QUEUE_SHARDS],
        /// Live fanouts whose source retired before they finished.
        detached: Arc<DepartureQueue>,
        messages: Arc<MessageFanouts>,
        sessions: AtomicUsize,
        next_key: AtomicU64,
    }

    #[derive(Default)]
    struct InputTotals {
        messages: u64,
        bytes: u64,
    }

    /// One worker publishes cumulative input statistics at its ready-batch boundary.
    /// Admission and retained-byte gauges remain immediate, independent atomics.
    pub(crate) struct ReceiveBatch<'a> {
        relay: &'a Relay,
        totals: InputTotals,
    }

    /// Routing errors retain attempted recipient work for native worker fairness.
    /// Validation and preparation refusals count as one attempt, never a full turn.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(crate) struct RoutingFailure {
        pub reason: Disconnect,
        pub work: usize,
    }

    impl RoutingFailure {
        fn visited(reason: Disconnect, work: usize) -> Self {
            Self {
                reason,
                work: work.max(1),
            }
        }

        fn after(mut self, preceding: usize) -> Self {
            self.work = self.work.saturating_add(preceding);
            self
        }
    }

    impl From<Disconnect> for RoutingFailure {
        fn from(reason: Disconnect) -> Self {
            Self::visited(reason, 1)
        }
    }

    impl ReceiveBatch<'_> {
        #[cfg(test)]
        pub(crate) fn receive(&mut self, peer: &Peer, bytes: Bytes) -> Result<usize, Disconnect> {
            self.relay
                .receive_with_totals::<false>(peer, bytes, &mut self.totals, None)
                .map_err(|error| error.reason)
        }

        /// Workers keep one reusable continuation slot per large publisher.
        pub(crate) fn receive_bounded(
            &mut self,
            peer: &Peer,
            bytes: Bytes,
            pending: &mut Option<Box<RoomFanout>>,
            quantum: usize,
        ) -> Result<usize, RoutingFailure> {
            self.relay.receive_with_totals::<false>(
                peer,
                bytes,
                &mut self.totals,
                Some((pending, quantum)),
            )
        }

        pub(crate) fn receive_datagram(
            &mut self,
            peer: &Peer,
            bytes: Bytes,
            pending: &mut Option<Box<RoomFanout>>,
            quantum: usize,
        ) -> Result<usize, RoutingFailure> {
            self.relay.receive_with_totals::<true>(
                peer,
                bytes,
                &mut self.totals,
                Some((pending, quantum)),
            )
        }

        pub(crate) fn publish(&mut self) {
            let totals = std::mem::take(&mut self.totals);
            let metrics = &self.relay.metrics;
            for (counter, value) in [
                (&metrics.incoming_messages, totals.messages),
                (&metrics.incoming_bytes, totals.bytes),
            ] {
                if value != 0 {
                    counter.fetch_add(value, Relaxed);
                }
            }
        }
    }

    impl Drop for ReceiveBatch<'_> {
        fn drop(&mut self) {
            self.publish();
        }
    }

    // ---------------------------------------------------------------------------
    // Session ownership: lock-free ID -> peer map.

    /// Every operation is a single-key operation on a lock-free concurrent map,
    /// so routing never waits for a join, a departure or a takeover. An ID
    /// reassignment publishes the new key before it retires the old one, so a
    /// route that still finds the old key during that step reaches the session
    /// that owned it; a route begun after the assignment reply uses the new key.
    ///
    /// The map holds weak handles. A lock-free map reclaims removed entries
    /// lazily, and a session's socket and budgets must be released by its own
    /// connection the moment it leaves, never by the map's reclamation schedule.
    struct Users {
        map: papaya::HashMap<u64, Weak<Peer>>,
    }

    /// Session and channel maps are pre-sized so ordinary growth never resizes
    /// them; a resize is the one operation that can make another thread's map
    /// operation help copy entries before it proceeds.
    const PRESIZED_ENTRIES: usize = 1 << 16;

    impl Users {
        fn new() -> Self {
            Self {
                map: papaya::HashMap::with_capacity(PRESIZED_ENTRIES),
            }
        }

        fn get(&self, id: u64) -> Option<Arc<Peer>> {
            self.map.pin().get(&id).and_then(Weak::upgrade)
        }

        fn try_insert(&self, id: u64, peer: &Arc<Peer>) -> bool {
            self.map.pin().try_insert(id, Arc::downgrade(peer)).is_ok()
        }

        /// Point `id` at `peer` while `key` still owns it or its previous owner
        /// is gone. Returns false when another session owns the ID now.
        fn replace(&self, id: u64, key: u64, peer: &Arc<Peer>) -> bool {
            use papaya::{Compute, Operation};
            let map = self.map.pin();
            let outcome = map.compute(id, |entry| match entry {
                Some((_, owner)) if owner.upgrade().is_some_and(|owner| owner.key != key) => {
                    Operation::Abort(())
                }
                _ => Operation::Insert(Arc::downgrade(peer)),
            });
            !matches!(outcome, Compute::Aborted(()))
        }

        /// Remove `id` only while `key` still owns it. A displaced session must
        /// never erase the new owner of its previous ID.
        fn remove_owned(&self, id: u64, key: u64) {
            let _ = self.map.pin().remove_if(&id, |_, owner| {
                owner.upgrade().is_none_or(|owner| owner.key == key)
            });
        }

        /// Move ownership of `old` to `new`. Returns the displaced owner of `new`,
        /// or `None` when `peer` no longer owns `old`.
        fn reassign(&self, peer: &Peer, old: u64, new: u64) -> Option<Option<Arc<Peer>>> {
            let map = self.map.pin();
            let owner = map
                .get(&old)
                .and_then(Weak::upgrade)
                .filter(|owner| owner.key == peer.key)?;
            let displaced = map
                .insert(new, Arc::downgrade(&owner))
                .and_then(Weak::upgrade);
            // Only this session's own entry retires: another takeover of `old`
            // in the meantime already replaced it.
            let _ = map.remove_if(&old, |_, current| {
                current
                    .upgrade()
                    .is_none_or(|current| current.key == peer.key)
            });
            Some(displaced)
        }

        #[cfg(test)]
        fn len(&self) -> usize {
            self.map.pin().len()
        }
    }

    // ---------------------------------------------------------------------------
    // Channels.

    /// One room. Membership, variables and the cached recipient view share one
    /// lock; departure jobs have their own queue so a large room's notifications
    /// never delay another room's.
    pub(crate) struct Channel {
        pub(crate) name: Bytes,
        state: RwLock<ChannelState>,
        departures: Arc<DepartureQueue>,
    }

    #[derive(Default)]
    struct ChannelState {
        members: HashMap<u64, Arc<Peer>>,
        // Membership metadata only. Uncaptured views update in place; a membership
        // change invalidates a captured view while its routing owners finish.
        broadcast: OnceLock<Arc<Vec<Arc<Peer>>>>,
        variables: HashMap<Bytes, Bytes>,
        variable_bytes: usize,
        /// Set once the empty room leaves the index. A joiner that raced the
        /// retirement creates a fresh room instead of resurrecting this one.
        retired: bool,
    }

    impl Channel {
        fn new(name: Bytes) -> Arc<Self> {
            Arc::new(Self {
                name,
                state: RwLock::new(ChannelState::default()),
                departures: Arc::new(DepartureQueue::default()),
            })
        }

        #[cfg(test)]
        pub(crate) fn cached_view(&self) -> Option<Arc<Vec<Arc<Peer>>>> {
            self.state.read().broadcast.get().cloned()
        }

        #[cfg(test)]
        pub(crate) fn members_len(&self) -> usize {
            self.state.read().members.len()
        }

        #[cfg(test)]
        pub(crate) fn members_capacity(&self) -> usize {
            self.state.read().members.capacity()
        }

        #[cfg(test)]
        pub(crate) fn member(&self, key: u64) -> Option<Arc<Peer>> {
            self.state.read().members.get(&key).cloned()
        }

        #[cfg(test)]
        pub(crate) fn variable(&self, key: &[u8]) -> Option<Bytes> {
            self.state.read().variables.get(key).cloned()
        }

        #[cfg(test)]
        pub(crate) fn departure_queue(&self) -> &Arc<DepartureQueue> {
            &self.departures
        }
    }

    impl ChannelState {
        fn insert(&mut self, peer: Arc<Peer>) {
            if let Some(view) = self.broadcast.get_mut().and_then(Arc::get_mut) {
                // The write lock excludes new captures. A unique view has no
                // routing owner, so retain its allocation across churn.
                view.push(peer.clone());
            } else {
                // Active broadcasts keep their immutable membership snapshot.
                // Rebuild lazily rather than copying a view still in use.
                self.broadcast.take();
            }
            self.members.insert(peer.key, peer);
        }

        fn remove(&mut self, key: u64) -> bool {
            if self.members.remove(&key).is_none() {
                return false;
            }
            if let Some(view) = self.broadcast.get_mut().and_then(Arc::get_mut) {
                let index = view
                    .iter()
                    .position(|peer| peer.key == key)
                    .expect("recipient view matches channel membership");
                view.swap_remove(index);
                // Bound spare handles in a retained room after a large wave.
                if view.capacity() > 256 && view.len() <= view.capacity() / 8 {
                    view.shrink_to(view.len() * 2);
                }
            } else {
                self.broadcast.take();
            }
            true
        }

        fn view(&self) -> Arc<Vec<Arc<Peer>>> {
            self.broadcast
                .get_or_init(|| Arc::new(self.members.values().cloned().collect()))
                .clone()
        }
    }

    // ---------------------------------------------------------------------------
    // Global observers.

    /// An immutable snapshot of the global observers. Routing loads it without
    /// locking; a listener-flag change publishes a new snapshot with the next
    /// generation. Every observer job records the generation it captured and a
    /// revocation records on the observer the generation it revoked at, so a job
    /// captured at or before that generation skips the observer. That replaces
    /// the lock a departure would otherwise hold from capture to enqueue.
    struct Listeners {
        generation: u64,
        /// Advances only when the disconnect-observer list changes, so a shared
        /// disconnect view survives changes to the message classes.
        disconnect_generation: u64,
        // Only authenticated global members opting into each class belong here.
        // Routing visits eligible listeners, independent of the idle population.
        lists: [Vec<Arc<Peer>>; 3],
        message_snapshots: [OnceLock<Recipients>; 2],
    }

    impl Listeners {
        fn empty(generation: u64) -> Self {
            Self {
                generation,
                disconnect_generation: generation,
                lists: Default::default(),
                message_snapshots: Default::default(),
            }
        }

        fn of(&self, flag: u8) -> &[Arc<Peer>] {
            &self.lists[flag.trailing_zeros() as usize]
        }

        fn mask(&self) -> u8 {
            self.lists
                .iter()
                .enumerate()
                .filter(|(_, list)| !list.is_empty())
                .fold(0, |mask, (bit, _)| mask | (1 << bit))
        }

        /// The next snapshot after one member's flags change from `previous` to
        /// `flags`. Cached message views survive for classes that did not change.
        fn with_flags(
            &self,
            peer: &Peer,
            previous: u8,
            flags: u8,
            member: Option<&Arc<Peer>>,
        ) -> Self {
            let mut lists = self.lists.clone();
            let message_snapshots: [OnceLock<Recipients>; 2] = Default::default();
            for (bit, list) in lists.iter_mut().enumerate() {
                let mask = 1 << bit;
                if (previous ^ flags) & mask == 0 {
                    if bit < 2
                        && let Some(view) = self.message_snapshots[bit].get()
                    {
                        let _ = message_snapshots[bit].set(view.clone());
                    }
                    continue;
                }
                if flags & mask != 0 {
                    if let Some(member) = member {
                        list.push(member.clone());
                    }
                } else if let Some(position) = list.iter().position(|p| p.key == peer.key) {
                    list.swap_remove(position);
                }
            }
            Self {
                generation: self.generation + 1,
                disconnect_generation: if (previous ^ flags) & DISCONNECT_MESSAGE != 0 {
                    self.disconnect_generation + 1
                } else {
                    self.disconnect_generation
                },
                lists,
                message_snapshots,
            }
        }

        fn message_snapshot(&self, flag: u8) -> Arc<Vec<Arc<Peer>>> {
            self.message_snapshots[flag.trailing_zeros() as usize]
                .get_or_init(|| Arc::new(self.of(flag).to_vec()))
                .clone()
        }
    }

    // ---------------------------------------------------------------------------
    // Deferred departure work.

    /// FIFO of bounded jobs for one recipient population. Each room has one, the
    /// global observers share one, and orphaned live fanouts have one.
    /// Observer-notice queues: departures and worker turns spread across them,
    /// a barrier scans all of them. Chosen by the departing session's room.
    const OBSERVER_QUEUE_SHARDS: usize = 16;

    #[derive(Default)]
    pub(crate) struct DepartureQueue {
        state: Mutex<QueueState>,
        /// Index among the observer shards; zero for room and detached queues,
        /// whose jobs carry no observer watermark.
        shard: usize,
    }

    #[derive(Default)]
    struct QueueState {
        jobs: VecDeque<Box<RoomFanout>>,
        // Never reset when the queue empties: peers retain their barrier watermark.
        sequence: u64,
        /// Whether this queue is currently listed for worker progress.
        queued: bool,
    }

    impl DepartureQueue {
        fn shard(index: usize) -> Self {
            Self {
                state: Mutex::default(),
                shard: index,
            }
        }

        #[cfg(test)]
        pub(crate) fn len(&self) -> usize {
            self.state.lock().jobs.len()
        }

        #[cfg(test)]
        pub(crate) fn sequence(&self) -> u64 {
            self.state.lock().sequence
        }

        #[cfg(test)]
        pub(crate) fn set_sequence(&self, sequence: u64) {
            self.state.lock().sequence = sequence;
        }

        #[cfg(test)]
        pub(crate) fn with_jobs<T>(&self, f: impl FnOnce(&VecDeque<Box<RoomFanout>>) -> T) -> T {
            f(&self.state.lock().jobs)
        }
    }

    /// Queues with pending work, in round-robin order. This is a lock-free queue:
    /// workers pop an entry and push it back while it has work, departures append
    /// one entry when a queue becomes non-empty, and nobody ever waits on it.
    #[derive(Default)]
    struct Retirements {
        active: SegQueue<Arc<DepartureQueue>>,
    }

    /// Shared disconnect membership is charged once while any departure retains it.
    /// The relay holds only a weak cache; the last job releases both handles and
    /// credit. Jobs and recipient output retain their independent reservations.
    struct RetirementSnapshot {
        members: Vec<Arc<Peer>>,
        metrics: Arc<Metrics>,
        bytes: usize,
    }

    impl Drop for RetirementSnapshot {
        fn drop(&mut self) {
            // Release the actual handle buffer before making its credit reusable.
            drop(std::mem::take(&mut self.members));
            self.metrics.queued_bytes.fetch_sub(self.bytes, Relaxed);
        }
    }

    /// How one departure notice reaches the room it leaves.
    enum Departure {
        Direct(Vec<Arc<Peer>>),
        Deferred(Arc<Vec<Arc<Peer>>>),
    }

    enum FanoutRecipients {
        Room(Arc<Vec<Arc<Peer>>>),
        Disconnect(Arc<RetirementSnapshot>),
        Message(MessageRecipients),
    }

    struct MessageRecipients {
        room: Option<Arc<Vec<Arc<Peer>>>>,
        observers: Arc<Vec<Arc<Peer>>>,
        excluded_observer: u64,
    }

    fn membership_bytes(capacity: usize) -> Option<usize> {
        capacity
            .checked_mul(std::mem::size_of::<Arc<Peer>>())
            .and_then(|bytes| {
                bytes.checked_add(
                    std::mem::size_of::<Vec<Arc<Peer>>>() + 2 * std::mem::size_of::<usize>(),
                )
            })
    }

    impl MessageRecipients {
        fn room(&self) -> &[Arc<Peer>] {
            self.room.as_ref().map_or(&[], |room| room.as_slice())
        }

        fn storage_bytes(&self) -> Option<usize> {
            let room = self
                .room
                .as_ref()
                .map_or(Some(0), |room| membership_bytes(room.capacity()))?;
            room.checked_add(membership_bytes(self.observers.capacity())?)
        }
    }

    impl FanoutRecipients {
        fn single_slice(&self) -> &[Arc<Peer>] {
            match self {
                Self::Room(members) => members,
                Self::Disconnect(snapshot) => &snapshot.members,
                Self::Message(_) => {
                    unreachable!("message publications have two recipient segments")
                }
            }
        }

        fn len(&self) -> usize {
            match self {
                Self::Message(message) => {
                    message.room().len().saturating_add(message.observers.len())
                }
                _ => self.single_slice().len(),
            }
        }
    }

    /// One immutable membership snapshot and one charged shared body. Publisher
    /// slots reuse their box; departure jobs belong to a departure queue. Empty
    /// slots retain no payload or peer ownership.
    #[derive(Default)]
    pub(crate) struct RoomFanout {
        recipients: Option<FanoutRecipients>,
        packet: Option<Envelope>,
        cursor: usize,
        source: u64,
        snapshot_bytes: usize,
        // Zero denotes a live publisher; positive values order departure jobs.
        retirement_sequence: u64,
        /// Jobs in the observer shards honour each observer's revocation watermark.
        checks_barrier: bool,
        /// The observer shard this job is queued in, which selects the watermark.
        shard: usize,
        /// Listener snapshot generation this job captured its observers from.
        generation: u64,
        /// A refused recipient never aborts a job that outlives its source.
        tolerant: bool,
        // Reused only by sources publishing to a large observer audience. The
        // indexed job has no Publication of its own, so ownership is acyclic.
        observed: Option<Publication>,
    }

    impl RoomFanout {
        fn begin(
            &mut self,
            source: &Peer,
            recipients: FanoutRecipients,
            frame: Frame,
            retirement_sequence: u64,
        ) -> Result<(), Disconnect> {
            debug_assert!(!self.active());
            let packet = source.charge_strict(&frame)?.into_owned();
            let job_bytes = if retirement_sequence != 0 {
                std::mem::size_of::<Self>() + 2 * std::mem::size_of::<Box<Self>>()
            } else {
                0
            };
            // Room views remain conservatively charged per publisher. Disconnect
            // snapshots own one shared charge, so jobs reserve only their metadata.
            let snapshot_bytes = match &recipients {
                FanoutRecipients::Room(members) => {
                    membership_bytes(members.capacity()).and_then(|n| n.checked_add(job_bytes))
                }
                FanoutRecipients::Disconnect(_) => Some(job_bytes),
                FanoutRecipients::Message(message) => message
                    .storage_bytes()
                    .and_then(|n| n.checked_add(job_bytes)),
            }
            .ok_or(Disconnect::OVERLOADED)?;
            if packet
                .budget
                .metrics
                .queued_bytes
                .fetch_update(Relaxed, Relaxed, |used| {
                    used.checked_add(snapshot_bytes)
                        .filter(|&next| next <= packet.budget.global_limit)
                })
                .is_err()
            {
                packet.budget.metrics.global_pressure.fetch_add(1, Relaxed);
                return Err(Disconnect::OVERLOADED);
            }
            self.retirement_sequence = retirement_sequence;
            self.checks_barrier = false;
            self.shard = 0;
            self.generation = 0;
            self.tolerant = retirement_sequence != 0;
            self.snapshot_bytes = snapshot_bytes;
            self.packet = Some(packet);
            self.recipients = Some(recipients);
            self.cursor = 0;
            self.source = source.key;
            Ok(())
        }

        pub(crate) fn active(&self) -> bool {
            self.packet.is_some() || self.observed.as_ref().is_some_and(Publication::active)
        }

        pub(crate) fn clear(&mut self) {
            if let Some(observed) = &mut self.observed {
                observed.clear();
            }
            self.clear_local();
        }

        fn clear_local(&mut self) {
            // Captured views can be the final owners after index invalidation.
            // Drop their storage before returning the corresponding queue credit.
            self.recipients = None;
            if self.snapshot_bytes != 0 {
                self.packet
                    .as_ref()
                    .expect("charged membership view")
                    .budget
                    .metrics
                    .queued_bytes
                    .fetch_sub(self.snapshot_bytes, Relaxed);
                self.snapshot_bytes = 0;
            }
            self.packet = None;
        }

        /// Mark a live publication as outliving its source. Remaining recipients
        /// are still visited; refusals no longer have a source to report to.
        fn detach(&mut self) {
            self.tolerant = true;
            if let Some(observed) = &mut self.observed {
                observed.detach();
            }
        }

        pub(crate) fn resume(&mut self, quantum: usize) -> Result<(), Disconnect> {
            self.resume_routing(quantum).map_err(|error| error.reason)
        }

        fn resume_routing(&mut self, quantum: usize) -> Result<(), RoutingFailure> {
            let visits = self.recipients.as_ref().map_or(0, |view| {
                quantum.min(view.len().saturating_sub(self.cursor))
            });
            if self.packet.is_some() {
                self.resume_with(visits, None)?;
            }
            if self.packet.is_none()
                && let Some(observed) = &mut self.observed
            {
                observed
                    .resume(quantum - visits)
                    .map_err(|error| error.after(visits))?;
            }
            Ok(())
        }

        fn resume_with(
            &mut self,
            quantum: usize,
            message_sequence: Option<u64>,
        ) -> Result<(), RoutingFailure> {
            let recipients = self.recipients.as_ref().expect("live fanout snapshot");
            let frame = &self.packet.as_ref().expect("charged fanout body").frame;
            let end = self.cursor.saturating_add(quantum).min(recipients.len());
            let tolerant = self.tolerant;
            if let FanoutRecipients::Message(message) = recipients {
                let room = message.room();
                let room_end = end.min(room.len());
                if self.cursor < room_end {
                    Self::submit(
                        &room[self.cursor..room_end],
                        self.source,
                        frame,
                        None,
                        tolerant,
                    )?;
                }
                let observer_start = self.cursor.saturating_sub(room.len());
                let observer_end = end.saturating_sub(room.len());
                if observer_start < observer_end {
                    Self::submit(
                        &message.observers[observer_start..observer_end],
                        message.excluded_observer,
                        frame,
                        Some((
                            BarrierKind::Message,
                            message_sequence.expect("observer jobs have a publication sequence"),
                            self.generation,
                            0,
                        )),
                        tolerant,
                    )
                    .map_err(|error| error.after(room_end.saturating_sub(self.cursor)))?;
                }
            } else {
                let barrier = self.checks_barrier.then_some((
                    BarrierKind::Retirement,
                    self.retirement_sequence,
                    self.generation,
                    self.shard,
                ));
                Self::submit(
                    &recipients.single_slice()[self.cursor..end],
                    self.source,
                    frame,
                    barrier,
                    tolerant,
                )?;
            }
            self.cursor = end;
            if end == recipients.len() {
                self.clear_local();
            }
            Ok(())
        }

        fn submit(
            recipients: &[Arc<Peer>],
            excluded: u64,
            frame: &Frame,
            barrier: Option<(BarrierKind, u64, u64, usize)>,
            tolerant: bool,
        ) -> Result<(), RoutingFailure> {
            for (index, recipient) in recipients.iter().enumerate() {
                if recipient.key == excluded {
                    continue;
                }
                if let Some((kind, sequence, generation, shard)) = barrier {
                    let (watermark, revoked) = match kind {
                        BarrierKind::Retirement => (
                            recipient
                                .retirement_barriers
                                .as_deref()
                                .map(|barriers| &barriers[shard]),
                            &recipient.retirement_revoked,
                        ),
                        BarrierKind::Message => {
                            (Some(&recipient.message_barrier), &recipient.message_revoked)
                        }
                    };
                    if watermark.is_some_and(|watermark| watermark.load(Relaxed) >= sequence)
                        || revoked.load(Relaxed) >= generation
                    {
                        continue;
                    }
                }
                let result = recipient.enqueue_fanout(frame);
                if !tolerant {
                    result.map_err(|reason| RoutingFailure::visited(reason, index + 1))?;
                }
            }
            Ok(())
        }
    }

    #[derive(Clone, Copy)]
    enum BarrierKind {
        Retirement,
        Message,
    }

    impl Drop for RoomFanout {
        fn drop(&mut self) {
            self.clear();
        }
    }

    fn deliver_room(
        source: &Peer,
        recipients: Arc<Vec<Arc<Peer>>>,
        frame: Frame,
    ) -> Result<usize, RoutingFailure> {
        let work = recipients.len().max(1);
        for (index, recipient) in recipients.iter().enumerate() {
            if recipient.key != source.key {
                recipient
                    .enqueue_fanout(&frame)
                    .map_err(|reason| RoutingFailure::visited(reason, index + 1))?;
            }
        }
        Ok(work)
    }

    // ---------------------------------------------------------------------------
    // Peers and output accounting.

    struct Budget {
        used: AtomicUsize,
        limit: usize,
        global_limit: usize,
        metrics: Arc<Metrics>,
    }

    /// Byte reservations survive dequeue and remain charged until the actual write
    /// completes (or is cancelled). Dropping any queue or failed send releases them.
    pub(crate) struct Envelope {
        pub frame: Frame,
        budget: Arc<Budget>,
    }

    /// A charged synchronous submission borrows its owners. Only deferred output
    /// promotes it to an owned envelope; either form releases the same reservation.
    pub(crate) struct Submission<'a> {
        pub frame: &'a Frame,
        budget: Option<&'a Arc<Budget>>,
    }

    impl Submission<'_> {
        pub(crate) fn into_owned(mut self) -> Envelope {
            let budget = self.budget.take().expect("live submission").clone();
            Envelope {
                frame: self.frame.clone(),
                budget,
            }
        }
    }

    impl Drop for Submission<'_> {
        fn drop(&mut self) {
            if let Some(budget) = self.budget {
                budget.release(self.frame.len());
            }
        }
    }

    impl Budget {
        fn release(&self, bytes: usize) {
            self.used.fetch_sub(bytes, Relaxed);
            self.metrics.queued_bytes.fetch_sub(bytes, Relaxed);
        }
    }

    /// A transport owns ordering and backpressure. Sending never waits for socket
    /// readiness, and close only signals the owning I/O loop.
    pub(crate) trait PeerOutput: Send + Sync {
        fn send(&self, packet: Submission<'_>) -> bool;
        fn close(&self, reason: Disconnect);

        /// Ordered control barriers may drain ready output before reserving a slot.
        fn send_barrier(&self, packet: Submission<'_>) -> bool {
            self.send(packet)
        }

        /// Native workers can advance retirement work independently of the source.
        fn supports_retirement(&self) -> bool {
            false
        }
        fn wake_retirement(&self) {}

        /// Multicast may defer to recipient owners for parallel, batched output.
        /// Simple/test transports retain their ordinary FIFO submission contract.
        fn send_fanout(&self, packet: Submission<'_>) -> bool {
            self.send(packet)
        }

        /// Multicast to an observer whose capture may race a revocation. A
        /// transport driven from several threads evaluates `gate` under the lock
        /// that orders its FIFO and drops the packet when it returns false, so
        /// no such frame can land behind a revocation's acknowledgement. This
        /// default serves simple and test transports that are not driven
        /// concurrently.
        fn send_gated(&self, packet: Submission<'_>, gate: &dyn Fn() -> bool) -> bool {
            if gate() {
                self.send_fanout(packet)
            } else {
                true
            }
        }
    }

    #[cfg(test)]
    struct AsyncOutput {
        outgoing: mpsc::Sender<Envelope>,
        close: watch::Sender<Option<Disconnect>>,
    }

    #[cfg(test)]
    impl PeerOutput for AsyncOutput {
        fn send(&self, packet: Submission<'_>) -> bool {
            self.outgoing.try_send(packet.into_owned()).is_ok()
        }

        fn close(&self, reason: Disconnect) {
            self.close.send_replace(Some(reason));
        }
    }

    impl Drop for Envelope {
        fn drop(&mut self) {
            self.budget.release(self.frame.len());
        }
    }

    pub(crate) struct Peer {
        key: u64,
        pub id: AtomicU64,
        /// Canonical name storage shared with the room.
        pub(crate) channel: Bytes,
        room: Arc<Channel>,
        authenticated: AtomicBool,
        listener: AtomicU8,
        closing: AtomicBool,
        // Watermarks let a barrier submit this observer's pending notices without
        // advancing everyone else's cursors, and prevent replay afterwards. The
        // sequence watermark orders against jobs already queued; the revoked
        // generation also covers a job captured before the revocation but queued
        // after its barrier ran.
        /// One retirement watermark per observer shard, since each shard numbers
        /// its own jobs. Only `re_globl` members can observe, so only they carry
        /// the array.
        retirement_barriers: Option<Box<[AtomicU64; OBSERVER_QUEUE_SHARDS]>>,
        retirement_revoked: AtomicU64,
        message_barrier: AtomicU64,
        message_revoked: AtomicU64,
        output: Arc<dyn PeerOutput>,
        budget: Arc<Budget>,
    }

    impl Peer {
        pub(crate) fn close(&self, reason: Disconnect) -> bool {
            if self.closing.swap(true, Relaxed) {
                return false;
            }
            self.output.close(reason);
            true
        }

        fn is_global(&self) -> bool {
            self.channel.as_ref() == GLOBAL_CHANNEL
        }

        /// Output addressed to this peer by routing: dropped under pressure.
        fn enqueue_frame(&self, frame: &Frame) -> Result<(), Disconnect> {
            self.enqueue::<false, false, false>(frame)
        }

        fn enqueue_fanout(&self, frame: &Frame) -> Result<(), Disconnect> {
            self.enqueue::<true, false, false>(frame)
        }

        /// Multicast to this observer for a capture made under `generation`.
        /// The transport re-checks the revocation mark under its own FIFO lock,
        /// so the frame either precedes a concurrent revocation's acknowledgement
        /// or is dropped. No lock shared with any other session is involved.
        fn enqueue_gated(
            &self,
            frame: &Frame,
            revoked: &AtomicU64,
            generation: u64,
        ) -> Result<(), Disconnect> {
            if revoked.load(Relaxed) >= generation {
                return Ok(());
            }
            let Some(packet) = self.reserve(frame, false)? else {
                return Ok(());
            };
            if !self
                .output
                .send_gated(packet, &|| revoked.load(Relaxed) < generation)
            {
                self.overloaded();
            }
            Ok(())
        }

        fn enqueue_barrier(&self, frame: &Frame) -> Result<(), Disconnect> {
            self.enqueue::<false, true, true>(frame)
        }

        /// A reply to this peer's own command. A peer that cannot even receive
        /// its reply is closed rather than left waiting for one.
        fn enqueue_reply(&self, frame: &Frame) -> Result<(), Disconnect> {
            self.enqueue::<false, false, true>(frame)
        }

        /// The welcome is submitted before the peer is published anywhere, and
        /// with no registry lock held, so it may be written inline like a reply.
        fn enqueue_welcome(&self, frame: &Frame) -> Result<(), Disconnect> {
            self.enqueue::<false, false, true>(frame)
        }

        // Select the output method at compile time. Ordinary private submissions
        // carry no multicast flag, extra branch or different frame representation.
        #[inline(always)]
        fn enqueue<const FANOUT: bool, const BARRIER: bool, const REPLY: bool>(
            &self,
            frame: &Frame,
        ) -> Result<(), Disconnect> {
            let Some(packet) = self.reserve(frame, REPLY)? else {
                return Ok(());
            };
            let accepted = if BARRIER {
                self.output.send_barrier(packet)
            } else if FANOUT {
                self.output.send_fanout(packet)
            } else {
                self.output.send(packet)
            };
            if !accepted {
                self.overloaded();
            }
            Ok(())
        }

        #[inline(always)]
        fn reserve<'a>(
            &'a self,
            frame: &'a Frame,
            reply: bool,
        ) -> Result<Option<Submission<'a>>, Disconnect> {
            if self.closing.load(Relaxed) {
                return Ok(None);
            }
            self.charge(frame, reply)
        }

        /// Reserve output credit for one frame addressed to this peer.
        ///
        /// Per-session exhaustion closes this peer: it is the slow consumer. Global
        /// exhaustion never punishes the sender; the frame is dropped, the pressure
        /// counter advances, and this peer is closed only when it holds a large
        /// share of its own allowance or the frame is its own command reply.
        fn charge<'a>(
            &'a self,
            frame: &'a Frame,
            reply: bool,
        ) -> Result<Option<Submission<'a>>, Disconnect> {
            let size = frame.len();
            if self
                .budget
                .used
                .fetch_update(Relaxed, Relaxed, |used| {
                    used.checked_add(size)
                        .filter(|&next| next <= self.budget.limit)
                })
                .is_err()
            {
                if frame.unreliable {
                    self.budget.metrics.datagrams_dropped.fetch_add(1, Relaxed);
                } else {
                    self.overloaded();
                }
                return Ok(None);
            }
            if self
                .budget
                .metrics
                .queued_bytes
                .fetch_update(Relaxed, Relaxed, |used| {
                    used.checked_add(size)
                        .filter(|&next| next <= self.budget.global_limit)
                })
                .is_err()
            {
                let held = self.budget.used.fetch_sub(size, Relaxed) - size;
                self.budget.metrics.global_pressure.fetch_add(1, Relaxed);
                if frame.unreliable {
                    self.budget.metrics.datagrams_dropped.fetch_add(1, Relaxed);
                } else if reply || held >= self.budget.limit / 4 {
                    self.overloaded();
                }
                return Ok(None);
            }
            Ok(Some(Submission {
                frame,
                budget: Some(&self.budget),
            }))
        }

        /// Staging reservations for deferred work report refusal to the caller,
        /// which then falls back to immediate delivery. Closed sources can still
        /// stage their own departure notice.
        fn charge_strict<'a>(&'a self, frame: &'a Frame) -> Result<Submission<'a>, Disconnect> {
            let size = frame.len();
            if self
                .budget
                .used
                .fetch_update(Relaxed, Relaxed, |used| {
                    used.checked_add(size)
                        .filter(|&next| next <= self.budget.limit)
                })
                .is_err()
            {
                return Err(Disconnect::OVERLOADED);
            }
            if self
                .budget
                .metrics
                .queued_bytes
                .fetch_update(Relaxed, Relaxed, |used| {
                    used.checked_add(size)
                        .filter(|&next| next <= self.budget.global_limit)
                })
                .is_err()
            {
                self.budget.used.fetch_sub(size, Relaxed);
                self.budget.metrics.global_pressure.fetch_add(1, Relaxed);
                return Err(Disconnect::OVERLOADED);
            }
            Ok(Submission {
                frame,
                budget: Some(&self.budget),
            })
        }

        fn overloaded(&self) {
            if self.close(Disconnect::OVERLOADED) {
                self.budget.metrics.slow_consumers.fetch_add(1, Relaxed);
            }
        }
    }

    pub(crate) struct Registration {
        pub peer: Arc<Peer>,
        relay: Arc<Relay>,
    }

    impl Registration {
        /// Hand an unfinished publication to the relay before this session leaves.
        pub(crate) fn detach_fanout(&self, work: Box<RoomFanout>) {
            self.relay.detach_fanout(&self.peer, work);
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            self.relay.remove(&self.peer);
        }
    }

    #[cfg(test)]
    pub(crate) struct Subscription {
        pub registration: Registration,
        pub outgoing: mpsc::Receiver<Envelope>,
        pub close: watch::Receiver<Option<Disconnect>>,
    }

    // ---------------------------------------------------------------------------
    // Inspection types.

    #[derive(Serialize)]
    pub struct ChannelInfo {
        pub name: Option<String>,
        pub name_base64: String,
        pub members: usize,
        pub variables: usize,
        pub variable_bytes: usize,
    }

    /// Registry names are bounded to 16 bytes. Inline inspection names hold small
    /// metadata without per-name allocation or reference-count traffic.
    #[derive(Clone, Copy)]
    struct SnapshotName {
        bytes: [u8; 16],
        len: u8,
    }

    impl SnapshotName {
        fn new(name: &[u8]) -> Self {
            let mut bytes = [0; 16];
            bytes[..name.len()].copy_from_slice(name);
            Self {
                bytes,
                len: name.len() as u8,
            }
        }

        fn as_bytes(&self) -> &[u8] {
            &self.bytes[..usize::from(self.len)]
        }

        fn base64(&self) -> [u8; 24] {
            let mut encoded = [0; 24];
            STANDARD
                .encode_slice(self.as_bytes(), &mut encoded)
                .expect("16-byte channel names fit in 24 base64 bytes");
            encoded
        }
    }

    /// One coherent view owns only small names and numeric metadata. It keeps no
    /// registry guard, peer or variable value alive while inspection formats output.
    #[derive(Clone, Copy)]
    pub(crate) struct ChannelSnapshot {
        name: SnapshotName,
        members: usize,
        variables: usize,
        variable_bytes: usize,
    }

    impl ChannelSnapshot {
        fn of(channel: &Channel) -> Option<Self> {
            let state = channel.state.read();
            // The index walk and this metadata read are separate. A newly
            // published room may not have its first member yet, or the last
            // member may have left after the walk captured its handle. Neither
            // is a live ordinary room in a listing. The permanent global room
            // remains visible even when empty.
            if state.retired
                || (state.members.is_empty() && channel.name.as_ref() != GLOBAL_CHANNEL)
            {
                return None;
            }
            Some(Self {
                name: SnapshotName::new(&channel.name),
                members: state.members.len(),
                variables: state.variables.len(),
                variable_bytes: state.variable_bytes,
            })
        }
    }

    impl Serialize for ChannelSnapshot {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            use serde::ser::SerializeStruct;

            let name = self.name.as_bytes();
            let base64 = self.name.base64();
            let encoded_len = name.len().div_ceil(3) * 4;
            let encoded = std::str::from_utf8(&base64[..encoded_len]).expect("base64 is ASCII");
            let mut row = serializer.serialize_struct("ChannelInfo", 5)?;
            row.serialize_field("name", &std::str::from_utf8(name).ok())?;
            row.serialize_field("name_base64", encoded)?;
            row.serialize_field("members", &self.members)?;
            row.serialize_field("variables", &self.variables)?;
            row.serialize_field("variable_bytes", &self.variable_bytes)?;
            row.end()
        }
    }

    fn shrink_sparse_map<K: Eq + Hash, V>(map: &mut HashMap<K, V>) {
        // HashMap iteration scans spare buckets too. After substantial departure,
        // release excess slots in a room's membership index.
        // Keep small maps and 2x headroom to avoid churn. Rehashing moves metadata;
        // peers, queues, name/variable bytes and shared bodies keep their storage.
        let remaining = map.len();
        let capacity = map.capacity();
        if capacity > 256 && remaining <= capacity / 8 {
            map.shrink_to(remaining * 2);
        }
    }

    // ---------------------------------------------------------------------------
    // The relay.

    impl Relay {
        pub fn new(config: Config) -> Result<Arc<Self>> {
            config.validate()?;
            // Verify entropy availability before listening, never silently use predictable IDs.
            let _ = random_id()?;
            // Pre-size the index up to a modest cap so ordinary growth never
            // resizes it; an iteration would otherwise help finish the resize.
            let channels = papaya::HashMap::with_capacity(PRESIZED_ENTRIES);
            let global = Bytes::from_static(GLOBAL_CHANNEL);
            channels.pin().insert(global.clone(), Channel::new(global));
            let users = Users::new();
            let read_budget = (config.total_read_buffer_bytes > 0)
                .then(|| crate::read_budget::ReadBudget::new(config.total_read_buffer_bytes));
            let receive_arena = crate::receive_arena::ReceiveArena::new(
                config.receive_arena_bytes,
                config.receive_arena_page_bytes,
            );
            Ok(Arc::new(Self {
                config,
                metrics: Arc::new(Metrics {
                    read_budget,
                    receive_arena_budget: Some(receive_arena.budget.clone()),
                    ..Metrics::default()
                }),
                receive_arena,
                admission: AtomicU8::new(Admission::Open as u8),
                users,
                channels,
                channel_count: AtomicUsize::new(1),
                membership_generation: AtomicU64::new(0),
                global_view: ArcSwapOption::empty(),
                listeners: ArcSwap::from_pointee(Listeners::empty(1)),
                disconnect_snapshot: ArcSwapOption::empty(),
                retirements: Retirements::default(),
                observer_departures: std::array::from_fn(|index| {
                    Arc::new(DepartureQueue::shard(index))
                }),
                detached: Arc::new(DepartureQueue::default()),
                messages: Arc::new(MessageFanouts::default()),
                sessions: AtomicUsize::new(0),
                next_key: AtomicU64::new(1),
            }))
        }

        pub fn counts(&self) -> (usize, usize) {
            (
                self.sessions.load(Relaxed),
                self.channel_count.load(Relaxed),
            )
        }

        /// Manual drain and terminal shutdown both refuse new admission.
        pub fn is_draining(&self) -> bool {
            self.admission.load(Relaxed) != Admission::Open as u8
        }

        pub fn is_stopping(&self) -> bool {
            self.admission.load(Relaxed) == Admission::Stopping as u8
        }

        /// Set a reversible drain. Returns false when resuming a terminal shutdown;
        /// draining an already stopping relay remains an idempotent success.
        #[must_use]
        pub fn set_draining(&self, draining: bool) -> bool {
            self.admission
                .fetch_update(Relaxed, Relaxed, |state| {
                    if state == Admission::Stopping as u8 {
                        draining.then_some(state)
                    } else {
                        Some(if draining {
                            Admission::Draining as u8
                        } else {
                            Admission::Open as u8
                        })
                    }
                })
                .is_ok()
        }

        /// Stop admission permanently. `webtransport::serve` calls this when its
        /// shutdown token is cancelled; embedders may call it earlier to refuse
        /// new sessions while established ones finish.
        pub fn begin_shutdown(&self) {
            self.admission.store(Admission::Stopping as u8, Relaxed);
            // Terminal shutdown sends no departure notices. Release listener views
            // once instead of searching them for every retiring global session.
            self.listeners
                .rcu(|current| Arc::new(Listeners::empty(current.generation + 1)));
            self.disconnect_snapshot.store(None);
            self.global_view.store(None);
            while let Some(queue) = self.retirements.active.pop() {
                let mut state = queue.state.lock();
                let jobs = std::mem::take(&mut state.jobs);
                state.queued = false;
                drop(state);
                // Released outside the lock: a long queue's bodies and views
                // are freed without holding up the shard.
                drop(jobs);
            }
        }

        /// Handles to every live channel. The walk is lock-free: a channel
        /// present throughout is seen exactly once, one created or retired
        /// meanwhile may or may not appear.
        pub(crate) fn live_channels(&self) -> Vec<Arc<Channel>> {
            self.channels.pin().values().cloned().collect()
        }

        fn channel_snapshot(&self) -> Vec<ChannelSnapshot> {
            let channels = self.live_channels();
            channels
                .iter()
                .filter_map(|c| ChannelSnapshot::of(c))
                .collect()
        }

        // Keep the inspection-only index walk out of the per-message dispatch.
        // Inlining this collector enlarged receive_with_totals enough to prevent
        // its inlining into the native receive batch in the release build.
        #[inline(never)]
        fn channel_members_snapshot(&self) -> Vec<(SnapshotName, usize)> {
            let channels = self.live_channels();
            channels
                .iter()
                .filter_map(|c| ChannelSnapshot::of(c).map(|row| (row.name, row.members)))
                .collect()
        }

        pub fn channels(&self) -> Vec<ChannelInfo> {
            let mut channels: Vec<_> = self
                .channel_snapshot()
                .into_iter()
                .map(|channel| ChannelInfo {
                    name: std::str::from_utf8(channel.name.as_bytes())
                        .ok()
                        .map(str::to_owned),
                    name_base64: STANDARD.encode(channel.name.as_bytes()),
                    members: channel.members,
                    variables: channel.variables,
                    variable_bytes: channel.variable_bytes,
                })
                .collect();
            channels.sort_unstable_by(|a, b| a.name_base64.cmp(&b.name_base64));
            channels
        }

        pub(crate) fn channel_listing(&self) -> Vec<ChannelSnapshot> {
            let mut channels = self.channel_snapshot();
            // Base64 contains no zero bytes. Zero padding therefore preserves text
            // ordering, including empty names and keys that prefix another key.
            channels.sort_by_cached_key(|channel| channel.name.base64());
            channels
        }

        pub fn disconnect(&self, id: u64) -> bool {
            self.users.get(id).is_some_and(|p| {
                p.close(Disconnect {
                    code: 4000,
                    reason: "Administrative Disconnect",
                })
            })
        }

        #[cfg(test)]
        pub(crate) fn subscribe(
            self: &Arc<Self>,
            channel: &[u8],
        ) -> Result<Subscription, Disconnect> {
            let (outgoing, receiver) = mpsc::channel(self.config.queue_messages);
            let (close, close_receiver) = watch::channel(None);
            let registration =
                self.subscribe_with_output(channel, Arc::new(AsyncOutput { outgoing, close }))?;
            Ok(Subscription {
                registration,
                outgoing: receiver,
                close: close_receiver,
            })
        }

        /// Find a live room or create it. Lookup is a lock-free read of the
        /// index. Creation counts the channel first, then inserts; two creators
        /// of one name resolve atomically and the loser adopts the winner's
        /// channel and returns its count.
        fn room(&self, name: &[u8]) -> Arc<Channel> {
            let channels = self.channels.pin();
            if let Some(channel) = channels.get(name) {
                return channel.clone();
            }
            self.channel_count.fetch_add(1, Relaxed);
            // Copy a bounded name once per channel. Peers in an existing channel
            // share its canonical storage instead of allocating another name.
            let name = Bytes::copy_from_slice(name);
            let channel = Channel::new(name.clone());
            match channels.try_insert(name, channel.clone()) {
                Ok(_) => channel,
                Err(occupied) => {
                    self.channel_count.fetch_sub(1, Relaxed);
                    occupied.current.clone()
                }
            }
        }

        pub(crate) fn subscribe_with_output(
            self: &Arc<Self>,
            channel: &[u8],
            output: Arc<dyn PeerOutput>,
        ) -> Result<Registration, Disconnect> {
            if channel.len() > 16 {
                return Err(Disconnect::CHANNEL_LENGTH);
            }
            if self.is_draining() {
                self.metrics.rejected.fetch_add(1, Relaxed);
                return Err(Disconnect::OVERLOADED);
            }
            self.sessions.fetch_add(1, Relaxed);
            let mut lease = SessionLease {
                relay: self,
                id: None,
            };
            let key = self.next_key.fetch_add(1, Relaxed);
            // Set once the welcome has been written. A retry against a replacement
            // room keeps this ID, so the client never sees a second welcome.
            let mut welcomed = None;
            loop {
                // A room retired by a concurrent last departure is replaced by a
                // fresh one.
                let room = loop {
                    let room = self.room(channel);
                    if !room.state.read().retired {
                        break room;
                    }
                };
                let build = |id: u64| {
                    Arc::new(Peer {
                        key,
                        id: AtomicU64::new(id),
                        channel: room.name.clone(),
                        room: room.clone(),
                        authenticated: AtomicBool::new(false),
                        listener: AtomicU8::new(0),
                        closing: AtomicBool::new(false),
                        retirement_barriers: (room.name.as_ref() == GLOBAL_CHANNEL)
                            .then(|| Box::new(std::array::from_fn(|_| AtomicU64::new(0)))),
                        retirement_revoked: AtomicU64::new(0),
                        message_barrier: AtomicU64::new(0),
                        message_revoked: AtomicU64::new(0),
                        output: output.clone(),
                        budget: Arc::new(Budget {
                            used: AtomicUsize::new(0),
                            limit: self.config.queue_bytes,
                            global_limit: self.config.total_queue_bytes,
                            metrics: self.metrics.clone(),
                        }),
                    })
                };
                let (id, peer) = match welcomed {
                    None => loop {
                        let id = random_id().map_err(|_| Disconnect::OVERLOADED)?;
                        if id == 0 || id == BROADCAST {
                            continue;
                        }
                        let peer = build(id);
                        if self.users.try_insert(id, &peer) {
                            break (id, peer);
                        }
                    },
                    Some(id) => {
                        // The welcome already went out under this ID: point its
                        // ownership at a peer bound to the replacement room.
                        let peer = build(id);
                        if !self.users.replace(id, key, &peer) {
                            return Err(Disconnect::TAKEN);
                        }
                        (id, peer)
                    }
                };
                lease.id = Some((id, key));
                if welcomed.is_none() {
                    // The welcome is first in this session's FIFO: nothing can
                    // address the peer through its room before the insert below.
                    // No lock is held here, so the inline socket write cannot delay
                    // any other session.
                    peer.enqueue_welcome(&Frame::integer(id))?;
                    welcomed = Some(id);
                }
                {
                    let mut state = room.state.write();
                    if state.retired {
                        // Lost a race with the last departure after the welcome
                        // went out. Start over against the replacement room. The
                        // session count and the ID stay reserved by the lease.
                        drop(state);
                        continue;
                    }
                    state.insert(peer.clone());
                }
                self.membership_changed();
                self.metrics.joined.fetch_add(1, Relaxed);
                lease.id = None;
                std::mem::forget(lease);
                return Ok(Registration {
                    peer,
                    relay: self.clone(),
                });
            }
        }

        fn membership_changed(&self) {
            self.membership_generation.fetch_add(1, Relaxed);
            self.global_view.store(None);
        }

        /// Every current room member. Cached until the next membership change; a
        /// view built during a change is used once and not cached.
        fn global_recipients(&self) -> Arc<Vec<Arc<Peer>>> {
            let generation = self.membership_generation.load(Relaxed);
            if let Some(cached) = self.global_view.load_full()
                && cached.0 == generation
            {
                return cached.1.clone();
            }
            let channels = self.live_channels();
            let mut members = Vec::new();
            for channel in &channels {
                members.extend(
                    channel
                        .state
                        .read()
                        .members
                        .values()
                        .filter(|peer| !peer.closing.load(Relaxed))
                        .cloned(),
                );
            }
            let view = Arc::new(members);
            if self.membership_generation.load(Relaxed) == generation {
                // Publish unless a newer generation is already cached. A lost race
                // against an equal generation stores an identical view.
                let current = self.global_view.load();
                if current.as_ref().is_none_or(|cached| cached.0 < generation) {
                    self.global_view
                        .compare_and_swap(&current, Some(Arc::new((generation, view.clone()))));
                }
            }
            view
        }

        #[cfg(test)]
        pub(crate) fn receive(&self, peer: &Peer, bytes: Bytes) -> Result<(), Disconnect> {
            self.receive_batch().receive(peer, bytes).map(|_| ())
        }

        pub(crate) fn receive_batch(&self) -> ReceiveBatch<'_> {
            ReceiveBatch {
                relay: self,
                totals: InputTotals::default(),
            }
        }

        fn receive_with_totals<const UNRELIABLE: bool>(
            &self,
            peer: &Peer,
            bytes: Bytes,
            totals: &mut InputTotals,
            native: Option<(&mut Option<Box<RoomFanout>>, usize)>,
        ) -> Result<usize, RoutingFailure> {
            if peer.closing.load(Relaxed) {
                return Ok(1);
            }
            totals.messages = totals.messages.wrapping_add(1);
            totals.bytes = totals.bytes.wrapping_add(bytes.len() as u64);
            let (target, value) = protocol::parse(&bytes)?;
            if target == 0 {
                if UNRELIABLE {
                    return Err(Disconnect::PROTOCOL.into());
                }
                return self.control(peer, value).map(|_| 1).map_err(Into::into);
            }
            let private_recipient = if target == BROADCAST {
                if peer.is_global()
                    && self.config.disable_global_broadcast
                    && !peer.authenticated.load(Relaxed)
                {
                    return Err(Disconnect {
                        code: 1008,
                        reason: "Global Broadcast Requires Authentication",
                    }
                    .into());
                }
                None
            } else {
                match self.users.get(target) {
                    Some(recipient) => Some(recipient),
                    None => return Ok(1),
                }
            };
            let value_start = bytes.len() - value.len();
            let mut frame = Frame::routed(peer.id.load(Relaxed), bytes, value_start);
            frame.unreliable = UNRELIABLE;
            // One lock-free load of the observer snapshot per routed frame. Its
            // generation travels with any capture, so a concurrent revocation is
            // honoured by the capture itself.
            let listeners = self.listeners.load();
            let mask = listeners.mask();
            if let Some(recipient) = &private_recipient
                && mask & PRIVATE_MESSAGE == 0
            {
                // The ownership lookup above was atomic with reassignment. The
                // retained target owns its lifetime through the socket write.
                return recipient
                    .enqueue_frame(&frame)
                    .map(|_| 1)
                    .map_err(Into::into);
            }
            if target == BROADCAST && peer.is_global() {
                return self.deliver_room(peer, self.global_recipients(), frame, native);
            }
            if target == BROADCAST && mask & CHANNEL_MESSAGE == 0 {
                let recipients = peer.room.state.read().view();
                return self.deliver_room(peer, recipients, frame, native);
            }
            // Observers are present. The snapshot is immutable.
            let flag = if target == BROADCAST {
                CHANNEL_MESSAGE
            } else {
                PRIVATE_MESSAGE
            };
            let room = (target == BROADCAST).then(|| peer.room.state.read().view());
            let observers = listeners.of(flag);
            // Charge every fanout path by recipient visits, including global
            // observers. One complete message remains atomic with respect to this
            // source's FIFO; the connection yields before its next buffered frame.
            // Counts conservatively include the sender or duplicate primary target.
            let work = room
                .as_ref()
                .map_or(1, |room| room.len())
                .saturating_add(observers.len());
            if let Some((pending, quantum)) = native
                && work > quantum
            {
                // Preserve the primary private delivery before observer work,
                // including when a large captured view cannot obtain staging credit.
                if let Some(recipient) = &private_recipient {
                    recipient.enqueue_fanout(&frame)?;
                }
                let snapshot = listeners.message_snapshot(flag);
                let room = room.as_ref().filter(|room| room.len() > 1).cloned();
                let publisher = pending.get_or_insert_with(|| Box::new(RoomFanout::default()));
                let observed = publisher
                    .observed
                    .get_or_insert_with(|| Publication::new(self.messages.clone()));
                let direct_visits = usize::from(private_recipient.is_some());
                if observed
                    .begin(
                        peer,
                        MessageRecipients {
                            room: room.clone(),
                            observers: snapshot,
                            excluded_observer: private_recipient.as_ref().map_or(0, |p| p.key),
                        },
                        frame.clone(),
                        listeners.generation,
                    )
                    .is_ok()
                {
                    drop(listeners);
                    publisher
                        .resume_routing(quantum.saturating_sub(direct_visits))
                        .map_err(|error| error.after(direct_visits))?;
                    return Ok(quantum.max(direct_visits));
                }
                // Staging credit was refused: deliver immediately instead. The
                // primary recipient was already served above. Each observer
                // re-checks the revocation mark under its own output lock, so
                // nothing lands behind a concurrent revocation's acknowledgement.
                publisher.clear();
                let mut visited = direct_visits;
                if let Some(room) = &room {
                    for recipient in room.iter() {
                        visited += 1;
                        if recipient.key != peer.key {
                            recipient
                                .enqueue_fanout(&frame)
                                .map_err(|reason| RoutingFailure::visited(reason, visited))?;
                        }
                    }
                }
                let excluded = private_recipient.as_ref().map_or(0, |p| p.key);
                for recipient in observers {
                    visited += 1;
                    if recipient.key != excluded {
                        recipient
                            .enqueue_gated(&frame, &recipient.message_revoked, listeners.generation)
                            .map_err(|reason| RoutingFailure::visited(reason, visited))?;
                    }
                }
                return Ok(work.max(1));
            }
            // Small enough for this turn. Observers are gated as above.
            let mut visited = 0;
            if let Some(room) = &room {
                for recipient in room.iter() {
                    visited += 1;
                    if recipient.key != peer.key {
                        recipient
                            .enqueue_fanout(&frame)
                            .map_err(|reason| RoutingFailure::visited(reason, visited))?;
                    }
                }
                for recipient in observers {
                    visited += 1;
                    recipient
                        .enqueue_gated(&frame, &recipient.message_revoked, listeners.generation)
                        .map_err(|reason| RoutingFailure::visited(reason, visited))?;
                }
            } else if let Some(recipient) = &private_recipient {
                visited = 1;
                recipient.enqueue_fanout(&frame)?;
                for listener in observers {
                    visited += 1;
                    if listener.key != recipient.key {
                        listener
                            .enqueue_gated(&frame, &listener.message_revoked, listeners.generation)
                            .map_err(|reason| RoutingFailure::visited(reason, visited))?;
                    }
                }
            }
            Ok(work.max(1))
        }

        fn deliver_room(
            &self,
            source: &Peer,
            recipients: Arc<Vec<Arc<Peer>>>,
            frame: Frame,
            native: Option<(&mut Option<Box<RoomFanout>>, usize)>,
        ) -> Result<usize, RoutingFailure> {
            if let Some((pending, quantum)) = native
                && recipients.len() > quantum
            {
                let work = pending.get_or_insert_with(|| Box::new(RoomFanout::default()));
                if work
                    .begin(
                        source,
                        FanoutRecipients::Room(recipients.clone()),
                        frame.clone(),
                        0,
                    )
                    .is_ok()
                {
                    work.resume_routing(quantum)?;
                    return Ok(quantum);
                }
                // Refused staging credit: one longer turn beats a lost message.
            }
            deliver_room(source, recipients, frame)
        }

        /// A publisher's connection ended with an unfinished fanout. The remaining
        /// recipients still receive the message through a departure queue, ahead
        /// of the source's own departure notice.
        pub(crate) fn detach_fanout(&self, source: &Peer, mut work: Box<RoomFanout>) {
            if !work.active() || self.is_stopping() {
                return;
            }
            work.detach();
            let queue = match &work.recipients {
                Some(FanoutRecipients::Room(_)) if !source.is_global() => &source.room.departures,
                _ => &self.detached,
            };
            self.push_job(queue, work, &*source.output);
        }

        fn control(&self, peer: &Peer, value: &[u8]) -> Result<(), Disconnect> {
            match protocol::command(value)? {
                Command::Authenticate(password) => {
                    if protocol::secret_matches(self.config.auth_password.as_deref(), password) {
                        peer.authenticated.store(true, Relaxed);
                    }
                    peer.enqueue_reply(&Frame::reply_bool(0, peer.authenticated.load(Relaxed)))?;
                }
                Command::Listen(flags) => {
                    if !peer.authenticated.load(Relaxed) {
                        return self.unauthorized();
                    }
                    let mut barrier = false;
                    let mut notices = Vec::new();
                    if peer.is_global() {
                        let member = peer.room.state.read().members.get(&peer.key).cloned();
                        if peer.closing.load(Relaxed) {
                            return Ok(());
                        }
                        let previous = peer.listener.load(Relaxed);
                        let next = flags & 7;
                        // Publish first: every capture from now on sees the new
                        // membership, and the barrier below covers the old one.
                        // Concurrent changes by other observers each rebuild from
                        // the latest snapshot; nobody locks.
                        let old = self.listeners.rcu(|current| {
                            Arc::new(current.with_flags(peer, previous, next, member.as_ref()))
                        });
                        peer.listener.store(next, Relaxed);
                        if previous & !next & (CHANNEL_MESSAGE | PRIVATE_MESSAGE) != 0 {
                            notices.extend(self.messages.flush(peer, old.generation));
                            barrier = true;
                        }
                        if previous & DISCONNECT_MESSAGE != 0 && next & DISCONNECT_MESSAGE == 0 {
                            // Older deferred notices must precede revocation's ACK.
                            notices.extend(self.flush_observer_retirements(peer, old.generation));
                            barrier = true;
                        }
                    } else {
                        peer.listener.store(flags & 7, Relaxed);
                    }
                    // Every registry lock is released now. The watermarks stored
                    // above keep deferred jobs from replaying these notices, and
                    // nothing can reach this observer's FIFO before its ACK.
                    for notice in &notices {
                        let _ = peer.enqueue_barrier(notice);
                    }
                    let reply = Frame::reply_uint(1, u64::from(flags & 7));
                    if barrier {
                        peer.enqueue_barrier(&reply)?;
                    } else {
                        peer.enqueue_reply(&reply)?;
                    }
                }
                Command::Channels => {
                    if !peer.authenticated.load(Relaxed) {
                        return self.unauthorized();
                    }
                    let mut channels = self.channel_members_snapshot();
                    channels.sort_unstable_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
                    // Names fit one-byte CBOR length headers; each entry adds an
                    // array header and its full-width member count. Size detached
                    // metadata after releasing the registry guard, then encode once.
                    let capacity = channels
                        .iter()
                        .try_fold(
                            protocol::uint_len(channels.len() as u64),
                            |size, (name, members)| {
                                size.checked_add(
                                    name.as_bytes().len() + 2 + protocol::uint_len(*members as u64),
                                )
                            },
                        )
                        .filter(|&size| size <= isize::MAX as usize)
                        .ok_or(Disconnect::OVERLOADED)?;
                    let mut e = Encoder::new(Vec::with_capacity(capacity));
                    e.array(channels.len() as u64).expect("Vec encoding");
                    for (name, members) in channels {
                        e.array(2)
                            .and_then(|e| e.bytes(name.as_bytes()))
                            .and_then(|e| e.u64(members as u64))
                            .expect("Vec encoding");
                    }
                    peer.enqueue_reply(&Frame::reply(2, e.into_writer().into()))?;
                }
                Command::Assign(id) => {
                    if !peer.authenticated.load(Relaxed) {
                        return self.unauthorized();
                    }
                    if id == 0 || id == BROADCAST {
                        return Err(Disconnect::RESERVED);
                    }
                    if peer.closing.load(Relaxed) {
                        return Ok(());
                    }
                    let old = peer.id.load(Relaxed);
                    if old != id {
                        // Takeover may have happened after receive's initial
                        // closing check. The map entry must still belong to this
                        // immutable session key.
                        let Some(displaced) = self.users.reassign(peer, old, id) else {
                            return Ok(());
                        };
                        peer.id.store(id, Relaxed);
                        if let Some(previous) = displaced {
                            previous.close(Disconnect::TAKEN);
                            self.membership_changed();
                        }
                    }
                    peer.enqueue_reply(&Frame::reply_uint(3, id))?;
                }
                Command::SetVariable { key, value } => {
                    if peer.closing.load(Relaxed) {
                        return Ok(());
                    }
                    {
                        let mut state = peer.room.state.write();
                        if state.retired {
                            return Err(Disconnect::PROTOCOL);
                        }
                        let previous_size =
                            state.variables.get(key).map_or(0, |v| key.len() + v.len());
                        let added = key.len() + value.len();
                        let next = state.variable_bytes - previous_size + added;
                        if next > self.config.max_variable_bytes
                            || self
                                .metrics
                                .variable_bytes
                                .fetch_update(Relaxed, Relaxed, |total| {
                                    (total - previous_size + added
                                        <= self.config.total_variable_bytes)
                                        .then_some(total - previous_size + added)
                                })
                                .is_err()
                        {
                            return Err(Disconnect::OVERLOADED);
                        }
                        // Copy the retained value, so a tiny variable cannot pin a large input frame.
                        state
                            .variables
                            .insert(Bytes::copy_from_slice(key), Bytes::copy_from_slice(value));
                        state.variable_bytes = next;
                    }
                    peer.enqueue_reply(&Frame::reply_bool(4, true))?;
                }
                Command::GetVariable(key) => {
                    let value = peer.room.state.read().variables.get(key).cloned();
                    peer.enqueue_reply(&Frame::variable(value))?;
                }
            }
            Ok(())
        }

        fn unauthorized(&self) -> Result<(), Disconnect> {
            Err(Disconnect {
                code: 1008,
                reason: "Authentication Required",
            })
        }

        pub(crate) fn has_retirements(&self) -> bool {
            !self.retirements.active.is_empty()
        }

        /// Advance one queue by one bounded turn. Queues rotate so a large job in
        /// one room cannot delay another room's notices. Never blocks a worker.
        pub(crate) fn progress_retirements(&self, quantum: usize) {
            let Some(queue) = self.retirements.active.pop() else {
                return;
            };
            let Some(mut state) = queue.state.try_lock() else {
                self.retirements.active.push(queue);
                return;
            };
            if let Some(work) = state.jobs.front_mut() {
                let _ = work.resume(quantum);
                if !work.active() {
                    state.jobs.pop_front();
                }
            }
            if state.jobs.is_empty() {
                state.queued = false;
                if state.jobs.capacity() > 32 {
                    state.jobs = VecDeque::new();
                }
            } else {
                drop(state);
                self.retirements.active.push(queue);
            }
        }

        /// Append a job and make sure its queue is scheduled. Returns whether a
        /// worker needs waking: only an idle retirement system does.
        fn push_job(
            &self,
            queue: &Arc<DepartureQueue>,
            work: Box<RoomFanout>,
            output: &dyn PeerOutput,
        ) {
            let mut state = queue.state.lock();
            state.jobs.push_back(work);
            if !state.queued {
                state.queued = true;
                let wake = self.retirements.active.is_empty();
                self.retirements.active.push(queue.clone());
                drop(state);
                if wake {
                    output.wake_retirement();
                }
            }
        }

        /// Collect this observer's still-pending captured notices and record a
        /// watermark so deferred jobs cannot replay them after re-subscription.
        /// The caller submits the notices once every lock is released, ahead of
        /// the revocation acknowledgement. Only the observer shards are scanned;
        /// together they hold every job that can reach a `re_globl` member.
        fn flush_observer_retirements(&self, observer: &Peer, revoked_at: u64) -> Vec<Frame> {
            let mut notices = Vec::new();
            let Some(barriers) = observer.retirement_barriers.as_deref() else {
                return notices;
            };
            // One shard at a time, and one bounded chunk of its jobs per hold.
            // Each shard numbers its jobs in queue order and the observer keeps one
            // watermark per shard, advanced under the lock after every chunk: a
            // job only advances under its shard's mutex, so no turn of an
            // inspected job can run before it sees the watermark, while a job not
            // yet inspected is never suppressed and is inspected later with its
            // cursor wherever its turns left it. Departures and turns on the shard
            // wait for at most one chunk.
            for (shard, queue) in self.observer_departures.iter().enumerate() {
                loop {
                    let pending = queue.state.lock();
                    let previous = barriers[shard].load(Relaxed);
                    let start = pending
                        .jobs
                        .partition_point(|work| work.retirement_sequence <= previous);
                    let chunk = pending.jobs.range(start..).take(BARRIER_QUANTUM);
                    let mut inspected = 0;
                    let mut last = previous;
                    // Adjacent jobs often share an immutable view; reuse its lookup
                    // within this chunk, but check each job's own cursor.
                    let mut searched: Option<(&[Arc<Peer>], Option<usize>)> = None;
                    for work in chunk {
                        inspected += 1;
                        last = work.retirement_sequence;
                        let Some(recipients) = work.recipients.as_ref() else {
                            continue;
                        };
                        // Captured after the revocation: the observer is not in it.
                        if work.generation > revoked_at {
                            continue;
                        }
                        let view = match recipients {
                            FanoutRecipients::Message(_) => continue,
                            other => other.single_slice(),
                        };
                        let position = match searched {
                            Some((previous_view, position))
                                if std::ptr::eq(previous_view, view) =>
                            {
                                position
                            }
                            _ => {
                                let position = view
                                    .iter()
                                    .position(|peer| std::ptr::eq(Arc::as_ptr(peer), observer));
                                searched = Some((view, position));
                                position
                            }
                        };
                        if position.is_some_and(|position| position >= work.cursor) {
                            notices.push(
                                work.packet
                                    .as_ref()
                                    .expect("charged retirement body")
                                    .frame
                                    .clone(),
                            );
                        }
                    }
                    let finished = start + inspected == pending.jobs.len();
                    if finished {
                        // Cover the shard's numbering so far: a job queued after
                        // this hold is newer than the revocation's publication.
                        last = pending.sequence;
                    }
                    // Deferred visits skip notices already submitted by this
                    // barrier, even if the observer re-enables the bit before
                    // those jobs finish.
                    barriers[shard].store(last, Relaxed);
                    if finished {
                        break;
                    }
                }
            }
            // A job captured before the revocation but queued after its shard's
            // scan is skipped by generation instead of arriving after the
            // acknowledgement.
            observer.retirement_revoked.store(revoked_at, Relaxed);
            notices
        }

        /// Budget refusal fallback: finish this queue's older jobs synchronously
        /// so a direct notice cannot overtake them. Other queues keep their turns.
        fn flush_queue(&self, queue: &Arc<DepartureQueue>) {
            // Finish the jobs queued before this call so the caller's direct
            // delivery follows them, in bounded holds: a long queue must not stall
            // departures on this shard while it drains. Jobs appended meanwhile
            // are newer than the caller's notice and stay queued.
            let until = queue.state.lock().sequence;
            loop {
                let mut state = queue.state.lock();
                let mut turns = 0;
                while let Some(work) = state.jobs.front_mut() {
                    if work.retirement_sequence > until || turns == FLUSH_TURNS_PER_HOLD {
                        break;
                    }
                    turns += 1;
                    if work.resume(RETIREMENT_QUANTUM).is_err() || !work.active() {
                        state.jobs.pop_front();
                    }
                }
                let drained = state
                    .jobs
                    .front()
                    .is_none_or(|work| work.retirement_sequence > until);
                if !drained {
                    drop(state);
                    continue;
                }
                if !state.jobs.is_empty() {
                    // Newer jobs keep the queue listed for worker turns.
                    return;
                }
                if state.jobs.capacity() > 32 {
                    state.jobs = VecDeque::new();
                }
                let listed = std::mem::replace(&mut state.queued, false);
                drop(state);
                if listed {
                    // Cycle the lock-free list once to drop this queue's entry. A
                    // worker that holds the entry popped right now finds the queue
                    // empty and drops it itself.
                    for _ in 0..self.retirements.active.len() {
                        let Some(entry) = self.retirements.active.pop() else {
                            break;
                        };
                        if !Arc::ptr_eq(&entry, queue) {
                            self.retirements.active.push(entry);
                        }
                    }
                }
                return;
            }
        }

        /// Queue a departure notice as a bounded job. Returns false when staging
        /// credit or numbering is exhausted: the caller then finishes this queue
        /// synchronously and delivers directly, preserving order.
        fn defer_departure(
            &self,
            queue: &Arc<DepartureQueue>,
            source: &Peer,
            recipients: FanoutRecipients,
            frame: &Frame,
            checks_barrier: bool,
            generation: u64,
        ) -> bool {
            let mut state = queue.state.lock();
            if self.is_stopping() {
                return true;
            }
            // Exhaustion falls back to ordered synchronous delivery rather than
            // wrapping and making older observer watermarks suppress new notices.
            let Some(sequence) = state.sequence.checked_add(1) else {
                return false;
            };
            let mut work = Box::<RoomFanout>::default();
            if work
                .begin(source, recipients, frame.clone(), sequence)
                .is_err()
            {
                return false;
            }
            work.checks_barrier = checks_barrier;
            work.shard = queue.shard;
            work.generation = generation;
            state.sequence = sequence;
            state.jobs.push_back(work);
            let wake = if state.queued {
                false
            } else {
                state.queued = true;
                let wake = self.retirements.active.is_empty();
                self.retirements.active.push(queue.clone());
                wake
            };
            drop(state);
            if wake {
                source.output.wake_retirement();
            }
            true
        }

        /// The shared disconnect-observer view for departure jobs. The cache
        /// mutex is held only to read or publish a handle; the copy and its
        /// reservation happen outside it. Concurrent builders under the same
        /// listener generation produce identical views, and the first to publish
        /// wins.
        fn disconnect_recipients(&self, listeners: &Listeners) -> Option<Arc<RetirementSnapshot>> {
            if let Some(cached) = self.disconnect_snapshot.load_full()
                && cached.0 == listeners.disconnect_generation
                && let Some(snapshot) = cached.1.upgrade()
            {
                return Some(snapshot);
            }
            let members = listeners.of(DISCONNECT_MESSAGE).to_vec();
            let bytes = members
                .capacity()
                .checked_mul(std::mem::size_of::<Arc<Peer>>())
                .and_then(|n| {
                    n.checked_add(
                        std::mem::size_of::<RetirementSnapshot>()
                            + 2 * std::mem::size_of::<usize>(),
                    )
                })?;
            if self
                .metrics
                .queued_bytes
                .fetch_update(Relaxed, Relaxed, |used| {
                    used.checked_add(bytes)
                        .filter(|&next| next <= self.config.total_queue_bytes)
                })
                .is_err()
            {
                self.metrics.global_pressure.fetch_add(1, Relaxed);
                return None;
            }
            let snapshot = Arc::new(RetirementSnapshot {
                members,
                bytes,
                metrics: self.metrics.clone(),
            });
            let current = self.disconnect_snapshot.load();
            if let Some(published) = current
                .as_ref()
                .filter(|cached| cached.0 == listeners.disconnect_generation)
                .and_then(|cached| cached.1.upgrade())
            {
                // Another departure published the same membership first; use its
                // view and let ours release its credit.
                return Some(published);
            }
            self.disconnect_snapshot.compare_and_swap(
                &current,
                Some(Arc::new((
                    listeners.disconnect_generation,
                    Arc::downgrade(&snapshot),
                ))),
            );
            Some(snapshot)
        }

        fn remove(&self, peer: &Peer) {
            let room = &peer.room;
            let stopping = self.is_stopping();
            // Global room departures use the observer shards; see their notes.
            let queue = if peer.is_global() {
                self.observer_queue_of(peer)
            } else {
                &room.departures
            };
            let remaining = {
                let mut state = room.state.write();
                if stopping {
                    // Terminal removals do not need a future routing index. Drop its
                    // cache once; captured fanouts keep their own immutable owners.
                    state.broadcast.take();
                }
                if !state.remove(peer.key) {
                    return;
                }
                peer.closing.store(true, Relaxed);
                if state.members.is_empty() || stopping {
                    None
                } else if peer.output.supports_retirement()
                    && (state.members.len() > RETIREMENT_QUANTUM
                        || !queue.state.lock().jobs.is_empty())
                {
                    // Large, or ordered behind older work: capture the shared view.
                    Some(Departure::Deferred(state.view()))
                } else {
                    // Small and nothing older ahead: a short copy, no cached view.
                    Some(Departure::Direct(state.members.values().cloned().collect()))
                }
            };
            self.membership_changed();
            let id = peer.id.load(Relaxed);
            let frame = (!stopping).then(|| Frame::envelope(BROADCAST, protocol::uint(id)));
            match (&frame, remaining) {
                (Some(frame), Some(Departure::Direct(recipients))) => {
                    for recipient in &recipients {
                        let _ = recipient.enqueue_fanout(frame);
                    }
                }
                (Some(frame), Some(Departure::Deferred(recipients))) => {
                    if !self.defer_departure(
                        queue,
                        peer,
                        FanoutRecipients::Room(recipients.clone()),
                        frame,
                        peer.is_global(),
                        self.listeners.load().generation,
                    ) {
                        self.flush_queue(queue);
                        for recipient in recipients.iter() {
                            let _ = recipient.enqueue_fanout(frame);
                        }
                    }
                }
                _ => {}
            }
            // An empty ordinary room leaves the index. Re-check under the room's
            // lock: a joiner may have arrived between the membership update and
            // here. The mark, the count and the index entry all change under
            // that lock, so a joiner that sees the mark and retries finds no
            // stale entry. The count drops before the entry goes, so a creator
            // that misses the entry always finds the slot free. Only this exact
            // channel is removed; a replacement under the same name is left alone.
            if !peer.is_global() {
                let empty = room.state.read().members.is_empty();
                if empty {
                    let mut state = room.state.write();
                    if state.members.is_empty() && !state.retired {
                        state.retired = true;
                        self.channel_count.fetch_sub(1, Relaxed);
                        self.metrics
                            .variable_bytes
                            .fetch_sub(state.variable_bytes, Relaxed);
                        let channels = self.channels.pin();
                        let removed =
                            channels.remove_if(&room.name, |_, current| Arc::ptr_eq(current, room));
                        debug_assert!(
                            matches!(removed, Ok(Some(_))),
                            "a live channel is indexed under its name until it retires"
                        );
                    }
                } else {
                    shrink_sparse_map(&mut room.state.write().members);
                }
            } else {
                shrink_sparse_map(&mut room.state.write().members);
                let member_flags = peer.listener.load(Relaxed);
                if member_flags != 0 {
                    peer.listener.store(0, Relaxed);
                    self.listeners
                        .rcu(|current| Arc::new(current.with_flags(peer, member_flags, 0, None)));
                }
            }
            if let Some(frame) = &frame
                && !peer.is_global()
            {
                // The snapshot's generation travels with the job, so a revocation
                // that races this capture is honoured by the job itself.
                let listeners = self.listeners.load_full();
                let observers = listeners.of(DISCONNECT_MESSAGE);
                if !observers.is_empty() {
                    let queue = self.observer_queue_of(peer);
                    let deferred = peer.output.supports_retirement()
                        && (observers.len() > RETIREMENT_QUANTUM
                            || !queue.state.lock().jobs.is_empty());
                    let queued = deferred
                        && self
                            .disconnect_recipients(&listeners)
                            .is_some_and(|snapshot| {
                                self.defer_departure(
                                    queue,
                                    peer,
                                    FanoutRecipients::Disconnect(snapshot),
                                    frame,
                                    true,
                                    listeners.generation,
                                )
                            });
                    if !queued {
                        if deferred {
                            self.flush_queue(queue);
                        }
                        // Each observer re-checks the revocation mark under its
                        // own output lock, so no notice lands behind a concurrent
                        // revocation's acknowledgement.
                        for recipient in observers {
                            let _ = recipient.enqueue_gated(
                                frame,
                                &recipient.retirement_revoked,
                                listeners.generation,
                            );
                        }
                    }
                }
            }
            self.users.remove_owned(id, peer.key);
            self.sessions.fetch_sub(1, Relaxed);
            self.metrics.disconnected.fetch_add(1, Relaxed);
        }

        #[cfg(test)]
        pub(crate) fn channel(&self, name: &[u8]) -> Option<Arc<Channel>> {
            self.channels.pin().get(name).cloned()
        }

        #[cfg(test)]
        pub(crate) fn user(&self, id: u64) -> Option<Arc<Peer>> {
            self.users.get(id)
        }

        #[cfg(test)]
        pub(crate) fn cached_global_view(&self) -> Option<Arc<Vec<Arc<Peer>>>> {
            self.global_view.load_full().map(|cached| cached.1.clone())
        }

        #[cfg(test)]
        pub(crate) fn listeners_of(&self, flag: u8) -> Vec<Arc<Peer>> {
            self.listeners.load().of(flag).to_vec()
        }

        #[cfg(test)]
        pub(crate) fn listener_capacities(&self) -> [usize; 3] {
            let listeners = self.listeners.load();
            [0, 1, 2].map(|i| listeners.lists[i].capacity())
        }

        #[cfg(test)]
        pub(crate) fn message_snapshot_cached(&self, index: usize) -> Option<Arc<Vec<Arc<Peer>>>> {
            self.listeners.load().message_snapshots[index]
                .get()
                .cloned()
        }

        #[cfg(test)]
        pub(crate) fn disconnect_snapshot_cached(&self) -> bool {
            self.disconnect_snapshot
                .load_full()
                .is_some_and(|cached| cached.1.upgrade().is_some())
        }

        /// The observer-notice shard for a departing session: by room, so notices
        /// for sessions of one room stay in order while rooms spread out.
        fn observer_queue_of(&self, peer: &Peer) -> &Arc<DepartureQueue> {
            self.observer_shard(&peer.channel)
        }

        fn observer_shard(&self, channel: &[u8]) -> &Arc<DepartureQueue> {
            let mut hasher = DefaultHasher::new();
            channel.hash(&mut hasher);
            &self.observer_departures[(hasher.finish() % OBSERVER_QUEUE_SHARDS as u64) as usize]
        }

        #[cfg(test)]
        pub(crate) fn observer_queue_for(&self, channel: &[u8]) -> &Arc<DepartureQueue> {
            self.observer_shard(channel)
        }

        #[cfg(test)]
        pub(crate) fn observer_queue_len(&self) -> usize {
            self.observer_departures
                .iter()
                .map(|queue| queue.len())
                .sum()
        }

        #[cfg(test)]
        pub(crate) fn detached_queue(&self) -> &Arc<DepartureQueue> {
            &self.detached
        }

        /// Whether no registry lock is currently held by anyone. Output paths
        /// must run with this true.
        #[cfg(test)]
        pub(crate) fn registry_unlocked(&self) -> bool {
            self.live_channels()
                .iter()
                .all(|channel| channel.state.try_write().is_some())
        }

        #[cfg(test)]
        pub(crate) fn listener_generation(&self) -> u64 {
            self.listeners.load().generation
        }

        #[cfg(test)]
        pub(crate) fn users_len(&self) -> usize {
            self.users.len()
        }
    }

    /// Undo partial admission when a join fails after counting the session.
    struct SessionLease<'a> {
        relay: &'a Relay,
        id: Option<(u64, u64)>,
    }

    impl Drop for SessionLease<'_> {
        fn drop(&mut self) {
            if let Some((id, key)) = self.id {
                self.relay.users.remove_owned(id, key);
            }
            self.relay.sessions.fetch_sub(1, Relaxed);
        }
    }

    fn random_id() -> Result<u64> {
        let mut bytes = [0; 8];
        getrandom::fill(&mut bytes).context("operating system entropy unavailable")?;
        Ok(u64::from_ne_bytes(bytes))
    }

    #[cfg(test)]
    mod broadcast_tests {
        include!("tests/relay_broadcast.rs");
    }
    #[cfg(test)]
    mod channel_tests {
        include!("tests/relay_channel.rs");
    }
    #[cfg(test)]
    mod listener_tests {
        include!("tests/relay_listener.rs");
    }
    #[cfg(test)]
    mod tests {
        include!("tests/relay.rs");
    }

    #[cfg(test)]
    mod continuation_tests {
        include!("tests/relay_continuation.rs");
    }
}
pub mod webtransport {
    //! HTTP/3 transport. One reliable bidirectional stream carries length-prefixed
    //! CBOR records; datagrams carry complete, explicitly unreliable CBOR routes.
    mod ingress {
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
    }
    pub use ingress::EndpointIngress;

    use crate::{
        Relay,
        frame::Frame,
        protocol::{self, Disconnect},
        relay::{Envelope, FANOUT_QUANTUM, PeerOutput, Submission},
    };
    use anyhow::{Context, Result, bail};
    use bytes::Bytes;
    use parking_lot::Mutex;
    use std::{
        future::{Future, poll_fn},
        sync::{Arc, atomic::Ordering::Relaxed},
        task::{Context as TaskContext, Poll, Waker},
        time::{Duration, Instant},
    };
    use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
    use tokio::{sync::Notify, task::JoinSet};
    use tokio_util::sync::CancellationToken;
    use web_transport_quinn::{EstablishedIo as Connection, SendStream, SessionAcceptor};
    use wtransport::quinn::{Endpoint, VarInt};

    mod application {
        //! Explicit established-session data; pin ownership remains in the async shell.
        use super::{
            Output, Pending, QueuedOutput, StreamOutput, frame_chunks, poll_output_chunks, queue,
        };
        use crate::{
            Relay,
            relay::{Envelope, FANOUT_QUANTUM, Registration, RoutingFailure},
        };
        use anyhow::Result;
        use bytes::Bytes;
        use parking_lot::Mutex;
        use std::{
            future::Future,
            pin::Pin,
            sync::{Arc, atomic::Ordering::Relaxed},
            task::{Context, Poll},
            time::{Duration, Instant},
        };
        use tokio::io::{AsyncRead, ReadBuf};

        pub(super) fn poll_budget_available(cx: &mut Context<'_>) -> Poll<()> {
            if tokio::task::coop::has_budget_remaining() {
                return Poll::Ready(());
            }
            match tokio::task::coop::poll_proceed(cx) {
                Poll::Pending => Poll::Pending,
                Poll::Ready(restore) => {
                    drop(restore);
                    Poll::Ready(())
                }
            }
        }

        #[derive(Default)]
        pub(super) struct RecordReader {
            header: [u8; 4],
            header_read: usize,
            size: Option<usize>,
            body: RecordBody,
            body_read: usize,
            lease: Option<crate::read_budget::ReadLease>,
        }

        enum RecordBody {
            Direct(Vec<u8>),
            Arena(crate::receive_arena::ArenaBuffer),
        }

        impl Default for RecordBody {
            fn default() -> Self {
                Self::Direct(Vec::new())
            }
        }

        impl RecordBody {
            fn len(&self) -> usize {
                match self {
                    Self::Direct(v) => v.len(),
                    Self::Arena(v) => v.as_ref().len(),
                }
            }

            fn publish(self) -> Bytes {
                match self {
                    Self::Direct(v) => Bytes::from(v),
                    Self::Arena(v) => v.publish(),
                }
            }

            fn as_mut(&mut self) -> &mut [u8] {
                match self {
                    Self::Direct(v) => v,
                    Self::Arena(v) => v.as_mut(),
                }
            }
        }

        impl RecordReader {
            pub(super) fn poll<S: AsyncRead + Unpin>(
                &mut self,
                stream: &mut S,
                maximum: usize,
                budget: Option<&Arc<crate::read_budget::ReadBudget>>,
                arena: Option<&crate::receive_arena::ReceiveArena>,
                cx: &mut Context<'_>,
            ) -> Poll<Result<Bytes>> {
                while self.header_read < self.header.len() {
                    let mut read = ReadBuf::new(&mut self.header[self.header_read..]);
                    match Pin::new(&mut *stream).poll_read(cx, &mut read) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error.into())),
                        Poll::Ready(Ok(())) => {}
                    }
                    if read.filled().is_empty() {
                        return Poll::Ready(Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "early eof",
                        )
                        .into()));
                    }
                    self.header_read += read.filled().len();
                }
                let size = if let Some(size) = self.size {
                    size
                } else {
                    let size = u32::from_be_bytes(self.header) as usize;
                    if size > maximum {
                        return Poll::Ready(Err(anyhow::anyhow!("invalid record length")));
                    }
                    self.lease = match budget
                        .map(|budget| budget.try_lease(size.saturating_mul(2)))
                        .transpose()
                    {
                        Ok(lease) => lease,
                        Err(error) => return Poll::Ready(Err(error.into())),
                    };
                    self.body = arena.and_then(|a| a.allocate(size)).map_or_else(
                        || RecordBody::Direct(Vec::with_capacity(size.min(4096))),
                        RecordBody::Arena,
                    );
                    self.body_read = 0;
                    self.size = Some(size);
                    size
                };
                while self.body_read < size {
                    if self.body_read == self.body.len() {
                        let count = (size - self.body.len()).min(4096);
                        let start = self.body.len();
                        if let RecordBody::Direct(body) = &mut self.body {
                            if start + count > body.capacity() {
                                let capacity = body.capacity().saturating_mul(2).min(size);
                                body.reserve_exact(capacity.max(start + count) - start);
                            }
                            body.resize(start + count, 0);
                        }
                    }
                    let mut read = ReadBuf::new(&mut self.body.as_mut()[self.body_read..]);
                    match Pin::new(&mut *stream).poll_read(cx, &mut read) {
                        Poll::Pending => return Poll::Pending,
                        Poll::Ready(Err(error)) => return Poll::Ready(Err(error.into())),
                        Poll::Ready(Ok(())) => {}
                    }
                    if read.filled().is_empty() {
                        return Poll::Ready(Err(std::io::Error::new(
                            std::io::ErrorKind::UnexpectedEof,
                            "early eof",
                        )
                        .into()));
                    }
                    self.body_read += read.filled().len();
                }
                let bytes = std::mem::take(&mut self.body).publish();
                self.lease = None;
                self.header_read = 0;
                self.body_read = 0;
                self.size = None;
                Poll::Ready(Ok(bytes))
            }
        }

        struct RouteRun<'a> {
            pending: Pending<'a>,
            result: Result<usize, RoutingFailure>,
        }

        impl RouteRun<'_> {
            fn finish(self) -> Result<()> {
                self.result.map(|_| ()).map_err(|error| {
                    self.pending.registration.peer.close(error.reason);
                    anyhow::anyhow!(error.reason.reason)
                })
            }
        }

        enum InputPhase<'a> {
            Read,
            Route(RouteRun<'a>),
            QuantumYield,
        }

        pub(super) struct InputState<'a> {
            phase: InputPhase<'a>,
            remaining: usize,
            yielding: bool,
        }

        impl<'a> InputState<'a> {
            pub(super) fn new(quantum: usize) -> Self {
                Self {
                    phase: InputPhase::Read,
                    remaining: quantum,
                    yielding: false,
                }
            }

            pub(super) fn begin(
                &mut self,
                relay: &Relay,
                registration: &'a Registration,
                activity: &Option<Arc<Mutex<Instant>>>,
                bytes: Bytes,
                unreliable: bool,
            ) -> Result<()> {
                assert!(matches!(self.phase, InputPhase::Read));
                if let Some(activity) = activity {
                    *activity.lock() = Instant::now();
                }
                let mut pending = Pending {
                    registration,
                    work: None,
                };
                let mut batch = relay.receive_batch();
                let result = if unreliable {
                    batch.receive_datagram(
                        &registration.peer,
                        bytes,
                        &mut pending.work,
                        FANOUT_QUANTUM,
                    )
                } else {
                    batch.receive_bounded(
                        &registration.peer,
                        bytes,
                        &mut pending.work,
                        FANOUT_QUANTUM,
                    )
                };
                batch.publish();
                drop(batch);
                self.phase = InputPhase::Route(RouteRun { pending, result });
                Ok(())
            }

            pub(super) fn poll_ready(
                &mut self,
                cx: &mut Context<'_>,
                quantum: usize,
                mut poll_yield: impl FnMut(&mut Context<'_>, bool) -> Poll<()>,
            ) -> Poll<Result<()>> {
                loop {
                    if matches!(self.phase, InputPhase::Read) {
                        return Poll::Ready(Ok(()));
                    }
                    if matches!(self.phase, InputPhase::QuantumYield) {
                        let reset = !self.yielding;
                        self.yielding = true;
                        if poll_yield(cx, reset).is_pending() {
                            return Poll::Pending;
                        }
                        self.yielding = false;
                        self.remaining = quantum;
                        self.phase = InputPhase::Read;
                        continue;
                    }
                    let active = match &self.phase {
                        InputPhase::Route(run) => {
                            run.pending.work.as_ref().is_some_and(|work| work.active())
                        }
                        _ => unreachable!(),
                    };
                    if self.yielding || active {
                        let reset = !self.yielding;
                        self.yielding = true;
                        if poll_yield(cx, reset).is_pending() {
                            return Poll::Pending;
                        }
                        self.yielding = false;
                        let InputPhase::Route(run) = &mut self.phase else {
                            unreachable!()
                        };
                        if let Err(error) = run
                            .pending
                            .work
                            .as_mut()
                            .expect("yielded fanout")
                            .resume(FANOUT_QUANTUM)
                        {
                            return Poll::Ready(Err(anyhow::anyhow!(error.reason)));
                        }
                        continue;
                    }
                    let InputPhase::Route(run) =
                        std::mem::replace(&mut self.phase, InputPhase::Read)
                    else {
                        unreachable!()
                    };
                    if let Err(error) = run.finish() {
                        return Poll::Ready(Err(error));
                    }
                    self.remaining -= 1;
                    if self.remaining == 0 {
                        self.phase = InputPhase::QuantumYield;
                    }
                }
            }
        }

        pub(super) struct WriteAttempt<'a> {
            chunks: Option<[Bytes; 2]>,
            packet: Option<Envelope>,
            output: &'a Output,
            offset: usize,
        }

        impl<'a> WriteAttempt<'a> {
            fn new(output: &'a Output, packet: Envelope, offset: usize) -> Self {
                Self {
                    chunks: None,
                    packet: Some(packet),
                    output,
                    offset,
                }
            }
        }

        impl Future for WriteAttempt<'_> {
            type Output = Result<Envelope>;

            fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
                let this = self.get_mut();
                if this.chunks.is_none() {
                    this.chunks = Some(frame_chunks(
                        &this.packet.as_ref().expect("active packet").frame,
                        this.offset,
                    ));
                }
                match poll_output_chunks(
                    this.output,
                    this.chunks.as_mut().expect("write chunks"),
                    cx,
                ) {
                    Poll::Pending => Poll::Pending,
                    Poll::Ready(Err(error)) => Poll::Ready(Err(error)),
                    Poll::Ready(Ok(())) => {
                        Poll::Ready(Ok(this.packet.take().expect("uncompleted write")))
                    }
                }
            }
        }

        enum WriterPhase {
            Waiting,
            Writing,
            Yielding(Envelope),
            Ack([Bytes; 1]),
            Done,
        }

        pub(super) struct WriterState<'a> {
            phase: WriterPhase,
            outgoing: queue::Receiver<'a, QueuedOutput, StreamOutput>,
            remaining: usize,
            quantum: usize,
            draining: bool,
            yielding: bool,
            selecting: bool,
        }

        impl<'a> WriterState<'a> {
            pub(super) fn new(
                outgoing: queue::Receiver<'a, QueuedOutput, StreamOutput>,
                quantum: usize,
            ) -> Self {
                Self {
                    phase: WriterPhase::Waiting,
                    outgoing,
                    remaining: quantum,
                    quantum,
                    draining: false,
                    yielding: false,
                    selecting: false,
                }
            }

            pub(super) fn begin_drain(&mut self) {
                self.draining = true;
            }

            pub(super) fn poll(
                &mut self,
                cx: &mut Context<'_>,
                output: &'a Output,
                mut writing: Pin<&mut Option<tokio::time::Timeout<WriteAttempt<'a>>>>,
                mut poll_yield: impl FnMut(&mut Context<'_>, bool) -> Poll<()>,
            ) -> Poll<Result<()>> {
                loop {
                    match &mut self.phase {
                        WriterPhase::Waiting => {
                            if self.selecting || !self.draining {
                                self.selecting = true;
                                if poll_budget_available(cx).is_pending() {
                                    return Poll::Pending;
                                }
                            }
                            if self.draining {
                                self.outgoing.close();
                            }
                            let take = |packet, owner: &mut StreamOutput| {
                                assert!(!owner.busy);
                                owner.busy = true;
                                packet
                            };
                            let (packet, was_idle) =
                                match self.outgoing.poll_recv_with_idle_map(cx, take) {
                                    Poll::Pending => return Poll::Pending,
                                    Poll::Ready(packet) => packet,
                                };
                            self.selecting = false;
                            let Some(queued) = packet else {
                                if self.draining {
                                    self.phase =
                                        WriterPhase::Ack([Bytes::from_static(&[0, 0, 0, 0])]);
                                    continue;
                                }
                                self.phase = WriterPhase::Done;
                                return Poll::Ready(Ok(()));
                            };
                            if was_idle {
                                self.remaining = self.quantum;
                            }
                            let remaining =
                                Duration::from_secs(output.relay.config.write_timeout_secs)
                                    .saturating_sub(
                                        queued
                                            .started
                                            .map_or(Duration::ZERO, |started| started.elapsed()),
                                    );
                            let attempt = WriteAttempt::new(output, queued.packet, queued.offset);
                            writing
                                .as_mut()
                                .set(Some(tokio::time::timeout(remaining, attempt)));
                            self.phase = WriterPhase::Writing;
                        }
                        WriterPhase::Writing => {
                            let result = match writing
                                .as_mut()
                                .as_pin_mut()
                                .expect("active write")
                                .poll(cx)
                            {
                                Poll::Pending => return Poll::Pending,
                                Poll::Ready(result) => result,
                            };
                            writing.as_mut().set(None);
                            let packet = match result {
                                Err(error) => {
                                    output.relay.metrics.write_timeouts.fetch_add(1, Relaxed);
                                    return Poll::Ready(Err(error.into()));
                                }
                                Ok(Err(error)) => {
                                    output.relay.metrics.write_errors.fetch_add(1, Relaxed);
                                    return Poll::Ready(Err(error));
                                }
                                Ok(Ok(packet)) => packet,
                            };
                            output.delivered(&packet.frame);
                            self.remaining -= 1;
                            if self.remaining == 0 {
                                self.phase = WriterPhase::Yielding(packet);
                            } else {
                                drop(packet);
                                self.phase = WriterPhase::Waiting;
                            }
                        }
                        WriterPhase::Yielding(_) => {
                            let reset = !self.yielding;
                            self.yielding = true;
                            if poll_yield(cx, reset).is_pending() {
                                return Poll::Pending;
                            }
                            self.yielding = false;
                            self.remaining = self.quantum;
                            let WriterPhase::Yielding(packet) =
                                std::mem::replace(&mut self.phase, WriterPhase::Waiting)
                            else {
                                unreachable!()
                            };
                            drop(packet);
                        }
                        WriterPhase::Ack(chunks) => {
                            match poll_output_chunks(output, chunks, cx) {
                                Poll::Pending => return Poll::Pending,
                                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                                Poll::Ready(Ok(())) => {}
                            }
                            self.phase = WriterPhase::Done;
                            return Poll::Ready(Ok(()));
                        }
                        WriterPhase::Done => panic!("polled completed writer"),
                    }
                }
            }
        }

        #[cfg(test)]
        mod tests {
            include!("tests/webtransport_application.rs");
        }
    }
    mod queue {
        //! One consumer, serialized producers. Keep the first queued packet inline so
        //! an idle peer does not retain Tokio mpsc's initial 32-message allocation.
        use parking_lot::Mutex;
        #[cfg(test)]
        use std::future::poll_fn;
        use std::{
            collections::VecDeque,
            task::{Context, Poll, Waker},
        };

        pub(super) struct Queue<T, O = ()> {
            pub state: Mutex<State<T, O>>,
        }

        pub(super) struct State<T, O = ()> {
            pub owner: O,
            first: Option<T>,
            rest: VecDeque<T>,
            maximum: usize,
            closed: bool,
            consumer: Option<Waker>,
        }

        impl<T, O> State<T, O> {
            pub fn allocated_bytes(&self) -> usize {
                self.rest.capacity() * std::mem::size_of::<T>()
            }

            pub fn is_empty(&self) -> bool {
                self.first.is_none()
            }

            pub fn is_closed(&self) -> bool {
                self.closed
            }

            // Call after publishing output or closure under the queue lock. The owning
            // Tokio task's waker schedules work; it must not synchronously reenter us.
            pub fn wake_consumer(&mut self) {
                if let Some(waker) = self.consumer.take() {
                    waker.wake();
                }
            }

            pub fn try_push(&mut self, value: T) -> bool {
                if self.closed
                    || usize::from(self.first.is_some()) + self.rest.len() >= self.maximum
                {
                    return false;
                }
                if self.first.is_none() {
                    self.first = Some(value);
                } else {
                    self.rest.push_back(value);
                }
                true
            }

            fn pop(&mut self) -> Option<T> {
                let value = self.first.take()?;
                self.first = self.rest.pop_front();
                Some(value)
            }
        }

        impl<T> Queue<T> {
            #[cfg(test)]
            pub fn new(maximum: usize) -> Self {
                Self::with_owner(maximum, ())
            }
        }
        impl<T, O> Queue<T, O> {
            pub fn with_owner(maximum: usize, owner: O) -> Self {
                Self {
                    state: Mutex::new(State {
                        owner,
                        first: None,
                        rest: VecDeque::new(),
                        maximum,
                        closed: false,
                        consumer: None,
                    }),
                }
            }

            // Only the owning session creates a receiver. Dropping it refuses future
            // submissions and releases every queued output reservation.
            pub fn receiver(&self) -> Receiver<'_, T, O> {
                Receiver {
                    queue: self,
                    was_idle: false,
                }
            }
        }

        pub(super) struct Receiver<'a, T, O = ()> {
            queue: &'a Queue<T, O>,
            was_idle: bool,
        }

        // Cancelled async receives release their task registration, as Notified did.
        // The explicit poll API instead retains it until Ready or Receiver drop.
        #[cfg(test)]
        struct ReceiveWait<'a, T, O> {
            queue: &'a Queue<T, O>,
            armed: bool,
        }

        #[cfg(test)]
        impl<T, O> Drop for ReceiveWait<'_, T, O> {
            fn drop(&mut self) {
                if self.armed {
                    self.queue.state.lock().consumer = None;
                }
            }
        }

        impl<T, O> Receiver<'_, T, O> {
            pub fn close(&self) {
                let mut state = self.queue.state.lock();
                state.closed = true;
                state.wake_consumer();
            }

            #[cfg(test)]
            pub async fn recv(&mut self) -> Option<T> {
                self.recv_with_idle().await.0
            }

            // Report an actual empty-queue wait so the writer can start a fresh work quantum
            // after sleeping, rather than carrying an old partial quantum into a burst.
            #[cfg(test)]
            pub async fn recv_with_idle(&mut self) -> (Option<T>, bool) {
                self.recv_with_idle_map(|value, _| value).await
            }

            // Update the owner under the queue lock when dequeue must be atomic with it.
            pub fn poll_recv_with_idle_map<U>(
                &mut self,
                cx: &mut Context<'_>,
                mut map: impl FnMut(T, &mut O) -> U,
            ) -> Poll<(Option<U>, bool)> {
                let mut state = self.queue.state.lock();
                if let Some(value) = state.pop() {
                    state.consumer = None;
                    let was_idle = std::mem::take(&mut self.was_idle);
                    return Poll::Ready((Some(map(value, &mut state.owner)), was_idle));
                }
                if state.closed {
                    state.consumer = None;
                    return Poll::Ready((None, std::mem::take(&mut self.was_idle)));
                }
                if !state
                    .consumer
                    .as_ref()
                    .is_some_and(|waker| waker.will_wake(cx.waker()))
                {
                    state.consumer = Some(cx.waker().clone());
                }
                self.was_idle = true;
                Poll::Pending
            }

            #[cfg(test)]
            pub async fn recv_with_idle_map<U>(
                &mut self,
                mut map: impl FnMut(T, &mut O) -> U,
            ) -> (Option<U>, bool) {
                self.was_idle = false;
                let mut wait = ReceiveWait {
                    queue: self.queue,
                    armed: true,
                };
                let result = poll_fn(|cx| self.poll_recv_with_idle_map(cx, &mut map)).await;
                wait.armed = false;
                result
            }
        }

        impl<T, O> Drop for Receiver<'_, T, O> {
            fn drop(&mut self) {
                let mut state = self.queue.state.lock();
                state.closed = true;
                state.consumer = None;
                state.first = None;
                state.rest.clear();
            }
        }

        #[cfg(test)]
        mod tests {
            include!("tests/webtransport_queue.rs");
        }
    }

    struct QueuedOutput {
        packet: Envelope,
        offset: usize,
        started: Option<Instant>,
    }

    struct StreamOutput {
        stream: SendStream,
        busy: bool,
    }

    struct Output {
        // Session includes HTTP metadata and per-clone polling state. These users
        // call its shared async API, so keep one handle rather than copying it into
        // each long-lived future and output sink.
        connection: Arc<Connection>,
        queue: queue::Queue<QueuedOutput, StreamOutput>,
        relay: Arc<Relay>,
        retirement: Arc<Notify>,
        activity: Option<Arc<Mutex<Instant>>>,
    }
    impl Output {
        fn submit(
            &self,
            packet: Submission<'_>,
            queue: &mut queue::State<QueuedOutput, StreamOutput>,
            immediate: bool,
        ) -> bool {
            if !packet.frame.unreliable {
                if queue.is_closed() {
                    return false;
                }
                let wake = queue.is_empty();
                let mut offset = 0;
                let mut started = None;
                if immediate && wake && !queue.owner.busy {
                    let mut chunks = frame_chunks(packet.frame, 0);
                    // One cancel-safe poll: Pending consumes no bytes. The queue
                    // notification wakes the owning writer, which polls again with
                    // its real waker; this producer never waits for write readiness.
                    let attempt = queue.owner.stream.write_chunks(&mut chunks);
                    tokio::pin!(attempt);
                    if let Poll::Ready(Ok(written)) = attempt
                        .as_mut()
                        .poll(&mut TaskContext::from_waker(Waker::noop()))
                    {
                        offset = written.bytes;
                        if offset == 4 + packet.frame.len() {
                            self.delivered(packet.frame);
                            return true;
                        }
                        if offset != 0 {
                            started = Some(Instant::now());
                        }
                    }
                }
                let allocated = queue.allocated_bytes();
                let accepted = queue.try_push(QueuedOutput {
                    packet: packet.into_owned(),
                    offset,
                    started,
                });
                let growth = queue.allocated_bytes() - allocated;
                if growth != 0 {
                    self.relay
                        .metrics
                        .output_queue_capacity_bytes
                        .fetch_add(growth, Relaxed);
                }
                if accepted && wake {
                    queue.wake_consumer();
                }
                return accepted;
            }
            if queue.is_closed() {
                return false;
            }
            let frame = packet.frame;
            let maximum = self.connection.max_datagram_size();
            if frame.len() > maximum {
                self.relay.metrics.datagrams_oversized.fetch_add(1, Relaxed);
                self.relay.metrics.datagrams_dropped.fetch_add(1, Relaxed);
                return true;
            }
            // Quinn retains an owned contiguous datagram until transmission. Build
            // that allocation directly instead of copying through per-peer scratch.
            let mut datagram = Vec::with_capacity(frame.len());
            datagram.extend_from_slice(frame.prefix());
            datagram.extend_from_slice(frame.payload());
            if self.connection.send_datagram(Bytes::from(datagram)).is_ok() {
                if let Some(activity) = &self.activity {
                    *activity.lock() = Instant::now();
                }
                self.relay.metrics.datagrams_submitted.fetch_add(1, Relaxed);
                self.relay
                    .metrics
                    .datagram_bytes_submitted
                    .fetch_add(frame.len() as u64, Relaxed);
            } else {
                self.relay.metrics.datagrams_dropped.fetch_add(1, Relaxed);
            }
            // Unreliable pressure drops a message, not the session or its commands.
            true
        }
    }
    impl Output {
        fn delivered(&self, frame: &Frame) {
            if let Some(activity) = &self.activity {
                *activity.lock() = Instant::now();
            }
            self.relay.metrics.deliveries.fetch_add(1, Relaxed);
            self.relay
                .metrics
                .delivered_bytes
                .fetch_add(frame.len() as u64, Relaxed);
        }
    }
    impl Drop for Output {
        fn drop(&mut self) {
            self.relay
                .metrics
                .output_queue_capacity_bytes
                .fetch_sub(self.queue.state.get_mut().allocated_bytes(), Relaxed);
        }
    }
    impl PeerOutput for Output {
        fn supports_retirement(&self) -> bool {
            true
        }
        fn wake_retirement(&self) {
            self.retirement.notify_one();
        }
        fn send(&self, packet: Submission<'_>) -> bool {
            self.submit(packet, &mut self.queue.state.lock(), true)
        }
        fn send_fanout(&self, packet: Submission<'_>) -> bool {
            self.submit(packet, &mut self.queue.state.lock(), false)
        }
        fn close(&self, reason: Disconnect) {
            self.connection
                .close(u32::from(reason.code), reason.reason.as_bytes());
        }
        fn send_gated(&self, packet: Submission<'_>, gate: &dyn Fn() -> bool) -> bool {
            let mut queue = self.queue.state.lock();
            !gate() || self.submit(packet, &mut queue, false)
        }
    }

    /// Read one record without allocating its declared size before data arrives.
    /// Keep this future alive across select branches: partial reads are not cancelled.
    pub async fn read_record<S: AsyncRead + Unpin>(
        stream: &mut S,
        maximum: usize,
    ) -> Result<Bytes> {
        read_record_budgeted(stream, maximum, None).await
    }
    async fn read_record_budgeted<S: AsyncRead + Unpin>(
        stream: &mut S,
        maximum: usize,
        budget: Option<&Arc<crate::read_budget::ReadBudget>>,
    ) -> Result<Bytes> {
        let mut header = [0; 4];
        stream.read_exact(&mut header).await?;
        let size = u32::from_be_bytes(header) as usize;
        anyhow::ensure!(size <= maximum, "invalid record length");
        // Reserve the full announced record plus growth headroom before allocating.
        // The lease is released when immutable ownership passes to routing.
        let _lease = budget
            .map(|budget| budget.try_lease(size.saturating_mul(2)))
            .transpose()?;
        let mut body = Vec::with_capacity(size.min(4096));
        while body.len() < size {
            let count = (size - body.len()).min(4096);
            let start = body.len();
            if start + count > body.capacity() {
                // Keep geometric growth without retaining a power-of-two allocation
                // larger than the complete record (including its CBOR envelope).
                let capacity = body.capacity().saturating_mul(2).min(size);
                body.reserve_exact(capacity.max(start + count) - start);
            }
            body.resize(start + count, 0);
            stream.read_exact(&mut body[start..]).await?;
        }
        Ok(Bytes::from(body))
    }

    pub async fn write_record<S: AsyncWrite + Unpin>(stream: &mut S, bytes: &[u8]) -> Result<()> {
        let size = u32::try_from(bytes.len()).context("record exceeds u32 framing")?;
        stream.write_all(&size.to_be_bytes()).await?;
        stream.write_all(bytes).await?;
        Ok(())
    }
    fn frame_chunks(frame: &Frame, offset: usize) -> [Bytes; 2] {
        let size = u32::try_from(frame.len()).expect("validated message length");
        let mut prefix = [0; 18];
        prefix[..4].copy_from_slice(&size.to_be_bytes());
        let header_len = 4 + frame.prefix().len();
        prefix[4..header_len].copy_from_slice(frame.prefix());
        [
            if offset < header_len {
                Bytes::copy_from_slice(&prefix[offset..header_len])
            } else {
                Bytes::new()
            },
            frame
                .shared_payload()
                .slice(offset.saturating_sub(header_len)..),
        ]
    }
    fn poll_output_chunks(
        output: &Output,
        chunks: &mut [Bytes],
        cx: &mut TaskContext<'_>,
    ) -> Poll<Result<()>> {
        let mut state = output.queue.state.lock();
        // Suffixes outlive this temporary future, as in the original async bridge.
        let result = {
            let write = state.owner.stream.write_all_chunks(chunks);
            tokio::pin!(write);
            write.as_mut().poll(cx)
        };
        if matches!(&result, Poll::Ready(Ok(()))) {
            state.owner.busy = false;
        }
        result.map(|result| result.map_err(Into::into))
    }

    /// Default QUIC transport policy used by the relay's server builders.
    pub fn server_transport_config() -> wtransport::quinn::TransportConfig {
        let mut transport = wtransport::quinn::TransportConfig::default();
        let mut acknowledgements = wtransport::quinn::AckFrequencyConfig::default();
        // Request eight packets per ACK, with a bounded sparse-traffic delay and
        // immediate acknowledgement of reordering. Unsupported peers ignore this.
        acknowledgements
            .ack_eliciting_threshold(VarInt::from_u32(7))
            .max_ack_delay(Some(Duration::from_millis(5)))
            .reordering_threshold(VarInt::from_u32(1));
        transport.ack_frequency_config(Some(acknowledgements));
        // Detect loss by the time threshold alone (RFC 9002 section 6.1.2, 9/8 of the
        // round trip). The packet threshold also declares a packet lost once three
        // later ones are acknowledged, which reads ordinary reordering as congestion
        // and collapses the window; under 2% real loss the time rule alone recovered
        // as quickly. It relies on the immediate acknowledgement of reordering above.
        transport.packet_threshold(u32::MAX);
        transport
    }

    /// Bind the shared UDP socket explicitly so bursts from many sessions do not
    /// compete for a small per-socket OS default. The OS may cap requested sizes.
    pub fn bind_endpoint(
        config: &crate::Config,
        transport: &wtransport::ServerConfig,
    ) -> Result<Endpoint> {
        let socket = std::net::UdpSocket::bind(config.bind)?;
        let settings = socket2::SockRef::from(&socket);
        if config.udp_receive_buffer_bytes != 0 {
            settings.set_recv_buffer_size(config.udp_receive_buffer_bytes)?;
        }
        if config.udp_send_buffer_bytes != 0 {
            settings.set_send_buffer_size(config.udp_send_buffer_bytes)?;
        }
        tracing::info!(
            receive_buffer_bytes = settings.recv_buffer_size()?,
            send_buffer_bytes = settings.send_buffer_size()?,
            "QUIC UDP socket buffers"
        );
        socket.set_nonblocking(true)?;
        Ok(Endpoint::new(
            transport.quic_endpoint_config().clone(),
            Some(transport.quic_config().clone()),
            socket,
            Arc::new(wtransport::quinn::TokioRuntime),
        )?)
    }

    /// Run a preconfigured TLS/QUIC endpoint with the existing relay core.
    pub async fn serve(
        endpoint: Endpoint,
        relay: Arc<Relay>,
        shutdown: CancellationToken,
    ) -> Result<()> {
        let endpoint = Arc::new(endpoint);
        anyhow::ensure!(
            relay
                .metrics
                .webtransport_endpoint
                .set(endpoint.clone())
                .is_ok(),
            "relay already has a WebTransport endpoint"
        );
        let stop = StopOnExit {
            relay: relay.clone(),
            endpoint: endpoint.clone(),
        };
        let mut sessions = JoinSet::new();
        let retirement = Arc::new(Notify::new());
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                _ = async {
                    if relay.has_retirements() { tokio::task::yield_now().await; }
                    else { retirement.notified().await; }
                } => relay.progress_retirements(FANOUT_QUANTUM),
                Some(result) = sessions.join_next(), if !sessions.is_empty() => { if let Err(error) = result { tracing::warn!(%error, "WebTransport task failed"); } },
                incoming = endpoint.accept() => {
                    let Some(incoming) = incoming else { break; };
                    if relay.is_draining() {
                        incoming.refuse();
                        relay.metrics.rejected.fetch_add(1, Relaxed);
                        continue;
                    }
                    relay.metrics.connections.fetch_add(1, Relaxed);
                    relay.metrics.accepted.fetch_add(1, Relaxed);
                    let relay = relay.clone();
                    let retirement = retirement.clone();
                    let lease = ConnectionLease(relay.clone());
                    // The larger admission future is cold. Release its storage after
                    // the handshake instead of retaining it in every live task.
                    let handshake = Box::pin(accept_connection(incoming, relay.clone()));
                    sessions.spawn(async move {
                        let _lease = lease;
                        let (connection, acceptor) = match tokio::time::timeout(Duration::from_secs(relay.config.handshake_timeout_secs), handshake).await {
                            Ok(Ok(connection)) => connection,
                            Ok(Err(error)) if error.is::<Refused>() => return,
                            _ => { relay.metrics.handshake_errors.fetch_add(1, Relaxed); return; }
                        };
                        if let Err(error) = session(connection.clone(), acceptor, relay, retirement).await {
                            tracing::debug!(%error, "WebTransport session ended");
                            connection.close(1002, b"Session ended");
                        }
                    });
                }
            }
        }
        drop(stop);
        let drained = async { while sessions.join_next().await.is_some() {} };
        if tokio::time::timeout(
            Duration::from_secs(relay.config.shutdown_timeout_secs),
            drained,
        )
        .await
        .is_err()
        {
            sessions.abort_all();
            while sessions.join_next().await.is_some() {}
        }
        Ok(())
    }

    async fn accept_connection(
        incoming: wtransport::quinn::Incoming,
        relay: Arc<Relay>,
    ) -> Result<(Arc<Connection>, SessionAcceptor)> {
        let quic = incoming.await?;
        let request = web_transport_quinn::Request::accept(quic).await?;
        if request.url.path() != "/war" {
            request.reject(axum::http::StatusCode::NOT_FOUND).await?;
            bail!("unknown WebTransport path");
        }
        if !relay.config.origin_policy.allows(&request.headers) {
            relay.metrics.origin_rejected.fetch_add(1, Relaxed);
            relay.metrics.rejected.fetch_add(1, Relaxed);
            request.reject(axum::http::StatusCode::FORBIDDEN).await?;
            return Err(Refused.into());
        }
        request
            .ok()
            .await
            .context("accept WebTransport")?
            .try_into_established()
            .map_err(|_| anyhow::anyhow!("new session unexpectedly has a pending poll operation"))
    }

    /// A session request refused by policy, counted as a rejection rather than a
    /// failed handshake.
    #[derive(Debug)]
    struct Refused;
    impl std::fmt::Display for Refused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("session request refused by policy")
        }
    }
    impl std::error::Error for Refused {}

    /// Stops admission and closes every connection with `1001` when `serve` ends,
    /// including when its task is aborted rather than cancelled.
    struct StopOnExit {
        relay: Arc<Relay>,
        endpoint: Arc<Endpoint>,
    }
    impl Drop for StopOnExit {
        fn drop(&mut self) {
            self.relay.begin_shutdown();
            self.endpoint
                .close(VarInt::from_u32(1001), b"Server Shutdown");
        }
    }

    struct ConnectionLease(Arc<Relay>);
    impl Drop for ConnectionLease {
        fn drop(&mut self) {
            self.0.metrics.connections.fetch_sub(1, Relaxed);
        }
    }

    // Match the native receive fairness quantum: amortize scheduling while bounding
    // ready input admitted in one turn. Large fanout keeps its separate work quantum.
    const INPUT_QUANTUM: usize = 256;

    async fn session(
        connection: Arc<Connection>,
        acceptor: SessionAcceptor,
        relay: Arc<Relay>,
        retirement: Arc<Notify>,
    ) -> Result<()> {
        let admission =
            tokio::time::timeout(Duration::from_secs(relay.config.join_timeout_secs), async {
                let (send, mut receive) = acceptor.accept_bi().await?;
                let join = read_record_budgeted(
                    &mut receive,
                    relay.config.max_message_bytes,
                    relay.metrics.read_budget.as_ref(),
                )
                .await?;
                Ok::<_, anyhow::Error>(((send, receive), join))
            })
            .await;
        // Preserve extra stream ownership before either ordinary join error can return.
        connection.finish_accepting(acceptor);
        let ((send, mut receive), join) = admission??;
        let activity =
            (relay.config.idle_timeout_secs != 0).then(|| Arc::new(Mutex::new(Instant::now())));
        let output = Arc::new(Output {
            connection: connection.clone(),
            queue: queue::Queue::with_owner(
                relay.config.queue_messages,
                StreamOutput {
                    stream: send,
                    busy: false,
                },
            ),
            relay: relay.clone(),
            retirement,
            activity: activity.clone(),
        });
        // A refused join closes with its own reason; the caller's generic 1002 then has no effect.
        let refuse = |e: Disconnect| {
            connection.close(u32::from(e.code), e.reason.as_bytes());
            anyhow::anyhow!(e.reason)
        };
        let registration = relay
            .subscribe_with_output(protocol::join(&join).map_err(refuse)?, output.clone())
            .map_err(refuse)?;
        let record_quantum = INPUT_QUANTUM.min(relay.config.queue_messages);
        let mut writer = application::WriterState::new(output.queue.receiver(), record_quantum);
        let writing = None;
        tokio::pin!(writing);
        let writer_yield = None;
        tokio::pin!(writer_yield);
        let result = {
            // Reverse local drop order retires reliable work before datagram work.
            let mut datagrams = application::InputState::new(INPUT_QUANTUM);
            let mut reader = application::RecordReader::default();
            let mut reliable = application::InputState::new(record_quantum);
            let idle = async {
                let Some(activity) = &activity else {
                    return std::future::pending::<Result<()>>().await;
                };
                let duration = Duration::from_secs(relay.config.idle_timeout_secs);
                loop {
                    let deadline = *activity.lock() + duration;
                    tokio::time::sleep_until(deadline.into()).await;
                    if activity.lock().elapsed() >= duration {
                        relay.metrics.idle_timeouts.fetch_add(1, Relaxed);
                        registration.peer.close(Disconnect::IDLE);
                        return Ok(());
                    }
                }
            };
            tokio::pin!(idle);
            let closed = connection.closed();
            tokio::pin!(closed);
            let datagram = None;
            tokio::pin!(datagram);
            let reliable_yield = None;
            tokio::pin!(reliable_yield);
            let datagram_yield = None;
            tokio::pin!(datagram_yield);
            poll_fn(|cx| {
                if application::poll_budget_available(cx).is_pending() {
                    return Poll::Pending;
                }
                if let Poll::Ready(result) = idle.as_mut().poll(cx) {
                    return Poll::Ready(result.map(|_| false));
                }
                if closed.as_mut().poll(cx).is_ready() {
                    return Poll::Ready(Ok(false));
                }
                if let Poll::Ready(result) =
                    writer.poll(cx, &output, writing.as_mut(), |cx, reset| {
                        if reset {
                            writer_yield.as_mut().set(Some(tokio::task::yield_now()));
                        }
                        let result = writer_yield
                            .as_mut()
                            .as_pin_mut()
                            .expect("writer yield")
                            .poll(cx);
                        if result.is_ready() {
                            writer_yield.as_mut().set(None);
                        }
                        result
                    })
                {
                    return Poll::Ready(result.map(|_| false));
                }
                let reliable_result = loop {
                    match reliable.poll_ready(cx, record_quantum, |cx, reset| {
                        if reset {
                            reliable_yield.as_mut().set(Some(tokio::task::yield_now()));
                        }
                        let result = reliable_yield
                            .as_mut()
                            .as_pin_mut()
                            .expect("reliable yield")
                            .poll(cx);
                        if result.is_ready() {
                            reliable_yield.as_mut().set(None);
                        }
                        result
                    }) {
                        Poll::Pending => break Poll::Pending,
                        Poll::Ready(Err(error)) => break Poll::Ready(Err(error)),
                        Poll::Ready(Ok(())) => {}
                    }
                    let bytes = match reader.poll(
                        &mut receive,
                        relay.config.max_message_bytes,
                        relay.metrics.read_budget.as_ref(),
                        Some(&relay.receive_arena),
                        cx,
                    ) {
                        Poll::Pending => break Poll::Pending,
                        Poll::Ready(Err(error)) => break Poll::Ready(Err(error)),
                        Poll::Ready(Ok(bytes)) => bytes,
                    };
                    if bytes.is_empty() {
                        break Poll::Ready(Ok(()));
                    }
                    if let Err(error) =
                        reliable.begin(&relay, &registration, &activity, bytes, false)
                    {
                        break Poll::Ready(Err(error));
                    }
                };
                if let Poll::Ready(result) = reliable_result {
                    return Poll::Ready(result.map(|_| true));
                }
                let datagram_result: Poll<Result<()>> = loop {
                    match datagrams.poll_ready(cx, INPUT_QUANTUM, |cx, reset| {
                        if reset {
                            datagram_yield.as_mut().set(Some(tokio::task::yield_now()));
                        }
                        let result = datagram_yield
                            .as_mut()
                            .as_pin_mut()
                            .expect("datagram yield")
                            .poll(cx);
                        if result.is_ready() {
                            datagram_yield.as_mut().set(None);
                        }
                        result
                    }) {
                        Poll::Pending => break Poll::Pending,
                        Poll::Ready(Err(error)) => break Poll::Ready(Err(error)),
                        Poll::Ready(Ok(())) => {}
                    }
                    if datagram.as_mut().as_pin_mut().is_none() {
                        datagram.as_mut().set(Some(connection.read_datagram()));
                    }
                    let result = match datagram
                        .as_mut()
                        .as_pin_mut()
                        .expect("datagram read")
                        .poll(cx)
                    {
                        Poll::Pending => break Poll::Pending,
                        Poll::Ready(result) => result,
                    };
                    datagram.as_mut().set(None);
                    let bytes = match result {
                        Ok(bytes) => bytes,
                        Err(error) => break Poll::Ready(Err(error.into())),
                    };
                    relay.metrics.datagrams_received.fetch_add(1, Relaxed);
                    if bytes.len() > relay.config.max_message_bytes {
                        registration.peer.close(Disconnect::PROTOCOL);
                        break Poll::Ready(Err(anyhow::anyhow!("datagram exceeds message limit")));
                    }
                    if let Err(error) =
                        datagrams.begin(&relay, &registration, &activity, bytes, true)
                    {
                        break Poll::Ready(Err(error));
                    }
                };
                datagram_result.map(|result| result.map(|_| false))
            })
            .await
        };
        // Both input states retired accepted work before this registration leaves.
        drop(registration);
        if result? {
            writer.begin_drain();
            tokio::time::timeout(
                Duration::from_secs(relay.config.write_timeout_secs),
                poll_fn(|cx| {
                    writer.poll(cx, &output, writing.as_mut(), |cx, reset| {
                        if reset {
                            writer_yield.as_mut().set(Some(tokio::task::yield_now()));
                        }
                        let result = writer_yield
                            .as_mut()
                            .as_pin_mut()
                            .expect("writer yield")
                            .poll(cx);
                        if result.is_ready() {
                            writer_yield.as_mut().set(None);
                        }
                        result
                    })
                }),
            )
            .await??;
            let _ = tokio::time::timeout(
                Duration::from_secs(relay.config.write_timeout_secs),
                connection.closed(),
            )
            .await;
            connection.close(1000, b"Graceful close complete");
        }
        Ok(())
    }

    // Preserve an already accepted publication when any session future is cancelled.
    struct Pending<'a> {
        registration: &'a crate::relay::Registration,
        work: Option<Box<crate::relay::RoomFanout>>,
    }
    impl Drop for Pending<'_> {
        fn drop(&mut self) {
            if let Some(work) = self.work.take() {
                self.registration.detach_fanout(work);
            }
        }
    }

    #[cfg(test)]
    mod immediate_output_tests {
        include!("tests/webtransport_immediate_output.rs");
    }
}

pub use admin::admin_router;
pub use config::Config;
pub use relay::Relay;
