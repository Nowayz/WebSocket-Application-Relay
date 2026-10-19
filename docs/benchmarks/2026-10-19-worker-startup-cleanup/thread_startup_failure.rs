//! Linux subprocess fault probe. The launcher supplies a child-only RLIMIT_NPROC.
use std::{sync::{Arc,mpsc},time::Duration};
use war_relay::{Config,Relay,serve};
fn main() {
 let runtime=tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
 let listener=runtime.block_on(tokio::net::TcpListener::bind("127.0.0.1:0")).unwrap();
 let address=listener.local_addr().unwrap();
 let relay=Relay::new(Config{workers:Some(8),shutdown_timeout_secs:1,..Config::default()}).unwrap();
 let mut blockers=Vec::new();
 loop {
  assert!(blockers.len()<128,"child thread limit was not reached");
  let (send,receive)=mpsc::channel();
  match std::thread::Builder::new().name("limit-probe".into()).spawn(move||{let _=receive.recv();}) {
   Ok(thread)=>blockers.push((send,thread)),
   Err(_)=>break,
  }
 }
 let filled=blockers.len();assert!(filled>=2,"not enough child thread slots");
 for _ in 0..2 {let (send,thread)=blockers.pop().unwrap();send.send(()).unwrap();thread.join().unwrap();}
 let error=runtime.block_on(serve(listener,None,relay.clone(),tokio_util::sync::CancellationToken::new())).unwrap_err();
 let owners_after_return=Arc::strong_count(&relay);
 let rebound=std::net::TcpListener::bind(address).is_ok();
 assert!(error.to_string().contains("cannot start native I/O worker"),"{error:#}");
 for (send,thread) in blockers {let _=send.send(());thread.join().unwrap();}
 runtime.block_on(async {for _ in 0..100 {if Arc::strong_count(&relay)==1 {break;}tokio::time::sleep(Duration::from_millis(1)).await;}});
 assert_eq!(Arc::strong_count(&relay),1,"eventual worker retirement");
 println!("{{\"filled_child_thread_slots\":{filled},\"owners_after_return\":{owners_after_return},\"listener_rebind_after_return\":{rebound}}}");
}
