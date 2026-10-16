use super::*;

fn query_escape(value: &str) -> String {
    value
        .bytes()
        .map(|byte| match byte {
            b'+' => "%2B".to_owned(),
            b'/' => "%2F".to_owned(),
            b'=' => "%3D".to_owned(),
            _ => char::from(byte).to_string(),
        })
        .collect()
}

async fn listing(server: &Server, path: &str) -> Vec<serde_json::Value> {
    let response = server.http("GET", path, Some("admin-test")).await;
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    assert!(headers.starts_with("HTTP/1.1 200"), "{response}");
    assert!(headers.contains("content-type: application/json"));
    serde_json::from_str(body).unwrap()
}

#[tokio::test]
async fn bounded_http_pages_preserve_order_binary_names_variables_and_validation() {
    let server = Server::new(config()).await;
    let names: &[&[u8]] = &[
        b"",
        b"\0",
        b"\xff",
        b"\xfb\xff",
        b"abc",
        b"abc\0",
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
        data(&mut socket).await;
        sockets.push(socket);
    }
    command(&mut sockets[0], 4, b"\x82\x43key\xf5").await;
    let expected = listing(&server, "/v1/channels").await;
    assert_eq!(
        listing(&server, "/v1/channels?cache_bust=1").await,
        expected
    );
    for limit in [1, 2, 3, 1000] {
        let mut collected = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut path = format!("/v1/channels?limit={limit}");
            if let Some(after) = &after {
                path.push_str("&after=");
                path.push_str(&query_escape(after));
            }
            let page = listing(&server, &path).await;
            assert!(page.len() <= limit);
            after = page
                .last()
                .map(|row| row["name_base64"].as_str().unwrap().to_owned());
            let done = page.len() < limit;
            collected.extend(page);
            if done {
                break;
            }
        }
        assert_eq!(collected, expected);
    }
    let first = listing(&server, "/v1/channels?limit=1").await;
    assert_eq!(first[0]["name_base64"], "");
    let after_empty = listing(&server, "/v1/channels?limit=1&after=").await;
    assert_ne!(after_empty[0]["name_base64"], "");
    for query in [
        "limit=0",
        "limit=1001",
        "limit=-1",
        "limit=",
        "limit=nan",
        "limit=184467440737095516160",
        "limit=1&limit=2",
        "after=",
        "limit=1&after=YQ",
        "limit=1&after=YR%3D%3D",
        "limit=1&after=%00",
        "limit=1&after=+/8=",
        "limit=1&after=AAAAAAAAAAAAAAAAAAAAAAA%3D",
        "limit=1&after=AAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        "limit=1&after=AA%3D%3D&after=AA%3D%3D",
    ] {
        let path = format!("/v1/channels?{query}");
        for token in [None, Some("wrong")] {
            assert!(
                server
                    .http("GET", &path, token)
                    .await
                    .starts_with("HTTP/1.1 401")
            );
        }
        assert!(
            server
                .http("GET", &path, Some("admin-test"))
                .await
                .starts_with("HTTP/1.1 400"),
            "{query}"
        );
    }
    assert_eq!(listing(&server, "/v1/channels").await, expected);
    server.stop().await;
    drop(sockets);
}

#[tokio::test]
async fn a_cursor_survives_channel_retirement_and_observes_later_membership() {
    let server = Server::new(config()).await;
    let (mut a, _) = server.join("a").await;
    let (b, _) = server.join("b").await;
    let page = listing(&server, "/v1/channels?limit=1").await;
    assert_eq!(page[0]["name"], "a");
    let cursor = query_escape(page[0]["name_base64"].as_str().unwrap());
    a.close(None).await.unwrap();
    assert!(matches!(
        a.next().await.unwrap().unwrap(),
        Message::Close(_)
    ));
    let (behind, _) = server.join("\0").await;
    let (ahead, _) = server.join("c").await;
    let (b2, _) = server.join("b").await;
    let page = listing(&server, &format!("/v1/channels?limit=100&after={cursor}")).await;
    let expected = listing(&server, "/v1/channels")
        .await
        .into_iter()
        .filter(|row| row["name_base64"].as_str().unwrap() > "YQ==")
        .collect::<Vec<_>>();
    assert_eq!(page, expected);
    assert_eq!(
        page.iter().find(|row| row["name"] == "b").unwrap()["members"],
        2
    );
    assert!(page.iter().any(|row| row["name"] == "c"));
    assert!(!page.iter().any(|row| row["name"] == "\0"));
    server.stop().await;
    drop((a, b, behind, ahead, b2));
}

#[tokio::test]
async fn large_escaped_channel_names_preserve_the_complete_http_response() {
    let server = Server::new(config()).await;
    let mut sockets = Vec::new();
    for last in 1..=32 {
        let mut name = [1; 16];
        name[15] = last;
        let (socket, _) = server.join(std::str::from_utf8(&name).unwrap()).await;
        sockets.push(socket);
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
    assert_eq!(rows.as_array().unwrap().len(), 33);
    // Six-byte JSON escapes make these rows exceed a normal row-size hint.
    // Check every name, including the final row after the buffer has grown.
    for last in 1..=32 {
        let mut name = [1; 16];
        name[15] = last;
        assert_eq!(
            rows.as_array()
                .unwrap()
                .iter()
                .filter(|row| row["name"] == std::str::from_utf8(&name).unwrap())
                .count(),
            1
        );
    }
    server.stop().await;
    drop(sockets);
}

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
