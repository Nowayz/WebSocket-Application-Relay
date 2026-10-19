use std::{hint::black_box, sync::atomic::Ordering::Relaxed, time::Instant};
use war_relay::metrics::Metrics;
use tokio_tungstenite::tungstenite::protocol::ReadBudget;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let case = &args[1];
    let output_path = &args[2];
    let iterations: usize = args[3].parse().unwrap();
    let batches: usize = args[4].parse().unwrap();
    let mut metrics = Metrics::default();
    let mut lease = None;
    if case == "max" {
        metrics.accepted.store(u64::MAX, Relaxed);
        metrics.accept_errors.store(u64::MAX, Relaxed);
        metrics.setup_errors.store(u64::MAX, Relaxed);
        metrics.rejected.store(u64::MAX, Relaxed);
        metrics.handshake_errors.store(u64::MAX, Relaxed);
        metrics.joined.store(u64::MAX, Relaxed);
        metrics.disconnected.store(u64::MAX, Relaxed);
        metrics.incoming_messages.store(u64::MAX, Relaxed);
        metrics.incoming_bytes.store(u64::MAX, Relaxed);
        metrics.routing_reused_bytes.store(u64::MAX, Relaxed);
        metrics.routing_copied_bytes.store(u64::MAX, Relaxed);
        metrics.deliveries.store(u64::MAX, Relaxed);
        metrics.delivered_bytes.store(u64::MAX, Relaxed);
        metrics.slow_consumers.store(u64::MAX, Relaxed);
        metrics.protocol_errors.store(u64::MAX, Relaxed);
        metrics.rate_limited.store(u64::MAX, Relaxed);
        metrics.idle_timeouts.store(u64::MAX, Relaxed);
        metrics.read_buffer_reclaims.store(u64::MAX, Relaxed);
        metrics.read_buffer_reclaim_header_bytes.store(u64::MAX, Relaxed);
        metrics.read_buffer_reclaims_deferred.store(u64::MAX, Relaxed);
        metrics.queued_bytes.store(usize::MAX, Relaxed);
        metrics.connections.store(usize::MAX, Relaxed);
        metrics.native_connection_slots.store(usize::MAX, Relaxed);
        metrics.variable_bytes.store(usize::MAX, Relaxed);
        metrics.global_pressure.store(u64::MAX, Relaxed);
        metrics.write_timeouts.store(u64::MAX, Relaxed);
        metrics.write_errors.store(u64::MAX, Relaxed);
        metrics.aborted_writes.store(u64::MAX, Relaxed);
        metrics.unstarted_writes_cancelled.store(u64::MAX, Relaxed);
        metrics.close_failures.store(u64::MAX, Relaxed);
        metrics.transport_disconnects.store(u64::MAX, Relaxed);
        metrics.join_rejected.store(u64::MAX, Relaxed);
        metrics.origin_rejected.store(u64::MAX, Relaxed);
        metrics.output_queue_capacity_bytes.store(usize::MAX, Relaxed);
        metrics.output_queue_peak_capacity_bytes.store(usize::MAX, Relaxed);
        metrics.ip_rejected.store(u64::MAX, Relaxed);
    } else if case == "budget" {
        metrics.accepted.store(23, Relaxed);
        metrics.accept_errors.store(46, Relaxed);
        metrics.setup_errors.store(69, Relaxed);
        metrics.rejected.store(92, Relaxed);
        metrics.handshake_errors.store(115, Relaxed);
        metrics.joined.store(138, Relaxed);
        metrics.disconnected.store(161, Relaxed);
        metrics.incoming_messages.store(184, Relaxed);
        metrics.incoming_bytes.store(207, Relaxed);
        metrics.routing_reused_bytes.store(230, Relaxed);
        metrics.routing_copied_bytes.store(253, Relaxed);
        metrics.deliveries.store(276, Relaxed);
        metrics.delivered_bytes.store(299, Relaxed);
        metrics.slow_consumers.store(322, Relaxed);
        metrics.protocol_errors.store(345, Relaxed);
        metrics.rate_limited.store(368, Relaxed);
        metrics.idle_timeouts.store(391, Relaxed);
        metrics.read_buffer_reclaims.store(414, Relaxed);
        metrics.read_buffer_reclaim_header_bytes.store(437, Relaxed);
        metrics.read_buffer_reclaims_deferred.store(460, Relaxed);
        metrics.queued_bytes.store(483, Relaxed);
        metrics.connections.store(506, Relaxed);
        metrics.native_connection_slots.store(529, Relaxed);
        metrics.variable_bytes.store(552, Relaxed);
        metrics.global_pressure.store(575, Relaxed);
        metrics.write_timeouts.store(598, Relaxed);
        metrics.write_errors.store(621, Relaxed);
        metrics.aborted_writes.store(644, Relaxed);
        metrics.unstarted_writes_cancelled.store(667, Relaxed);
        metrics.close_failures.store(690, Relaxed);
        metrics.transport_disconnects.store(713, Relaxed);
        metrics.join_rejected.store(736, Relaxed);
        metrics.origin_rejected.store(759, Relaxed);
        metrics.output_queue_capacity_bytes.store(782, Relaxed);
        metrics.output_queue_peak_capacity_bytes.store(805, Relaxed);
        metrics.ip_rejected.store(828, Relaxed);
        let budget = ReadBudget::new(1024);
        lease = Some(budget.try_lease(256).unwrap());
        assert!(budget.try_lease(2048).is_err());
        metrics.read_budget = Some(budget);
    } else { assert_eq!(case, "zero"); }
    let sessions = if case == "max" { usize::MAX } else { 100_000 };
    let channels = if case == "max" { usize::MAX } else { 10_000 };
    let draining = case != "zero";
    let output = metrics.render(sessions, channels, draining);
    if output_path != "-" {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(output_path).unwrap();
        file.write_all(output.as_bytes()).unwrap();
    }
    let mut samples = Vec::with_capacity(batches);
    for _ in 0..batches {
        let began = Instant::now();
        for _ in 0..iterations {
            black_box(black_box(&metrics).render(black_box(sessions), black_box(channels), black_box(draining)));
        }
        samples.push(if iterations==0 {0.0} else {began.elapsed().as_nanos() as f64 / iterations as f64});
    }
    println!("{}", serde_json::json!({"case":case,"length":output.len(),"capacity":output.capacity(),"iterations":iterations,"batches":batches,"ns_per_render":samples}));
    drop(lease);
}
