use std::{
    fmt::Write,
    sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
};

#[derive(Default)]
pub struct Metrics {
    pub accepted: AtomicU64,
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
    pub queued_bytes: AtomicUsize,
    pub connections: AtomicUsize,
    pub variable_bytes: AtomicUsize,
    pub global_pressure: AtomicU64,
    pub write_timeouts: AtomicU64,
    pub write_errors: AtomicU64,
    pub aborted_writes: AtomicU64,
    pub close_failures: AtomicU64,
    pub transport_disconnects: AtomicU64,
}

impl Metrics {
    pub fn render(&self, sessions: usize, channels: usize, draining: bool) -> String {
        let mut text = String::with_capacity(2500);
        for (name, value, help) in [
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
                "Output operations cancelled by session policy or shutdown.",
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
                "Connections or joins refused by admission limits or drain.",
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
                "Sessions closed because an output queue exceeded its budget.",
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
                "Reserved output payload bytes, including an in-flight write.",
            ),
            (
                "draining",
                usize::from(draining),
                "Whether admission is disabled.",
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
