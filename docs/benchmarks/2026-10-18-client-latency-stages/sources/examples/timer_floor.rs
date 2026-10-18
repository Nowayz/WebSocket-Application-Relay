//! Diagnostic timer floor with the population client's room count/pacing, no I/O.
use std::time::{Duration, Instant};
use tokio::{runtime::Builder, task::JoinSet, time::sleep_until};
fn main() {
    Builder::new_multi_thread().worker_threads(8).enable_all().build().unwrap().block_on(async {
        let start = Instant::now() + Duration::from_millis(250);
        let rooms = 6250;
        let mut jobs = JoinSet::new();
        for room in 0..rooms {
            jobs.spawn(async move {
                let phase = Duration::from_nanos(room * 250_000_000 / rooms);
                let mut samples = Vec::with_capacity(16);
                for sequence in 0..16 {
                    let due = start + phase + Duration::from_millis(sequence * 250);
                    sleep_until(due.into()).await;
                    samples.push(Instant::now().duration_since(due).as_nanos() as u64);
                }
                samples
            });
        }
        let mut samples = Vec::with_capacity(100000);
        while let Some(result) = jobs.join_next().await { samples.extend(result.unwrap()); }
        samples.sort_unstable();
        println!("{}", serde_json::json!({"scope":"No-I/O client timer floor, 6250 rooms,4Hz,4 seconds,8 workers; diagnostic not relay latency", "samples":samples.len(), "p50_ms":samples[49999] as f64/1e6,"p95_ms":samples[94999] as f64/1e6,"p99_ms":samples[98999] as f64/1e6,"max_ms":samples[99999] as f64/1e6}));
    });
}
