//! Public helper construction cost; not relay throughput.
use std::{hint::black_box, time::Instant};
use bytes::Bytes;
use war_relay::protocol;
fn run(name: &str, make: impl Fn() -> Bytes) {
    let b = make();
    let len = b.len();
    let capacity = b.try_into_mut().unwrap().capacity();
    for clones in [0, 8] {
        let mut times = Vec::new();
        for _ in 0..7 {
            let began = Instant::now();
            for _ in 0..100_000 {
                let value = black_box(make());
                for _ in 0..clones { black_box(value.clone()); }
                black_box(value);
            }
            times.push(began.elapsed().as_nanos() as f64 / 100_000.0);
        }
        println!("{}", serde_json::json!({"case":name,"clones":clones,"encoded_len":len,"capacity":capacity,"ns_per_operation":times}));
    }
}
fn main() {
    let text = "x".repeat(1024);
    let value = protocol::byte_string(&[0x5a; 16]);
    run("text16", || protocol::text(black_box("sixteen-byte-msg")));
    run("text1024", || protocol::text(black_box(&text)));
    run("uint1", || protocol::uint(black_box(1)));
    run("uint65536", || protocol::uint(black_box(65536)));
    run("envelope1", || protocol::envelope(black_box(1),black_box(&value)));
    run("envelope_max", || protocol::envelope(black_box(u64::MAX),black_box(&value)));
    run("reply0", || protocol::reply(black_box(0),black_box(&value)));
    run("reply255", || protocol::reply(black_box(255),black_box(&value)));
}
