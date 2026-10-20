use super::*;

#[test]
fn input_batch_defers_only_cumulative_statistics_and_publishes_once() {
    let relay = Relay::new(config()).unwrap();
    let mut sender = relay.subscribe(b"batch").unwrap();
    let mut recipient = relay.subscribe(b"batch").unwrap();
    sender.outgoing.try_recv().unwrap();
    recipient.outgoing.try_recv().unwrap();
    let bytes = protocol::envelope(recipient.registration.peer.id.load(Relaxed), &[0xf5]);
    let mut batch = relay.receive_batch();
    batch
        .receive(&sender.registration.peer, bytes.clone(), 4096)
        .unwrap();
    let held = recipient.outgoing.try_recv().unwrap();
    assert_eq!(relay.metrics.incoming_messages.load(Relaxed), 0);
    assert_eq!(relay.metrics.routing_reused_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
    assert_eq!(
        recipient.registration.peer.budget.used.load(Relaxed),
        held.frame.len()
    );
    assert_eq!(
        batch.receive(&sender.registration.peer, Bytes::from_static(b"\xff"), 4096),
        Err(Disconnect::PROTOCOL)
    );
    batch.publish();
    batch.publish();
    assert_eq!(relay.metrics.incoming_messages.load(Relaxed), 2);
    assert_eq!(
        relay.metrics.incoming_bytes.load(Relaxed),
        bytes.len() as u64 + 1
    );
    assert_eq!(relay.metrics.routing_reused_bytes.load(Relaxed), 1);
    batch
        .receive(&sender.registration.peer, bytes.clone(), 4096)
        .unwrap();
    drop(batch);
    assert_eq!(relay.metrics.incoming_messages.load(Relaxed), 3);
    assert_eq!(
        relay.metrics.incoming_bytes.load(Relaxed),
        bytes.len() as u64 * 2 + 1
    );
    assert_eq!(relay.metrics.routing_reused_bytes.load(Relaxed), 2);
    drop((sender, recipient));
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
    drop(held);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn input_batches_keep_instance_identity_and_publish_during_unwind() {
    let first = Relay::new(config()).unwrap();
    let second = Relay::new(config()).unwrap();
    let mut one = first.subscribe(b"batch").unwrap();
    let mut two = second.subscribe(b"batch").unwrap();
    one.outgoing.try_recv().unwrap();
    two.outgoing.try_recv().unwrap();
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let mut a = first.receive_batch();
            let mut b = second.receive_batch();
            a.receive(
                &one.registration.peer,
                protocol::envelope(one.registration.peer.id.load(Relaxed), &[0xf5]),
                4096,
            )
            .unwrap();
            b.receive(
                &two.registration.peer,
                protocol::envelope(two.registration.peer.id.load(Relaxed), &[0xf5]),
                4096,
            )
            .unwrap();
            b.publish();
            assert_eq!(first.metrics.incoming_messages.load(Relaxed), 0);
            assert_eq!(second.metrics.incoming_messages.load(Relaxed), 1);
            panic!("interrupted ready batch");
        }))
        .is_err()
    );
    assert_eq!(first.metrics.incoming_messages.load(Relaxed), 1);
    assert_eq!(second.metrics.incoming_messages.load(Relaxed), 1);
    drop((one, two));
    assert_eq!(first.metrics.queued_bytes.load(Relaxed), 0);
    assert_eq!(second.metrics.queued_bytes.load(Relaxed), 0);
}

fn config() -> Config {
    Config {
        auth_password: Some("test-password".into()),
        ..Config::default()
    }
}

#[test]
fn variable_reply_keeps_stored_bytes_after_replacement_and_channel_retirement() {
    let relay = Relay::new(config()).unwrap();
    let mut sub = relay.subscribe(b"variables").unwrap();
    sub.outgoing.try_recv().unwrap();
    let value = protocol::byte_string(&vec![0xa5; 32768]);
    let mut input = b"\x82\x44body".to_vec();
    input.extend_from_slice(&value);
    relay
        .receive(&sub.registration.peer, protocol::request(4, &input))
        .unwrap();
    sub.outgoing.try_recv().unwrap();
    let pointer = relay
        .channel(b"variables")
        .unwrap()
        .variable(b"body")
        .unwrap()
        .as_ptr();
    relay
        .receive(&sub.registration.peer, protocol::request(5, b"\x44body"))
        .unwrap();
    let held = sub.outgoing.try_recv().unwrap();
    assert_eq!(held.frame.payload().as_ptr(), pointer);
    assert_eq!(held.frame.payload(), value.as_ref());
    assert_eq!(held.frame.prefix(), b"\x82\x00\x82\x18\xc8\x82\xf5");
    relay
        .receive(
            &sub.registration.peer,
            protocol::request(4, b"\x82\x44body\xf5"),
        )
        .unwrap();
    sub.outgoing.try_recv().unwrap();
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 5);
    drop(sub);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
    assert_eq!(held.frame.payload().as_ptr(), pointer);
    assert_eq!(held.frame.payload(), value.as_ref());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
    drop(held);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn fanout_shares_the_received_value_and_keeps_it_alive_after_receive_returns() {
    let relay = Relay::new(config()).unwrap();
    let mut sender = relay.subscribe(b"a").unwrap();
    let mut first = relay.subscribe(b"a").unwrap();
    let mut second = relay.subscribe(b"a").unwrap();
    for subscription in [&mut sender, &mut first, &mut second] {
        subscription.outgoing.try_recv().unwrap();
    }
    let value = protocol::byte_string(&[0x5a; 2048]);
    let input = protocol::envelope(BROADCAST, &value);
    let pointer = protocol::parse(&input).unwrap().1.as_ptr();
    relay.receive(&sender.registration.peer, input).unwrap();
    let first_frame = first.outgoing.try_recv().unwrap();
    let second_frame = second.outgoing.try_recv().unwrap();
    assert_eq!(first_frame.frame.payload().as_ptr(), pointer);
    assert_eq!(second_frame.frame.payload().as_ptr(), pointer);
    assert_eq!(first_frame.frame.payload(), value.as_ref());
    assert_eq!(
        relay.metrics.routing_reused_bytes.load(Relaxed),
        value.len() as u64
    );
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    drop(sender);
    drop(first);
    drop(second);
    // In-flight envelopes still own their byte reservations and receive storage.
    assert_eq!(first_frame.frame.payload(), value.as_ref());
    assert!(relay.metrics.queued_bytes.load(Relaxed) > 0);
    drop(first_frame);
    drop(second_frame);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn tiny_views_of_a_large_current_allocation_are_compacted() {
    let relay = Relay::new(config()).unwrap();
    let mut sender = relay.subscribe(b"a").unwrap();
    let mut recipient = relay.subscribe(b"a").unwrap();
    sender.outgoing.try_recv().unwrap();
    recipient.outgoing.try_recv().unwrap();
    let small = protocol::envelope(recipient.registration.peer.id.load(Relaxed), &[0xf6]);
    let mut read_slab = bytes::BytesMut::with_capacity(65536);
    read_slab.extend_from_slice(&small);
    let input = read_slab.split().freeze();
    let backing = input.clone();
    let pointer = protocol::parse(&input).unwrap().1.as_ptr();
    relay
        .receive_with_storage(&sender.registration.peer, input, 65536)
        .unwrap();
    let packet = recipient.outgoing.try_recv().unwrap();
    assert_ne!(packet.frame.payload().as_ptr(), pointer);
    assert_eq!(packet.frame.payload(), [0xf6]);
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 1);
    assert!(!backing.is_empty());
    drop(packet);
    drop(sender);
    drop(recipient);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn slow_consumer_is_closed_and_every_byte_reservation_is_released() {
    let relay = Relay::new(Config {
        queue_messages: 1,
        ..config()
    })
    .unwrap();
    let sender = relay.subscribe(b"a").unwrap();
    let recipient = relay.subscribe(b"a").unwrap();
    // Recipient queue still holds its welcome; the next routed frame exceeds its message budget.
    relay
        .receive(
            &sender.registration.peer,
            protocol::envelope(BROADCAST, &[1]),
        )
        .unwrap();
    assert_eq!(*recipient.close.borrow(), Some(Disconnect::OVERLOADED));
    assert_eq!(relay.metrics.slow_consumers.load(Relaxed), 1);
    drop(recipient);
    drop(sender);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn takeover_cleanup_cannot_remove_new_id_owner() {
    let relay = Relay::new(config()).unwrap();
    let mut first = relay.subscribe(b"a").unwrap();
    let mut next = relay.subscribe(b"a").unwrap();
    first.outgoing.try_recv().unwrap();
    next.outgoing.try_recv().unwrap();
    let id = first.registration.peer.id.load(Relaxed);
    let new_peer = &next.registration.peer;
    relay
        .receive(
            new_peer,
            protocol::request(0, &protocol::text("test-password")),
        )
        .unwrap();
    relay
        .receive(new_peer, protocol::request(3, &protocol::uint(id)))
        .unwrap();
    assert_eq!(*first.close.borrow(), Some(Disconnect::TAKEN));
    drop(first);
    assert!(relay.user(id).is_some_and(|p| p.key == new_peer.key));
    drop(next);
    assert_eq!(relay.counts(), (0, 1));
}

#[test]
fn sparse_id_registry_rehash_preserves_owners_and_in_flight_payloads() {
    let relay = Relay::new(config()).unwrap();
    let mut old = relay.subscribe(b"old").unwrap();
    let mut owner = relay.subscribe(b"owner").unwrap();
    let mut source = relay.subscribe(b"source").unwrap();
    for subscription in [&mut old, &mut owner, &mut source] {
        subscription.outgoing.try_recv().unwrap();
    }
    let id = old.registration.peer.id.load(Relaxed);
    let source_id = source.registration.peer.id.load(Relaxed);
    relay
        .receive(
            &owner.registration.peer,
            protocol::request(0, &protocol::text("test-password")),
        )
        .unwrap();
    relay
        .receive(
            &owner.registration.peer,
            protocol::request(3, &protocol::uint(id)),
        )
        .unwrap();
    assert_eq!(*old.close.borrow(), Some(Disconnect::TAKEN));
    owner.outgoing.try_recv().unwrap();
    owner.outgoing.try_recv().unwrap();
    let fillers: Vec<_> = (0..1024)
        .map(|i| {
            let mut subscription = relay.subscribe(format!("burst-{i}").as_bytes()).unwrap();
            subscription.outgoing.try_recv().unwrap();
            subscription
        })
        .collect();
    let peak_capacity = relay.users_capacity();
    assert!(peak_capacity >= 1026);
    let value = protocol::byte_string(&[0x5a; 8192]);
    let input = protocol::envelope(id, &value);
    let payload_pointer = protocol::parse(&input).unwrap().1.as_ptr();
    relay.receive(&source.registration.peer, input).unwrap();
    let packet = owner.outgoing.try_recv().unwrap();
    assert_eq!(packet.frame.payload().as_ptr(), payload_pointer);
    drop(fillers);
    {
        // Sharded ownership keeps each shard small; no shard grows past the
        // shrink threshold here, so only the total is bounded by its peak.
        assert_eq!(relay.users_len(), 2);
        assert!(relay.users_capacity() <= peak_capacity);
        assert!(Arc::ptr_eq(
            &relay.user(id).unwrap(),
            &owner.registration.peer
        ));
        assert!(Arc::ptr_eq(
            &relay.user(source_id).unwrap(),
            &source.registration.peer
        ));
    }
    // The displaced owner is still registered. Its delayed cleanup cannot
    // erase the replacement after rehashing the ownership table.
    drop(old);
    assert!(Arc::ptr_eq(
        &relay.user(id).unwrap(),
        &owner.registration.peer
    ));
    relay
        .receive(
            &owner.registration.peer,
            protocol::envelope(source_id, &[0x18, 42]),
        )
        .unwrap();
    let reply = source.outgoing.try_recv().unwrap();
    assert_eq!(
        [reply.frame.prefix(), reply.frame.payload()].concat(),
        protocol::envelope(id, &[0x18, 42])
    );
    drop(reply);
    drop(source);
    drop(owner);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.users_len(), 0);
    assert_eq!(packet.frame.payload().as_ptr(), payload_pointer);
    assert_eq!(packet.frame.payload(), value.as_ref());
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), packet.frame.len());
    drop(packet);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn takeovers_reclaim_sparse_id_slots_before_displaced_sessions_finish() {
    let relay = Relay::new(config()).unwrap();
    let mut subscriptions: Vec<_> = (0..1024)
        .map(|i| {
            let mut sub = relay.subscribe(format!("owner-{i}").as_bytes()).unwrap();
            sub.outgoing.try_recv().unwrap();
            sub
        })
        .collect();
    let id = subscriptions[0].registration.peer.id.load(Relaxed);
    let peak_capacity = relay.users_capacity();
    assert!(peak_capacity >= 1024);
    for i in 1..subscriptions.len() {
        let peer = subscriptions[i].registration.peer.clone();
        relay
            .receive(
                &peer,
                protocol::request(0, &protocol::text("test-password")),
            )
            .unwrap();
        subscriptions[i].outgoing.try_recv().unwrap();
        relay
            .receive(&peer, protocol::request(3, &protocol::uint(id)))
            .unwrap();
        subscriptions[i].outgoing.try_recv().unwrap();
        assert_eq!(
            *subscriptions[i - 1].close.borrow(),
            Some(Disconnect::TAKEN)
        );
    }
    {
        assert_eq!(relay.counts().0, 1024);
        assert_eq!(relay.users_len(), 1);
        assert!(relay.users_capacity() <= peak_capacity);
        assert!(Arc::ptr_eq(
            &relay.user(id).unwrap(),
            &subscriptions.last().unwrap().registration.peer
        ));
    }
    // All displaced registrations still exist until this point.
    drop(subscriptions);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn sparse_membership_keeps_peers_variables_and_published_payloads_alive() {
    for room in [b"kept-room".as_slice(), GLOBAL_CHANNEL] {
        let relay = Relay::new(config()).unwrap();
        let mut source = relay.subscribe(room).unwrap();
        let mut target = relay.subscribe(room).unwrap();
        source.outgoing.try_recv().unwrap();
        target.outgoing.try_recv().unwrap();
        let source_id = source.registration.peer.id.load(Relaxed);
        let target_id = target.registration.peer.id.load(Relaxed);
        let mut argument = Encoder::new(Vec::new());
        argument
            .array(2)
            .unwrap()
            .bytes(b"kept")
            .unwrap()
            .bool(true)
            .unwrap();
        relay
            .receive(
                &source.registration.peer,
                protocol::request(4, &argument.into_writer()),
            )
            .unwrap();
        source.outgoing.try_recv().unwrap();
        let variable_pointer = relay
            .channel(room)
            .unwrap()
            .variable(b"kept")
            .unwrap()
            .as_ptr();
        let fillers: Vec<_> = (0..1024)
            .map(|_| {
                let mut sub = relay.subscribe(room).unwrap();
                sub.outgoing.try_recv().unwrap();
                sub
            })
            .collect();
        let peak_capacity = relay.channel(room).unwrap().members_capacity();
        assert!(peak_capacity >= 1026);
        // Exercise real administrative close signals. Registrations remain
        // until cleanup, while healthy peers keep their output queues.
        for sub in &fillers {
            assert!(relay.disconnect(sub.registration.peer.id.load(Relaxed)));
        }
        let value = protocol::byte_string(&[0x5a; 8192]);
        let input = protocol::envelope(target_id, &value);
        let pointer = protocol::parse(&input).unwrap().1.as_ptr();
        relay.receive(&source.registration.peer, input).unwrap();
        let held = target.outgoing.try_recv().unwrap();
        assert_eq!(held.frame.payload().as_ptr(), pointer);
        for sub in fillers {
            let id = sub.registration.peer.id.load(Relaxed);
            drop(sub);
            let expected = protocol::envelope(BROADCAST, &protocol::uint(id));
            for survivor in [&mut source, &mut target] {
                let departure = survivor.outgoing.try_recv().unwrap();
                assert_eq!(departure.frame.to_vec(), expected.as_ref());
            }
        }
        {
            let channel = relay.channel(room).unwrap();
            assert_eq!(channel.members_len(), 2);
            assert!(channel.members_capacity() <= 256);
            assert!(channel.members_capacity() < peak_capacity / 4);
            assert!(Arc::ptr_eq(
                &channel.member(source.registration.peer.key).unwrap(),
                &source.registration.peer
            ));
            assert!(Arc::ptr_eq(
                &channel.member(target.registration.peer.key).unwrap(),
                &target.registration.peer
            ));
            assert_eq!(
                channel.variable(b"kept").unwrap().as_ptr(),
                variable_pointer
            );
            assert_eq!(channel.variable(b"kept").unwrap().as_ref(), &[0xf5]);
        }
        assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 5);
        relay
            .receive(
                &source.registration.peer,
                protocol::envelope(target_id, &[0x18, 42]),
            )
            .unwrap();
        let after = target.outgoing.try_recv().unwrap();
        assert_eq!(
            [after.frame.prefix(), after.frame.payload()].concat(),
            protocol::envelope(source_id, &[0x18, 42])
        );
        drop(after);
        drop(source);
        drop(target);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(
            relay.metrics.variable_bytes.load(Relaxed),
            if room == GLOBAL_CHANNEL { 5 } else { 0 }
        );
        assert_eq!(held.frame.payload().as_ptr(), pointer);
        assert_eq!(held.frame.payload(), value.as_ref());
        assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
        drop(held);
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }
}

#[test]
fn sparse_channel_registry_preserves_names_variables_and_published_storage() {
    let relay = Relay::new(config()).unwrap();
    let mut source = relay.subscribe(b"\xff\0").unwrap();
    let mut target = relay.subscribe(b"kept").unwrap();
    let mut global = relay.subscribe(GLOBAL_CHANNEL).unwrap();
    for sub in [&mut source, &mut target, &mut global] {
        sub.outgoing.try_recv().unwrap();
        relay
            .receive(
                &sub.registration.peer,
                protocol::request(4, b"\x82\x44kept\x18\x2a"),
            )
            .unwrap();
        sub.outgoing.try_recv().unwrap();
    }
    relay
        .receive(
            &source.registration.peer,
            protocol::request(0, &protocol::text("test-password")),
        )
        .unwrap();
    source.outgoing.try_recv().unwrap();
    let expected = b"\x82\x00\x82\x02\x83\x82\x44kept\x01\x82\x48re_globl\x01\x82\x42\xff\0\x01";
    let names = [b"\xff\0".as_slice(), b"kept", GLOBAL_CHANNEL];
    let pointers: Vec<_> = names
        .iter()
        .map(|name| {
            let channel = relay.channel(name).unwrap();
            (
                channel.name.as_ptr(),
                channel.variable(b"kept").unwrap().as_ptr(),
            )
        })
        .collect();
    let fillers: Vec<_> = (0..1024)
        .map(|i| {
            let mut sub = relay.subscribe(format!("room-{i}").as_bytes()).unwrap();
            sub.outgoing.try_recv().unwrap();
            sub
        })
        .collect();
    let peak_capacity = relay.channels_capacity();
    assert!(peak_capacity >= 1027);
    let value = protocol::byte_string(&[0x5a; 8192]);
    let input = protocol::envelope(target.registration.peer.id.load(Relaxed), &value);
    let pointer = protocol::parse(&input).unwrap().1.as_ptr();
    relay.receive(&source.registration.peer, input).unwrap();
    let held = target.outgoing.try_recv().unwrap();
    drop(fillers);
    {
        assert_eq!(relay.counts().1, 3);
        assert!(relay.channels_capacity() <= 256);
        assert!(relay.channels_capacity() < peak_capacity / 4);
        for ((name, (key_pointer, value_pointer)), sub) in
            names.iter().zip(&pointers).zip([&source, &target, &global])
        {
            let channel = relay.channel(name).unwrap();
            assert_eq!(channel.name.as_ptr(), *key_pointer);
            assert_eq!(channel.variable(b"kept").unwrap().as_ptr(), *value_pointer);
            assert!(Arc::ptr_eq(
                &channel.member(sub.registration.peer.key).unwrap(),
                &sub.registration.peer
            ));
        }
    }
    // The two inspection interfaces intentionally sort by different keys:
    // raw channel bytes on CBOR, base64 text on the management listener.
    relay
        .receive(&source.registration.peer, protocol::request(2, &[0xf6]))
        .unwrap();
    assert_eq!(source.outgoing.try_recv().unwrap().frame.to_vec(), expected);
    assert_eq!(
        serde_json::to_value(relay.channels()).unwrap(),
        serde_json::json!([
            {"name":null,"name_base64":"/wA=","members":1,"variables":1,"variable_bytes":6},
            {"name":"kept","name_base64":"a2VwdA==","members":1,"variables":1,"variable_bytes":6},
            {"name":"re_globl","name_base64":"cmVfZ2xvYmw=","members":1,"variables":1,"variable_bytes":6}
        ])
    );
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 18);
    drop(global);
    drop(source);
    drop(target);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 6);
    let mut rejoined = relay.subscribe(b"kept").unwrap();
    rejoined.outgoing.try_recv().unwrap();
    relay
        .receive(
            &rejoined.registration.peer,
            protocol::request(5, b"\x44kept"),
        )
        .unwrap();
    assert_eq!(
        rejoined.outgoing.try_recv().unwrap().frame.to_vec(),
        b"\x82\x00\x82\x18\xc8\x82\xf4\xf6"
    );
    drop(rejoined);
    let mut rejoined = relay.subscribe(GLOBAL_CHANNEL).unwrap();
    rejoined.outgoing.try_recv().unwrap();
    relay
        .receive(
            &rejoined.registration.peer,
            protocol::request(5, b"\x44kept"),
        )
        .unwrap();
    assert_eq!(
        rejoined.outgoing.try_recv().unwrap().frame.to_vec(),
        b"\x82\x00\x82\x18\xc8\x82\xf5\x18\x2a"
    );
    drop(rejoined);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(held.frame.payload().as_ptr(), pointer);
    assert_eq!(held.frame.payload(), value.as_ref());
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
    drop(held);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn channel_snapshots_own_metadata_and_leave_the_registry_writable() {
    let relay = Relay::new(config()).unwrap();
    let name = b"\xff\0".as_slice();
    let mut source = relay.subscribe(name).unwrap();
    let mut target = relay.subscribe(name).unwrap();
    source.outgoing.try_recv().unwrap();
    target.outgoing.try_recv().unwrap();
    relay
        .receive(
            &source.registration.peer,
            protocol::request(4, b"\x82\x45ready\xf5"),
        )
        .unwrap();
    source.outgoing.try_recv().unwrap();
    let name_pointer = relay.channel(name).unwrap().name.as_ptr();
    let peer = Arc::downgrade(&source.registration.peer);
    let value = protocol::byte_string(&[0x5a; 8192]);
    let input = protocol::envelope(target.registration.peer.id.load(Relaxed), &value);
    let body_pointer = protocol::parse(&input).unwrap().1.as_ptr();
    relay.receive(&source.registration.peer, input).unwrap();
    let held = target.outgoing.try_recv().unwrap();

    let snapshot = relay.channel_snapshot();
    assert!(relay.registry_unlocked(), "snapshot retained the guard");
    let captured = snapshot.iter().find(|c| c.name.as_bytes() == name).unwrap();
    assert_ne!(captured.name.as_bytes().as_ptr(), name_pointer);
    let members = relay.channel_members_snapshot();
    assert!(relay.registry_unlocked());
    assert_eq!(
        members.iter().find(|c| c.0.as_bytes() == name).unwrap().1,
        2
    );
    assert_eq!(
        (
            captured.members,
            captured.variables,
            captured.variable_bytes
        ),
        (2, 1, 6)
    );
    let mut joined = relay.subscribe(name).unwrap();
    joined.outgoing.try_recv().unwrap();
    relay
        .receive(
            &joined.registration.peer,
            protocol::request(4, b"\x82\x45ready\x18\x2a"),
        )
        .unwrap();
    joined.outgoing.try_recv().unwrap();
    assert_eq!(
        relay
            .channels()
            .iter()
            .find(|c| c.name.is_none())
            .unwrap()
            .members,
        3
    );
    assert_eq!(
        (
            captured.members,
            captured.variables,
            captured.variable_bytes
        ),
        (2, 1, 6)
    );
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 7);
    drop(source);
    drop(target);
    drop(joined);
    assert!(peer.upgrade().is_none(), "snapshot retained a peer");
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
    assert_eq!(captured.name.as_bytes(), name);
    assert_eq!(
        members.iter().find(|c| c.0.as_bytes() == name).unwrap().1,
        2
    );
    assert_eq!(captured.members, 2);
    assert_eq!(held.frame.payload().as_ptr(), body_pointer);
    assert_eq!(held.frame.payload(), value.as_ref());
    assert_eq!(relay.metrics.routing_copied_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), held.frame.len());
    drop(held);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn global_queue_pressure_drops_output_and_closes_only_heavy_recipients() {
    let relay = Relay::new(Config {
        max_message_bytes: 32,
        queue_bytes: 64,
        total_queue_bytes: 64,
        ..config()
    })
    .unwrap();
    let mut sender = relay.subscribe(b"a").unwrap();
    let mut busy = relay.subscribe(b"b").unwrap();
    let mut empty = relay.subscribe(b"c").unwrap();
    sender.outgoing.try_recv().unwrap();
    busy.outgoing.try_recv().unwrap();
    empty.outgoing.try_recv().unwrap();
    let payload = protocol::byte_string(&[0; 20]);
    let busy_id = busy.registration.peer.id.load(Relaxed);
    let empty_id = empty.registration.peer.id.load(Relaxed);
    for _ in 0..2 {
        relay
            .receive(
                &sender.registration.peer,
                protocol::envelope(busy_id, &payload),
            )
            .unwrap();
    }
    // Global credit is exhausted by the unread output of `busy`. A message to
    // an idle recipient is dropped: the sender stays open and so does the
    // idle recipient, which holds nothing.
    relay
        .receive(
            &sender.registration.peer,
            protocol::envelope(empty_id, &payload),
        )
        .unwrap();
    assert_eq!(relay.metrics.global_pressure.load(Relaxed), 1);
    assert!(empty.outgoing.try_recv().is_err());
    assert!(!empty.registration.peer.closing.load(Relaxed));
    assert!(!sender.registration.peer.closing.load(Relaxed));
    assert!(!busy.registration.peer.closing.load(Relaxed));
    // The recipient that is holding the credit is the slow consumer: another
    // frame for it under global pressure closes it and frees its allowance.
    relay
        .receive(
            &sender.registration.peer,
            protocol::envelope(busy_id, &payload),
        )
        .unwrap();
    assert_eq!(*busy.close.borrow(), Some(Disconnect::OVERLOADED));
    assert_eq!(relay.metrics.slow_consumers.load(Relaxed), 1);
    assert!(!sender.registration.peer.closing.load(Relaxed));
    assert!(relay.metrics.queued_bytes.load(Relaxed) <= 64);
    drop(sender);
    drop(busy);
    drop(empty);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn global_variable_budget_is_released_by_channel_cleanup() {
    let relay = Relay::new(Config {
        max_variable_bytes: 16,
        total_variable_bytes: 16,
        ..config()
    })
    .unwrap();
    let first = relay.subscribe(b"first").unwrap();
    let second = relay.subscribe(b"second").unwrap();
    let mut e = Encoder::new(Vec::new());
    e.array(2)
        .unwrap()
        .str("x")
        .unwrap()
        .bytes(&[0; 8])
        .unwrap();
    let request = protocol::request(4, &e.into_writer());
    relay
        .receive(&first.registration.peer, request.clone())
        .unwrap();
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 10);
    assert_eq!(
        relay.receive(&second.registration.peer, request.clone()),
        Err(Disconnect::OVERLOADED)
    );
    drop(first);
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
    relay.receive(&second.registration.peer, request).unwrap();
    drop(second);
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
}

#[test]
fn synchronous_submission_borrows_owners_and_releases_charged_bytes() {
    struct BorrowProbe;
    impl PeerOutput for BorrowProbe {
        fn send(&self, packet: Submission<'_>) -> bool {
            let budget = packet.budget.expect("charged submission");
            assert_eq!(
                Arc::strong_count(budget),
                1,
                "synchronous budget was cloned"
            );
            assert_eq!(budget.used.load(Relaxed), packet.frame.len());
            assert_eq!(
                budget.metrics.queued_bytes.load(Relaxed),
                packet.frame.len()
            );
            if !packet.frame.payload().is_empty() {
                assert!(
                    packet.frame.has_unique_payload(),
                    "synchronous body was cloned"
                );
            }
            true
        }
        fn close(&self, _: Disconnect) {}
    }
    let relay = Relay::new(config()).unwrap();
    let registration = relay
        .subscribe_with_output(b"borrow", Arc::new(BorrowProbe))
        .unwrap();
    let frame = Frame::envelope(1, Bytes::from(vec![0; 64]));
    registration.peer.enqueue_frame(&frame).unwrap();
    assert!(frame.has_unique_payload());
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}

#[test]
fn deferred_submission_promotes_same_body_and_holds_reservation_through_retirement() {
    let relay = Relay::new(config()).unwrap();
    let mut subscription = relay.subscribe(b"defer").unwrap();
    drop(subscription.outgoing.try_recv().unwrap());
    let frame = Frame::envelope(1, Bytes::from(vec![0; 64]));
    let pointer = frame.payload().as_ptr();
    subscription
        .registration
        .peer
        .enqueue_frame(&frame)
        .unwrap();
    let held = subscription.outgoing.try_recv().unwrap();
    assert_eq!(held.frame.payload().as_ptr(), pointer);
    assert_eq!(Arc::strong_count(&held.budget), 2);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), frame.len());
    drop(subscription);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), frame.len());
    drop(held);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    assert!(frame.has_unique_payload());
}
