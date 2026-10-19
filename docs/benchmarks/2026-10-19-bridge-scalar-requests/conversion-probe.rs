//! Isolated conversion probe, not relay throughput.
use std::{hint::black_box,io::{self,Write},time::Instant};
use bytes::Bytes;
use tokio_tungstenite::tungstenite::Message;
use war_relay::{legacy_bridge,protocol};
fn main(){
 let args:Vec<_>=std::env::args().collect();let op:u8=args[1].parse().unwrap();let value:u64=args[2].parse().unwrap();
 let mut input=vec![0;8];input.push(op);
 let encoded=match op {
  1=>{input.push(u8::try_from(value).unwrap());protocol::uint(value)},
  3=>{input.extend_from_slice(&value.to_le_bytes());protocol::uint(value)},
  0|5=>{let payload=vec![b'a';value as usize];input.extend_from_slice(&payload);protocol::byte_string(&payload)},
  4=>{let payload=vec![0xa5;value as usize];input.extend_from_slice(&[3,b'k',b'e',b'y']);input.extend_from_slice(&payload);let mut pair=vec![0x82];pair.extend_from_slice(&protocol::byte_string(b"key"));pair.extend_from_slice(&protocol::byte_string(&payload));pair.into()},
  _=>panic!("unsupported probe command")
 };
 let input=Bytes::from(input);let expected=protocol::request(op,&encoded);
 assert_eq!(legacy_bridge::to_cbor(Message::Binary(input.clone())).unwrap().unwrap(),expected);
 if args.get(3).is_some_and(|s|s=="heap"){
  let mut line=String::with_capacity(16);io::stdin().read_line(&mut line).unwrap();println!("READY");io::stdout().flush().unwrap();line.clear();io::stdin().read_line(&mut line).unwrap();
  for _ in 0..1000 {black_box(legacy_bridge::to_cbor(Message::Binary(black_box(input.clone()))).unwrap().unwrap());}
  println!("DONE");io::stdout().flush().unwrap();line.clear();io::stdin().read_line(&mut line).unwrap();
 }else{
  let count=if op==4 {10000}else{100000};for _ in 0..10000 {black_box(legacy_bridge::to_cbor(Message::Binary(input.clone())).unwrap());}
  let mut samples=Vec::with_capacity(7);
  for _ in 0..7 {
   let start=Instant::now();for _ in 0..count {black_box(legacy_bridge::to_cbor(Message::Binary(black_box(input.clone()))).unwrap().unwrap());}
   samples.push(start.elapsed().as_nanos() as f64/count as f64);
  }
  let mut sorted=samples.clone();sorted.sort_by(f64::total_cmp);let median=sorted[3];
  println!("{{\"op\":{op},\"value\":{value},\"iterations_per_batch\":{count},\"ns_per_conversion\":{median},\"samples_ns\":{samples:?}}}");
 }
}
