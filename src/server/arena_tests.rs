use super::*;
use std::io::Read;
use tungstenite::protocol::Role;

#[test]
fn compaction_preserves_live_tokens_partial_input_queued_output_and_close_deadlines() {
    let relay = Relay::new(crate::Config {
        ping_interval_secs: 0,
        ..crate::Config::default()
    })
    .unwrap();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut worker = Worker::new(listener, relay.clone()).unwrap();
    let mut clients = Vec::new();
    for _ in 0..512 {
        let client = std::net::TcpStream::connect(address).unwrap();
        client.set_nodelay(true).unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        worker.accept().unwrap();
        clients.push(Some(WebSocket::from_raw_socket(client, Role::Client, None)));
    }
    assert_eq!(worker.connections.len(), 512);
    let original_capacity = worker.connections.capacity();
    worker.reclaim_connections();
    assert_eq!(worker.connections.capacity(), original_capacity);
    assert!(!worker.compacted, "a dense arena must keep its capacity");

    // Keep an unfinished HTTP upgrade, two joined decoders and a Close in
    // flight, including live slots near the end of the large arena.
    let survivors = [0, 193, 450, 511];
    let upgrade = b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: war.cbor.v1\r\n\r\n";
    clients[0]
        .as_mut()
        .unwrap()
        .get_mut()
        .write_all(&upgrade[..73])
        .unwrap();
    assert!(worker.connections[0].drive(&relay, &mut worker.scratch, false) == Drive::Wait);
    for &slot in &survivors[1..] {
        let connection = &mut worker.connections[slot];
        let mut ws = WebSocket::from_raw_socket(
            Stream {
                socket: connection.socket.clone(),
                remaining_upgrade: None,
            },
            Role::Server,
            Some(
                WebSocketConfig::default()
                    .read_buffer_size(4096)
                    .shared_read_buffer(true),
            ),
        );
        ws.set_read_budget(connection.handshake_lease.take().unwrap())
            .unwrap();
        connection.stage = Stage::WebSocket(ws);
        connection.registration = Some(
            relay
                .subscribe_with_output(
                    format!("survivor{slot}").as_bytes(),
                    connection.socket.clone(),
                )
                .unwrap(),
        );
        assert!(matches!(
            clients[slot].as_mut().unwrap().read().unwrap(),
            Message::Binary(_)
        ));
    }
    let closing_deadline = Instant::now() + Duration::from_secs(10);
    assert!(
        worker.connections[450].start_close(
            Disconnect::IDLE,
            &relay,
            false,
            closing_deadline - relay.config.write_timeout()
        ) != Drive::Drop
    );

    let id = worker.connections[193]
        .registration
        .as_ref()
        .unwrap()
        .peer
        .id
        .load(Relaxed);
    let value = protocol::byte_string(&vec![0xa5; 32 * 1024]);
    let packet = protocol::envelope(id, &value);
    let pointer = packet.as_ptr();
    worker.connections[193].socket.state.lock().control_pending = true;
    relay
        .receive(
            &worker.connections[193].registration.as_ref().unwrap().peer,
            packet.clone(),
        )
        .unwrap();
    assert!(
        !packet.is_unique(),
        "queued output must share the original body"
    );

    let partial_id = worker.connections[511]
        .registration
        .as_ref()
        .unwrap()
        .peer
        .id
        .load(Relaxed);
    let partial_packet = protocol::envelope(partial_id, &value);
    let mask = [0x91, 0x65, 0x39, 0xcd];
    let mut wire = vec![0x82, 0xfe];
    wire.extend_from_slice(&(partial_packet.len() as u16).to_be_bytes());
    wire.extend_from_slice(&mask);
    wire.extend(
        partial_packet
            .iter()
            .enumerate()
            .map(|(i, byte)| byte ^ mask[i & 3]),
    );
    clients[511]
        .as_mut()
        .unwrap()
        .get_mut()
        .write_all(&wire[..17])
        .unwrap();
    assert!(worker.connections[511].drive(&relay, &mut worker.scratch, false) == Drive::Wait);
    worker.connections[511].reclaim_deadline = Some(closing_deadline + Duration::from_secs(1));

    let mut expected = Vec::new();
    for &slot in &survivors {
        let connection = &worker.connections[slot];
        let deadline = connection.deadline(&relay);
        expected.push((
            connection.token,
            slot,
            Arc::as_ptr(&connection.socket),
            deadline,
        ));
        worker.timers.set(slot, deadline);
    }
    let ready: Vec<_> = worker.ready.iter().copied().collect();
    let pending: Vec<_> = worker.signal.pending.lock().iter().copied().collect();
    for (slot, client) in clients.iter_mut().enumerate() {
        if !survivors.contains(&slot) {
            worker.timers.remove(slot);
            let connection = worker.connections.remove(slot);
            worker.tokens.remove(&connection.token);
            drop(connection);
            client.take();
        }
    }
    worker.reclaim_connections();
    assert!(worker.connections.capacity() < original_capacity / 8);
    assert_eq!(worker.connections.len(), survivors.len());
    assert_eq!(worker.ready.iter().copied().collect::<Vec<_>>(), ready);
    assert_eq!(
        worker
            .signal
            .pending
            .lock()
            .iter()
            .copied()
            .collect::<Vec<_>>(),
        pending
    );
    let mut moved = 0;
    let mut deadlines = Vec::new();
    for (token, old_slot, pointer, deadline) in expected {
        let slot = worker.tokens[&token];
        moved += usize::from(slot != old_slot);
        let connection = &worker.connections[slot];
        assert_eq!(connection.token, token);
        assert_eq!(connection.socket.token, token);
        assert_eq!(Arc::as_ptr(&connection.socket), pointer);
        assert_eq!(connection.deadline(&relay), deadline);
        if let Some(deadline) = deadline {
            deadlines.push((deadline, slot));
        }
    }
    assert!(moved >= 2, "fixture must move live decoder records");
    deadlines.sort_unstable();
    for (deadline, slot) in deadlines {
        assert_eq!(worker.timers.pop_due(deadline), Some(slot));
    }
    assert_eq!(worker.timers.next(), None);

    // The partially parsed HTTP state moved with its connection. Complete the
    // same upgrade, then verify a full CBOR body through the resulting decoder.
    let upgrade_slot = worker.tokens[&Token(2)];
    clients[0]
        .as_mut()
        .unwrap()
        .get_mut()
        .write_all(&upgrade[73..])
        .unwrap();
    assert!(
        worker.connections[upgrade_slot].drive(&relay, &mut worker.scratch, false) == Drive::Wait
    );
    let mut response = Vec::new();
    while !response.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        clients[0]
            .as_mut()
            .unwrap()
            .get_mut()
            .read_exact(&mut byte)
            .unwrap();
        response.push(byte[0]);
        assert!(response.len() < 8192);
    }
    assert!(response.starts_with(b"HTTP/1.1 101 "));
    assert!(
        String::from_utf8(response)
            .unwrap()
            .to_ascii_lowercase()
            .contains("sec-websocket-protocol: war.cbor.v1\r\n")
    );
    let client = clients[0].as_mut().unwrap();
    client
        .send(Message::Binary(crate::protocol::byte_string(
            b"survivor-http",
        )))
        .unwrap();
    assert!(
        worker.connections[upgrade_slot].drive(&relay, &mut worker.scratch, false) == Drive::Wait
    );
    let Message::Binary(welcome) = client.read().unwrap() else {
        panic!("expected uint64 welcome after the resumed upgrade")
    };
    let mut decoder = minicbor::Decoder::new(&welcome);
    let upgrade_id = decoder.u64().unwrap();
    assert_eq!(decoder.position(), welcome.len());
    let upgrade_packet =
        crate::protocol::envelope(upgrade_id, &crate::protocol::byte_string(&[0x6c; 1024]));
    client
        .send(Message::Binary(upgrade_packet.clone()))
        .unwrap();
    assert!(
        worker.connections[upgrade_slot].drive(&relay, &mut worker.scratch, false) == Drive::Wait
    );
    assert_eq!(client.read().unwrap(), Message::Binary(upgrade_packet));

    let queued_slot = worker.tokens[&Token(195)];
    assert_eq!(packet.as_ptr(), pointer);
    assert!(!packet.is_unique());
    worker.connections[queued_slot]
        .socket
        .control_flushed()
        .unwrap();
    worker.connections[queued_slot].socket.flush().unwrap();
    assert_eq!(
        clients[193].as_mut().unwrap().read().unwrap(),
        Message::Binary(packet.clone())
    );
    assert!(packet.is_unique());

    let partial_slot = worker.tokens[&Token(513)];
    clients[511]
        .as_mut()
        .unwrap()
        .get_mut()
        .write_all(&wire[17..])
        .unwrap();
    assert!(
        worker.connections[partial_slot].drive(&relay, &mut worker.scratch, false) == Drive::Wait
    );
    assert_eq!(
        clients[511].as_mut().unwrap().read().unwrap(),
        Message::Binary(partial_packet)
    );
    let closing_slot = worker.tokens[&Token(452)];
    assert!(matches!(
        clients[450].as_mut().unwrap().read().unwrap(),
        Message::Close(_)
    ));
    clients[450].as_mut().unwrap().flush().unwrap();
    assert!(
        worker.connections[closing_slot].drive(&relay, &mut worker.scratch, false) == Drive::Drop
    );
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);

    // Reusing a vacated slot must not make an old readiness token address the
    // new socket. Normal acceptance allocates a fresh token after compaction.
    let new_client = std::net::TcpStream::connect(address).unwrap();
    worker.accept().unwrap();
    assert!(worker.tokens.contains_key(&Token(514)));
    assert!(
        ready
            .iter()
            .filter_map(|token| worker.tokens.get(token))
            .all(|&slot| worker.connections[slot].token != Token(514))
    );
    drop(new_client);
    worker.connections.clear();
    worker.tokens.clear();
    worker.reclaim_connections();
    assert_eq!(worker.connections.capacity(), 0);
    assert!(!worker.compacted);
    assert_eq!(relay.metrics.connections.load(Relaxed), 0);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.read_budget.as_ref().unwrap().reserved(), 0);
    assert_eq!(
        clients[0]
            .as_mut()
            .unwrap()
            .get_mut()
            .read(&mut [0])
            .unwrap(),
        0
    );
}
