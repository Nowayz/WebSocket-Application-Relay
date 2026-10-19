use super::*;

async fn connected() -> (Socket, WebSocketStream<TcpStream>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let client = tokio_tungstenite::connect_async(format!("ws://{address}"));
    let server = async {
        let (stream, _) = listener.accept().await.unwrap();
        tokio_tungstenite::accept_async(stream).await.unwrap()
    };
    let (client, server) = tokio::join!(client, server);
    (client.unwrap().0, server)
}

#[tokio::test]
async fn ready_frames_and_a_partial_frame_keep_fifo_and_decoder_state() {
    let (mut client, mut server) = connected().await;
    server.get_mut().write_all(&[0x82, 2, 1, 2, 0x82, 2, 3, 4]).await.unwrap();
    assert_eq!(data(&mut client).await.unwrap().as_ref(), &[1, 2]);
    assert_eq!(data(&mut client).await.unwrap().as_ref(), &[3, 4]);
    server.get_mut().write_all(&[0x82, 4, 5]).await.unwrap();
    let tail = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        server.get_mut().write_all(&[6, 7, 8]).await.unwrap();
        server
    });
    let received = data(&mut client);
    tokio::pin!(received);
    std::future::poll_fn(|cx| {
        use std::future::Future;
        assert!(received.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    }).await;
    assert_eq!(received.await.unwrap().as_ref(), &[5, 6, 7, 8]);
    tail.await.unwrap();
}

#[tokio::test]
async fn pending_read_keeps_its_five_second_timeout() {
    let (mut client, _server) = connected().await;
    let began = Instant::now();
    let error = tokio::time::timeout(Duration::from_secs(6), data(&mut client))
        .await.expect("outer deadline should not fire").unwrap_err();
    assert!(error.downcast_ref::<tokio::time::error::Elapsed>().is_some());
    assert!(began.elapsed() >= Duration::from_secs(5));
}

#[tokio::test]
async fn unexpected_message_and_peer_close_remain_errors() {
    let (mut client, mut server) = connected().await;
    server.send(Message::Text("not binary".into())).await.unwrap();
    assert!(data(&mut client).await.unwrap_err().to_string().contains("unexpected"));
    server.close(None).await.unwrap();
    assert!(data(&mut client).await.is_err());
}
