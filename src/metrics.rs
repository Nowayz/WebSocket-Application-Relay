use std::{
    fmt::Write,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
    },
};
use tungstenite::protocol::ReadBudget;

#[derive(Default)]
pub struct Metrics {
    pub read_budget: Option<Arc<ReadBudget>>,
    pub accepted: AtomicU64,
    pub accept_errors: AtomicU64,
    pub setup_errors: AtomicU64,
    pub rejected: AtomicU64,
    pub handshake_errors: AtomicU64,
    pub joined: AtomicU64,
    pub disconnected: AtomicU64,
    pub incoming_messages: AtomicU64,
    pub incoming_bytes: AtomicU64,
    pub routing_reused_bytes: AtomicU64,
    pub routing_copied_bytes: AtomicU64,
    pub deliveries: AtomicU64,
    pub delivered_bytes: AtomicU64,
    pub slow_consumers: AtomicU64,
    pub protocol_errors: AtomicU64,
    pub rate_limited: AtomicU64,
    pub idle_timeouts: AtomicU64,
    pub read_buffer_reclaims: AtomicU64,
    pub read_buffer_reclaim_header_bytes: AtomicU64,
    pub read_buffer_reclaims_deferred: AtomicU64,
    /// Exact global admission reservations, also exposed as a gauge. Includes
    /// in-flight messages and charged fanout membership snapshots.
    pub queued_bytes: AtomicUsize,
    pub connections: AtomicUsize,
    pub native_connection_slots: AtomicUsize,
    pub variable_bytes: AtomicUsize,
    pub global_pressure: AtomicU64,
    pub write_timeouts: AtomicU64,
    pub write_errors: AtomicU64,
    pub aborted_writes: AtomicU64,
    pub unstarted_writes_cancelled: AtomicU64,
    pub close_failures: AtomicU64,
    pub transport_disconnects: AtomicU64,
    pub join_rejected: AtomicU64,
    pub origin_rejected: AtomicU64,
    pub output_queue_capacity_bytes: AtomicUsize,
    pub output_queue_peak_capacity_bytes: AtomicUsize,
    pub ip_rejected: AtomicU64,
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
                "read_buffer_reserved_bytes",
                "gauge",
                budget.map_or(0, ReadBudget::reserved) as u64,
                "Decoder staging allowance reserved, including two-times capacity growth headroom; excludes published payloads and allocator overhead."
            ),
            sample!(
                "read_buffer_peak_reserved_bytes",
                "gauge",
                budget.map_or(0, ReadBudget::peak) as u64,
                "Peak successfully reserved decoder staging allowance."
            ),
            sample!(
                "read_buffer_limit_bytes",
                "gauge",
                budget.map_or(0, ReadBudget::limit) as u64,
                "Shared decoder staging allowance; zero disables admission."
            ),
            sample!(
                "read_buffer_reservations_rejected_total",
                "counter",
                budget.map_or(0, ReadBudget::rejected),
                "Decoder staging growth or connection admissions refused before allocation."
            ),
        ] {
            text.push_str(prefix);
            writeln!(text, "{value}").expect("String write");
        }
        for (prefix, value) in [
            sample!(
                "accept_errors_total",
                "counter",
                self.accept_errors.load(Relaxed),
                "Native listener accept failures excluding WouldBlock and Interrupted; admission retries while established connections continue."
            ),
            sample!(
                "setup_errors_total",
                "counter",
                self.setup_errors.load(Relaxed),
                "Accepted native sockets refused after TCP configuration or readiness registration failed; also included in rejected_total."
            ),
            sample!(
                "ip_rejected_total",
                "counter",
                self.ip_rejected.load(Relaxed),
                "Native TCP admissions refused by the direct-peer IP limit; also included in rejected_total."
            ),
            sample!(
                "read_buffer_reclaims_total",
                "counter",
                self.read_buffer_reclaims.load(Relaxed),
                "Decoder receive storage resets completed between frames; this is not a freed-byte count."
            ),
            sample!(
                "read_buffer_reclaim_header_bytes_total",
                "counter",
                self.read_buffer_reclaim_header_bytes.load(Relaxed),
                "Incomplete WebSocket header bytes copied while reclaiming decoder receive storage."
            ),
            sample!(
                "read_buffer_reclaims_deferred_total",
                "counter",
                self.read_buffer_reclaims_deferred.load(Relaxed),
                "Scheduled decoder resets deferred because a frame or buffered input remains in progress."
            ),
            sample!(
                "idle_timeouts_total",
                "counter",
                self.idle_timeouts.load(Relaxed),
                "Joined sessions closed by the configured application idle timeout."
            ),
            sample!(
                "unstarted_writes_cancelled_total",
                "counter",
                self.unstarted_writes_cancelled.load(Relaxed),
                "Queued output cancelled with no bytes written from its first retained frame; no partial frame exists."
            ),
            sample!(
                "routing_reused_bytes_total",
                "counter",
                self.routing_reused_bytes.load(Relaxed),
                "Encoded application value bytes routed from shared receive storage, before fanout."
            ),
            sample!(
                "routing_copied_bytes_total",
                "counter",
                self.routing_copied_bytes.load(Relaxed),
                "Encoded application value bytes compacted to avoid retaining a larger receive slab, before fanout."
            ),
            sample!(
                "write_timeouts_total",
                "counter",
                self.write_timeouts.load(Relaxed),
                "Socket output operations exceeding their deadline."
            ),
            sample!(
                "write_errors_total",
                "counter",
                self.write_errors.load(Relaxed),
                "Socket output operations failing with transport errors."
            ),
            sample!(
                "aborted_writes_total",
                "counter",
                self.aborted_writes.load(Relaxed),
                "Queued output cancelled before completion during session retirement."
            ),
            sample!(
                "close_failures_total",
                "counter",
                self.close_failures.load(Relaxed),
                "Close handshakes failing or exceeding their single deadline."
            ),
            sample!(
                "transport_disconnects_total",
                "counter",
                self.transport_disconnects.load(Relaxed),
                "Joined peers lost through I/O errors or absent close handshakes."
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
                "Accepted TCP connections, including failed handshakes."
            ),
            sample!(
                "rejected_total",
                "counter",
                self.rejected.load(Relaxed),
                "Connections or joins refused by admission limits, browser-origin policy or drain."
            ),
            sample!(
                "origin_rejected_total",
                "counter",
                self.origin_rejected.load(Relaxed),
                "Native HTTP upgrades refused by the configured browser-origin policy; also included in rejected_total."
            ),
            sample!(
                "handshake_errors_total",
                "counter",
                self.handshake_errors.load(Relaxed),
                "Failed or timed out WebSocket handshakes."
            ),
            sample!(
                "joined_total",
                "counter",
                self.joined.load(Relaxed),
                "Successful channel subscriptions."
            ),
            sample!(
                "join_rejected_total",
                "counter",
                self.join_rejected.load(Relaxed),
                "Upgraded WebSocket connections rejected before channel join; excludes handshake admission, peer Close and service shutdown."
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
            sample!(
                "protocol_errors_total",
                "counter",
                self.protocol_errors.load(Relaxed),
                "Sessions closed for invalid WAR data packets."
            ),
            sample!(
                "rate_limited_total",
                "counter",
                self.rate_limited.load(Relaxed),
                "Sessions closed by inbound rate limits."
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
                "Open TCP connections admitted to the relay, including handshakes."
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
                "native_connection_slots",
                "gauge",
                self.native_connection_slots.load(Relaxed),
                "Allocated native connection arena slots, including spare retained slots; not resident bytes or a connection limit."
            ),
            sample!(
                "output_queue_capacity_bytes",
                "gauge",
                self.output_queue_capacity_bytes.load(Relaxed),
                "Allocated native output queue slots, including unused retained capacity; excludes bodies, allocator overhead and kernel buffers."
            ),
            sample!(
                "output_queue_peak_capacity_bytes",
                "gauge",
                self.output_queue_peak_capacity_bytes.load(Relaxed),
                "Peak aggregate allocated native output queue slot capacity."
            ),
        ] {
            text.push_str(prefix);
            writeln!(text, "{value}").expect("String write");
        }
        text
    }
}
