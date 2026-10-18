use std::{
    fmt::Write,
    sync::{
        Arc,
        atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
    },
};
use tokio_tungstenite::tungstenite::protocol::ReadBudget;

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

impl Metrics {
    pub fn render(&self, sessions: usize, channels: usize, draining: bool) -> String {
        let mut text = String::with_capacity(2500);
        let budget = self.read_budget.as_deref();
        for (name, kind, value, help) in [
            (
                "read_buffer_reserved_bytes",
                "gauge",
                budget.map_or(0, ReadBudget::reserved) as u64,
                "Decoder staging allowance reserved, including two-times capacity growth headroom; excludes published payloads and allocator overhead.",
            ),
            (
                "read_buffer_peak_reserved_bytes",
                "gauge",
                budget.map_or(0, ReadBudget::peak) as u64,
                "Peak successfully reserved decoder staging allowance.",
            ),
            (
                "read_buffer_limit_bytes",
                "gauge",
                budget.map_or(0, ReadBudget::limit) as u64,
                "Shared decoder staging allowance; zero disables admission.",
            ),
            (
                "read_buffer_reservations_rejected_total",
                "counter",
                budget.map_or(0, ReadBudget::rejected),
                "Decoder staging growth or connection admissions refused before allocation.",
            ),
        ] {
            writeln!(
                text,
                "# HELP war_{name} {help}\n# TYPE war_{name} {kind}\nwar_{name} {value}"
            )
            .expect("String write");
        }
        for (name, value, help) in [
            (
                "accept_errors_total",
                self.accept_errors.load(Relaxed),
                "Native listener accept failures excluding WouldBlock and Interrupted; admission retries while established connections continue.",
            ),
            (
                "setup_errors_total",
                self.setup_errors.load(Relaxed),
                "Accepted native sockets refused after TCP configuration or readiness registration failed; also included in rejected_total.",
            ),
            (
                "ip_rejected_total",
                self.ip_rejected.load(Relaxed),
                "Native TCP admissions refused by the direct-peer IP limit; also included in rejected_total.",
            ),
            (
                "read_buffer_reclaims_total",
                self.read_buffer_reclaims.load(Relaxed),
                "Decoder receive storage resets completed between frames; this is not a freed-byte count.",
            ),
            (
                "read_buffer_reclaim_header_bytes_total",
                self.read_buffer_reclaim_header_bytes.load(Relaxed),
                "Incomplete WebSocket header bytes copied while reclaiming decoder receive storage.",
            ),
            (
                "read_buffer_reclaims_deferred_total",
                self.read_buffer_reclaims_deferred.load(Relaxed),
                "Scheduled decoder resets deferred because a frame or buffered input remains in progress.",
            ),
            (
                "idle_timeouts_total",
                self.idle_timeouts.load(Relaxed),
                "Joined sessions closed by the configured application idle timeout.",
            ),
            (
                "unstarted_writes_cancelled_total",
                self.unstarted_writes_cancelled.load(Relaxed),
                "Queued output cancelled with no bytes written from its first retained frame; no partial frame exists.",
            ),
            (
                "routing_reused_bytes_total",
                self.routing_reused_bytes.load(Relaxed),
                "Encoded application value bytes routed from shared receive storage, before fanout.",
            ),
            (
                "routing_copied_bytes_total",
                self.routing_copied_bytes.load(Relaxed),
                "Encoded application value bytes compacted to avoid retaining a larger receive slab, before fanout.",
            ),
            (
                "write_timeouts_total",
                self.write_timeouts.load(Relaxed),
                "Socket output operations exceeding their deadline.",
            ),
            (
                "write_errors_total",
                self.write_errors.load(Relaxed),
                "Socket output operations failing with transport errors.",
            ),
            (
                "aborted_writes_total",
                self.aborted_writes.load(Relaxed),
                "Queued output cancelled before completion during session retirement.",
            ),
            (
                "close_failures_total",
                self.close_failures.load(Relaxed),
                "Close handshakes failing or exceeding their single deadline.",
            ),
            (
                "transport_disconnects_total",
                self.transport_disconnects.load(Relaxed),
                "Joined peers lost through I/O errors or absent close handshakes.",
            ),
            (
                "global_pressure_total",
                self.global_pressure.load(Relaxed),
                "Output reservations refused by the process-wide byte budget.",
            ),
            (
                "accepted_total",
                self.accepted.load(Relaxed),
                "Accepted TCP connections, including failed handshakes.",
            ),
            (
                "rejected_total",
                self.rejected.load(Relaxed),
                "Connections or joins refused by admission limits, browser-origin policy or drain.",
            ),
            (
                "origin_rejected_total",
                self.origin_rejected.load(Relaxed),
                "Native HTTP upgrades refused by the configured browser-origin policy; also included in rejected_total.",
            ),
            (
                "handshake_errors_total",
                self.handshake_errors.load(Relaxed),
                "Failed or timed out WebSocket handshakes.",
            ),
            (
                "joined_total",
                self.joined.load(Relaxed),
                "Successful channel subscriptions.",
            ),
            (
                "join_rejected_total",
                self.join_rejected.load(Relaxed),
                "Upgraded WebSocket connections rejected before channel join; excludes handshake admission, peer Close and service shutdown.",
            ),
            (
                "disconnected_total",
                self.disconnected.load(Relaxed),
                "Removed channel subscriptions.",
            ),
            (
                "incoming_messages_total",
                self.incoming_messages.load(Relaxed),
                "Received data messages after subscription.",
            ),
            (
                "incoming_bytes_total",
                self.incoming_bytes.load(Relaxed),
                "Received data bytes after subscription.",
            ),
            (
                "deliveries_total",
                self.deliveries.load(Relaxed),
                "Data messages successfully written, including relay replies.",
            ),
            (
                "delivered_bytes_total",
                self.delivered_bytes.load(Relaxed),
                "Data bytes successfully written.",
            ),
            (
                "slow_consumers_total",
                self.slow_consumers.load(Relaxed),
                "Recipient queue capacity-close signals, including races with peer-initiated Close.",
            ),
            (
                "protocol_errors_total",
                self.protocol_errors.load(Relaxed),
                "Sessions closed for invalid WAR data packets.",
            ),
            (
                "rate_limited_total",
                self.rate_limited.load(Relaxed),
                "Sessions closed by inbound rate limits.",
            ),
        ] {
            writeln!(
                text,
                "# HELP war_{name} {help}\n# TYPE war_{name} counter\nwar_{name} {value}"
            )
            .expect("String write");
        }
        for (name, value, help) in [
            (
                "variable_bytes",
                self.variable_bytes.load(Relaxed),
                "Retained channel variable key and encoded-value bytes.",
            ),
            (
                "connections",
                self.connections.load(Relaxed),
                "Open TCP connections admitted to the relay, including handshakes.",
            ),
            (
                "sessions",
                sessions,
                "Registered sessions, including those pending close.",
            ),
            (
                "channels",
                channels,
                "Channels, including the persistent re_globl channel.",
            ),
            (
                "queued_bytes",
                self.queued_bytes.load(Relaxed),
                "Logical output reservations used for global admission, including in-flight messages and active fanout membership snapshots; shared bodies may be charged per recipient.",
            ),
            (
                "draining",
                usize::from(draining),
                "Whether admission is disabled.",
            ),
            (
                "output_queue_capacity_bytes",
                self.output_queue_capacity_bytes.load(Relaxed),
                "Allocated native output queue slots, including unused retained capacity; excludes bodies, allocator overhead and kernel buffers.",
            ),
            (
                "output_queue_peak_capacity_bytes",
                self.output_queue_peak_capacity_bytes.load(Relaxed),
                "Peak aggregate allocated native output queue slot capacity.",
            ),
        ] {
            writeln!(
                text,
                "# HELP war_{name} {help}\n# TYPE war_{name} gauge\nwar_{name} {value}"
            )
            .expect("String write");
        }
        text
    }
}
