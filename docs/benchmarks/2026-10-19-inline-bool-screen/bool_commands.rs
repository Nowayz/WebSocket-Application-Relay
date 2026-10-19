//! Fixed-work actual-socket authentication reply probe, not an application workload.
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async_with_config,tungstenite::Message};
use war_relay::protocol;
#[tokio::main(flavor="multi_thread",worker_threads=4)]
async fn main() {
 let url=std::env::args().nth(1).expect("URL");
 let mut tasks=tokio::task::JoinSet::new();
 for index in 0..8 {
  let url=url.clone();
  tasks.spawn(async move {
   let (mut ws,_)=connect_async_with_config(&url,None,true).await.unwrap();
   ws.send(Message::Binary(protocol::byte_string(format!("bool-{index}").as_bytes()))).await.unwrap();
   let Message::Binary(welcome)=ws.next().await.unwrap().unwrap() else {panic!("welcome")};
   let id=minicbor::Decoder::new(&welcome).u64().unwrap();assert!(id!=0 && id!=u64::MAX);
   let request=protocol::request(0,&protocol::byte_string(b"bool-output-check"));
   let reply=Message::Binary(protocol::reply(0,b"\xf5"));
   for _ in 0..12500 {
    ws.send(Message::Binary(request.clone())).await.unwrap();
    assert_eq!(ws.next().await.unwrap().unwrap(),reply);
   }
   ws.close(None).await.unwrap();
   // Consume the peer Close and flush its acknowledgement.
   loop {
    match ws.next().await {
     Some(Ok(Message::Close(_)))=>{ws.flush().await.unwrap();break;}
     Some(Ok(Message::Binary(_)))=>{}
     other=>panic!("close: {other:?}"),
    }
   }
  });
 }
 while let Some(result)=tasks.join_next().await {result.unwrap();}
 println!("verified 100000 boolean acknowledgements");
}
