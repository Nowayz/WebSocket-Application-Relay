use minicbor::Encoder;
use std::{hint::black_box, time::Instant};
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let iterations: usize = args[1].parse().unwrap();
    let count = 30_000;
    let mut e = Encoder::new(Vec::new());
    e.array(2).unwrap().u8(0).unwrap().array(2).unwrap().u8(2).unwrap().array(count).unwrap();
    for _ in 0..count { e.bytes(&[0xa5; 255]).unwrap(); }
    let input = e.into_writer();
    // The complete envelope is valid bounded CBOR, but these list elements
    // cannot be channel records: each must be [name, population].
    assert!(war_relay::protocol::parse(&input).is_ok());
    let began = Instant::now();
    for _ in 0..iterations {
        black_box(war_relay::legacy_bridge::from_cbor(black_box(&input)).unwrap_err());
    }
    println!("{{\"iterations\":{iterations},\"entries\":{count},\"input_bytes\":{},\"nanoseconds\":{}}}",input.len(),began.elapsed().as_nanos());
}
