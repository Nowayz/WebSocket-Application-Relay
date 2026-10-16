use super::*;

#[tokio::test]
async fn http_channel_listing_preserves_exact_json_and_authentication() {
    let server = Server::new(config()).await;
    let names: &[&[u8]] = &[
        b"",
        b"\0",
        b"\xff",
        b"abc",
        b"abc\0",
        b"\"\\\n",
        "π世界".as_bytes(),
        b"1234567890abcdef",
    ];
    let mut sockets = Vec::new();
    for name in names {
        let mut request = server.url.as_str().into_client_request().unwrap();
        request.headers_mut().insert(
            "sec-websocket-protocol",
            protocol::SUBPROTOCOL.parse().unwrap(),
        );
        let (mut socket, _) = connect_async(request).await.unwrap();
        socket
            .send(Message::Binary(protocol::byte_string(name)))
            .await
            .unwrap();
        let welcome = data(&mut socket).await;
        assert_ne!(Decoder::new(&welcome).u64().unwrap(), 0);
        sockets.push(socket);
    }
    assert_eq!(
        command(&mut sockets[0], 4, b"\x82\x43key\xf5").await,
        protocol::reply(4, b"\xf5")
    );
    for token in [None, Some("wrong")] {
        assert!(
            server
                .http("GET", "/v1/channels", token)
                .await
                .starts_with("HTTP/1.1 401")
        );
    }
    let response = server.http("GET", "/v1/channels", Some("admin-test")).await;
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("HTTP/1.1 200"));
    assert!(headers.contains("content-type: application/json"));
    assert_eq!(
        body,
        serde_json::to_string(&server.relay.channels()).unwrap()
    );
    let rows: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(rows.as_array().unwrap().len(), names.len() + 1);
    assert_eq!(
        rows[0],
        serde_json::json!({"name":"", "name_base64":"", "members":1, "variables":1, "variable_bytes":4})
    );
    assert_eq!(
        rows[1],
        serde_json::json!({"name":null, "name_base64":"/w==", "members":1, "variables":0, "variable_bytes":0})
    );
    server.stop().await;
    drop(sockets);
}
