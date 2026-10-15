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
    pub deliveries: AtomicU64,
    pub delivered_bytes: AtomicU64,
    pub slow_consumers: AtomicU64,
    pub protocol_errors: AtomicU64,
    pub rate_limited: AtomicU64,
    pub queued_bytes: AtomicUsize,
    pub connections: AtomicUsize,
    pub variable_bytes: AtomicUsize,
    pub global_pressure: AtomicU64,
}

impl Metrics {
    pub fn render(&self, sessions: usize, channels: usize, draining: bool) -> String {
        let mut text = String::with_capacity(2500);
        for (name, value, help) in [
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
