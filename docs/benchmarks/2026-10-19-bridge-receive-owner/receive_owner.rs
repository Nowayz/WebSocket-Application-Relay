//! Actual native-frame ownership probe, not a throughput benchmark.
use futures_util::{SinkExt,StreamExt};
use tokio_tungstenite::{connect_async_with_config,tungstenite::{Message,protocol::WebSocketConfig}};
use war_relay::{legacy_bridge,protocol};
#[tokio::main(flavor="current_thread")]
async fn main(){
 let url=std::env::args().nth(1).unwrap();
 let config=WebSocketConfig::default().read_buffer_size(4096).write_buffer_size(0);
 let (mut ws,_)=connect_async_with_config(&url,Some(config),true).await.unwrap();
 ws.send(Message::Binary(protocol::byte_string(b"receive-owner"))).await.unwrap();
 let Message::Binary(welcome)=ws.next().await.unwrap().unwrap() else {panic!("welcome")};
 let id=minicbor::Decoder::new(&welcome).u64().unwrap();assert!(id!=0 && id!=u64::MAX);drop(welcome);
 for length in [0,1,24,1024,4096,65536] {
  let payload=vec![0xa5;length];let packet=protocol::envelope(id,&protocol::byte_string(&payload));
  ws.send(Message::Binary(packet)).await.unwrap();
  let Message::Binary(bytes)=ws.next().await.unwrap().unwrap() else {panic!("binary response")};
  let (sender,value)=protocol::parse(&bytes).unwrap();assert_eq!(sender,id);assert_eq!(minicbor::Decoder::new(value).bytes().unwrap(),payload);
  let unique=bytes.is_unique();let offset=bytes.len()-length;let pointer=bytes.as_ptr().wrapping_add(offset);
  let Message::Binary(converted)=legacy_bridge::from_cbor(&bytes).unwrap().unwrap() else {panic!("binary legacy")};
  assert_eq!(&converted[..8],id.to_le_bytes());assert_eq!(&converted[8..],payload);
  println!("{{\"payload_bytes\":{length},\"receive_unique\":{unique},\"prefix_bytes\":{offset},\"payload_reused\":{}}}",converted.as_ptr().wrapping_add(8)==pointer);
 }
 ws.close(None).await.unwrap();assert!(matches!(ws.next().await,Some(Ok(Message::Close(_)))));ws.flush().await.unwrap();assert!(ws.next().await.is_none());
}
