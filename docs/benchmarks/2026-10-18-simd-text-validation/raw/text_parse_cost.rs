use std::{hint::black_box,time::Instant};
use war_relay::protocol;
fn main() {
 for length in [0usize,16,24,32,63,64,65,128,1024,65536] {
  for unicode in [false,true] {
   let mut bytes=vec![b'a';length];
   if unicode { for pair in bytes.chunks_exact_mut(2) { pair.copy_from_slice("é".as_bytes()); } }
   let text=std::str::from_utf8(&bytes).unwrap();
   let encoded=protocol::text(text);let packet=protocol::envelope(42,&encoded);
   let iterations=(1_000_000/length.max(16)).clamp(1000,50000);
   let mut times=Vec::new();
   for _ in 0..7 {
    let began=Instant::now();
    for _ in 0..iterations { black_box(protocol::parse(black_box(&packet))).unwrap(); }
    times.push(began.elapsed().as_nanos()as f64/iterations as f64);
   }
   println!("{}",serde_json::json!({"length":length,"unicode":unicode,"iterations":iterations,"ns":times}));
  }
 }
}
