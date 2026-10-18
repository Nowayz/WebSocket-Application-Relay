use std::{hint::black_box, time::Instant};
use war_relay::protocol;
fn main() {
 for n in [0,16,1024,65536] {
  let value=protocol::byte_string(&vec![0x5a;n]);
  let bytes=protocol::envelope(0x123456789abcdef0,&value);
  assert_eq!(protocol::parse(&bytes).unwrap(),(0x123456789abcdef0,value.as_ref()));
  let mut batches=Vec::new();
  for _ in 0..9 {
   let start=Instant::now();
   for _ in 0..1_000_000 { black_box(protocol::parse(black_box(&bytes)).unwrap()); }
   batches.push(start.elapsed().as_nanos() as f64/1e6);
  }
  println!("{{\"payload_bytes\":{n},\"ns_per_parse_batches\":{batches:?}}}");
 }
}
