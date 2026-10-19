#[path = "timer-retime-control.rs"] mod timers;
use std::{hint::black_box, time::{Duration,Instant}};
fn main() {
 for mode in ["postpone", "advance", "mixed", "unchanged"] {
  let mut timers=timers::Timers::default();let base=Instant::now();let n=100000usize;
  for slot in 0..n {timers.set(slot,Some(base+Duration::from_nanos((slot+10000000) as u64)));}
  let began=Instant::now();
  for round in 1..=8usize {for slot in 0..n {
   let offset=match mode {"postpone"=>10000000+round*n+slot,"advance"=>10000000-round*n+slot,"mixed"=>10000000+((slot*7919+round*104729)%1000000),_=>10000000+slot};
   timers.set(black_box(slot),Some(base+Duration::from_nanos(offset as u64)));
  }}
  let elapsed=began.elapsed().as_secs_f64();
  let mut count=0;while let Some(at)=timers.next(){assert!(timers.pop_due(at).is_some());count+=1;}assert_eq!(count,n);
  println!("{} {:.9}",mode,elapsed);
 }
}
