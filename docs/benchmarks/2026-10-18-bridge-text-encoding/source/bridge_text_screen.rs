use std::{hint::black_box,time::Instant};
use base64::{Engine,engine::general_purpose::STANDARD};
use tokio_tungstenite::tungstenite::{Message,Utf8Bytes};
use war_relay::legacy_bridge;
fn main() {
 for (label,payload) in [("ascii-1","x".to_owned()),("ascii-16","x".repeat(16)),("utf8-1k","λ".repeat(512)),("utf8-64k","🦀".repeat(16384))] {
  let n=payload.len();let count=if n>=65536 {2048} else {100000};
  let source=Utf8Bytes::from(STANDARD.encode(0x0102030405060708u64.to_le_bytes())+&payload);
  let mut expected=vec![0x82,0x1b,1,2,3,4,5,6,7,8];
  if n<24 {expected.push(0x60+n as u8)} else if n<256 {expected.extend([0x78,n as u8])} else if n<65536 {expected.push(0x79);expected.extend((n as u16).to_be_bytes())} else {expected.push(0x7a);expected.extend((n as u32).to_be_bytes())}
  expected.extend_from_slice(payload.as_bytes());
  assert_eq!(legacy_bridge::to_cbor(Message::Text(source.clone())).unwrap().unwrap().as_ref(),expected);
  for _ in 0..1000 {black_box(legacy_bridge::to_cbor(Message::Text(black_box(source.clone()))).unwrap());}
  let mut batches=Vec::new();
  for _ in 0..9 {let start=Instant::now();for _ in 0..count {black_box(legacy_bridge::to_cbor(Message::Text(black_box(source.clone()))).unwrap());} batches.push(start.elapsed().as_nanos() as f64/count as f64);}
  println!("{{\"case\":\"{label}\",\"payload_bytes\":{n},\"operations_per_batch\":{count},\"ns_per_operation\":{batches:?}}}");
 }
}
