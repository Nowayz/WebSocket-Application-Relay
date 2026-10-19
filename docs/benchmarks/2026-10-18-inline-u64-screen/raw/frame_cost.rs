use std::{hint::black_box,time::Instant};
use war_relay::protocol;
#[path = "../src/frame.rs"]
mod frame;
fn main() {
 for value in [1u64, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX] {
  let frame = frame::Frame::reply_uint(200,value);
  for clones in [0,8] {
   let mut times=Vec::new();
   for _ in 0..7 {
    let began=Instant::now();
    for _ in 0..200_000 {
     let frame=black_box(frame::Frame::reply_uint(black_box(200),black_box(value)));
     for _ in 0..clones { black_box(frame.clone()); }
     black_box(frame);
    }
    times.push(began.elapsed().as_nanos() as f64/200_000.0);
   }
   println!("{}",serde_json::json!({"value":value,"clones":clones,"frame_bytes":std::mem::size_of::<frame::Frame>(),"prefix_bytes":frame.prefix().len(),"payload_bytes":frame.payload().len(),"ns_per_operation":times}));
  }
 }
}
