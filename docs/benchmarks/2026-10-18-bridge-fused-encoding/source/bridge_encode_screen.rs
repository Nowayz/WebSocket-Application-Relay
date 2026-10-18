use std::{hint::black_box,time::Instant};
use bytes::Bytes;
use tokio_tungstenite::tungstenite::Message;
use war_relay::legacy_bridge;
fn main() {
 for n in [1,16,1024,65536] {
  let count=if n>=65536 {2048} else {100000};
  let mut source=0x0102030405060708u64.to_le_bytes().to_vec();source.extend((0..n).map(|i|i as u8));let source=Bytes::from(source);
  let mut expected=vec![0x82,0x1b,1,2,3,4,5,6,7,8];
  if n<24 {expected.push(0x40+n as u8)} else if n<256 {expected.extend([0x58,n as u8])} else if n<65536 {expected.push(0x59);expected.extend((n as u16).to_be_bytes())} else {expected.push(0x5a);expected.extend((n as u32).to_be_bytes())}
  expected.extend_from_slice(&source[8..]);
  assert_eq!(legacy_bridge::to_cbor(Message::Binary(source.clone())).unwrap().unwrap().as_ref(),expected);
  for _ in 0..1000 {black_box(legacy_bridge::to_cbor(Message::Binary(black_box(source.clone()))).unwrap());}
  let mut batches=Vec::new();
  for _ in 0..9 {let start=Instant::now();for _ in 0..count {black_box(legacy_bridge::to_cbor(Message::Binary(black_box(source.clone()))).unwrap());} batches.push(start.elapsed().as_nanos() as f64/count as f64);}
  println!("{{\"payload_bytes\":{n},\"operations_per_batch\":{count},\"ns_per_operation\":{batches:?}}}");
 }
}
