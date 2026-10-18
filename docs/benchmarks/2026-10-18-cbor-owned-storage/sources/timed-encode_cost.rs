//! Whole byte-string construction and destruction; independent wire checks precede timing.
use std::{hint::black_box, time::Instant};
use war_relay::protocol;

fn measure<const CLONE: bool>(payload: &[u8], iterations: usize) -> f64 {
    let began = Instant::now();
    for _ in 0..iterations {
        let value = black_box(protocol::byte_string(black_box(payload)));
        if CLONE {
            let copies: [bytes::Bytes; 8] = std::array::from_fn(|_| value.clone());
            black_box(copies);
        }
        black_box(value);
    }
    began.elapsed().as_nanos() as f64 / iterations as f64
}
fn main() {
    for (length, iterations) in [(16, 500_000), (1024, 200_000), (65536, 10_000)] {
        let payload = vec![0x5a; length];
        let encoded = protocol::byte_string(&payload);
        let mut decoder = minicbor::Decoder::new(&encoded);
        assert_eq!(decoder.bytes().unwrap(), payload);
        assert_eq!(decoder.position(), encoded.len());
        let mutable = encoded.try_into_mut().unwrap();
        let capacity = mutable.capacity();
        for shared in [false, true] {
            if shared { measure::<true>(&payload, 1000); } else { measure::<false>(&payload, 1000); }
            let mut batches = Vec::new();
            for _ in 0..7 {
                batches.push(if shared { measure::<true>(&payload, iterations) } else { measure::<false>(&payload, iterations) });
            }
            println!("{}", serde_json::json!({"payload_bytes":length,"clones":if shared {8} else {0},"iterations_per_batch":iterations,"encoded_capacity":capacity,"ns_per_operation":batches}));
        }
    }
}
