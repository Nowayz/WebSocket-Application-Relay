use std::{hint::black_box, time::Instant};
use war_relay::metrics::Metrics;
fn main() {
    let metrics = Metrics::default();
    let output = metrics.render(100_000, 10_000, false);
    std::fs::write(std::env::args().nth(1).expect("output"), &output).unwrap();
    let mut samples = Vec::new();
    for _ in 0..7 {
        let start = Instant::now();
        for _ in 0..20_000 { black_box(metrics.render(black_box(100_000), black_box(10_000), false)); }
        samples.push(start.elapsed().as_nanos() as f64 / 20_000.0);
    }
    println!("{}", serde_json::json!({"length":output.len(), "capacity":output.capacity(), "ns_per_render": samples}));
}
