//! Isolated conversion/allocator diagnostic; not relay throughput.
use std::{hint::black_box,io::{self,Write},time::Instant};
use bytes::Bytes;
use tokio_tungstenite::tungstenite::Message;
use war_relay::{legacy_bridge,protocol};
fn main() {
 let args:Vec<_>=std::env::args().collect();let length:usize=args[1].parse().unwrap();let op:u8=args[2].parse().unwrap();assert!(op==0 || op==5);
 let mut input=vec![0;8];input.push(op);input.resize(input.len()+length,b'a');let input=Bytes::from(input);
 let expected=protocol::request(op,&protocol::byte_string(&vec![b'a';length]));
 assert_eq!(legacy_bridge::to_cbor(Message::Binary(input.clone())).unwrap().unwrap(),expected);
 if args.get(3).is_some_and(|s|s=="heap") {
  let mut line=String::with_capacity(16);io::stdin().read_line(&mut line).unwrap();
  println!("READY");io::stdout().flush().unwrap();line.clear();io::stdin().read_line(&mut line).unwrap();
  for _ in 0..1000 {black_box(legacy_bridge::to_cbor(Message::Binary(black_box(input.clone()))).unwrap().unwrap());}
  println!("DONE");io::stdout().flush().unwrap();line.clear();io::stdin().read_line(&mut line).unwrap();
 } else {
  let count=if length>1024 {10000} else {100000};
  for _ in 0..1000 {black_box(legacy_bridge::to_cbor(Message::Binary(input.clone())).unwrap());}
  let start=Instant::now();
  for _ in 0..count {black_box(legacy_bridge::to_cbor(Message::Binary(black_box(input.clone()))).unwrap().unwrap());}
  println!("{{\"payload_bytes\":{length},\"iterations\":{count},\"ns_per_conversion\":{}}}",start.elapsed().as_nanos() as f64/count as f64);
 }
}
