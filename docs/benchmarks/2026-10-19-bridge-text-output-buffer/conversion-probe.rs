//! Focused legacy output conversion probe; not relay throughput.
use std::{hint::black_box,time::Instant};
use war_relay::{legacy_bridge,protocol};
fn main() {
 for length in [0,1,12,64,1024,65536] {
  let payload="é".repeat(length/2)+if length%2==1 {"x"} else {""};
  let packet=protocol::envelope(42,&protocol::text(&payload));
  let message=legacy_bridge::from_cbor(&packet).unwrap().unwrap();
  assert_eq!(message.to_text().unwrap(),format!("KgAAAAAAAAA={payload}"));
  let iterations=if length>1024 {10000} else {100000};
  for _ in 0..1000 {black_box(legacy_bridge::from_cbor(black_box(&packet)).unwrap());}
  let started=Instant::now();
  for _ in 0..iterations {black_box(legacy_bridge::from_cbor(black_box(&packet)).unwrap());}
  println!("{{\"payload_bytes\":{length},\"iterations\":{iterations},\"ns_per_conversion\":{}}}",started.elapsed().as_nanos() as f64/iterations as f64);
 }
}
