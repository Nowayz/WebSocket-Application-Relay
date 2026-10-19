use std::{hint::black_box, time::Instant};
use minicbor::Encoder;
use tokio_tungstenite::tungstenite::Message;
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let count: usize = args[1].parse().unwrap();
    let iterations: usize = args[2].parse().unwrap();
    let mut e = Encoder::new(Vec::new());
    e.array(2).unwrap().u8(0).unwrap().array(2).unwrap().u8(2).unwrap().array(count as u64).unwrap();
    let mut expected = vec![0; 8];
    expected.extend_from_slice(&(count as u32).to_le_bytes());
    for i in 0..count {
        let name: Vec<_> = (0..16).map(|j| (i+j) as u8).collect();
        e.array(2).unwrap().bytes(&name).unwrap().u32(i as u32).unwrap();
        expected.push(16); expected.extend_from_slice(&name);
    }
    for i in 0..count { expected.extend_from_slice(&(i as u32).to_le_bytes()); }
    let input = e.into_writer();
    let start = Instant::now();
    for _ in 0..iterations {
        let Some(Message::Binary(reply)) = war_relay::legacy_bridge::from_cbor(black_box(&input)).unwrap() else { panic!("binary reply"); };
        assert_eq!(reply.as_ref(), expected.as_slice());
        black_box(reply);
    }
    let ns = start.elapsed().as_nanos();
    println!("{{\"channels\":{count},\"iterations\":{iterations},\"nanoseconds\":{ns}}}");
}
