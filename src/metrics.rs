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
                    .map_or(0, |endpoint| endpoint.open_connections()) as u64,
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
