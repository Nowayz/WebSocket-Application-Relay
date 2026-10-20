use super::*;

#[test]
fn bounded_channel_pages_match_the_independent_full_listing_and_own_metadata() {
    let relay = Relay::new(Config::default()).unwrap();
    let mut names = vec![
        Vec::new(),
        b"abc".to_vec(),
        b"abc\0".to_vec(),
        "π世界".as_bytes().to_vec(),
    ];
    names.extend((0..=255).map(|byte| vec![byte]));
    names.extend((2..=16).map(|len| vec![0x5a; len]));
    let mut subscriptions: Vec<_> = names
        .iter()
        .map(|name| {
            let mut sub = relay.subscribe(name).unwrap();
            sub.outgoing.try_recv().unwrap();
            sub
        })
        .collect();
    relay
        .receive(
            &subscriptions[0].registration.peer,
            protocol::request(4, b"\x82\x43key\xf5"),
        )
        .unwrap();
    subscriptions[0].outgoing.try_recv().unwrap();
    let expected = serde_json::to_value(relay.channels()).unwrap();
    for limit in [1, 2, 7, MAX_CHANNEL_PAGE_SIZE] {
        let mut after = None;
        let mut collected = Vec::new();
        loop {
            let page = relay.channel_page(std::num::NonZeroUsize::new(limit).unwrap(), after);
            assert!(page.len() <= limit);
            let rows = serde_json::to_value(&page).unwrap();
            collected.extend(rows.as_array().unwrap().iter().cloned());
            if page.len() < limit {
                break;
            }
            after = Some(page.last().unwrap().name.base64());
        }
        assert_eq!(collected, *expected.as_array().unwrap());
    }
    let page = relay.channel_page(std::num::NonZeroUsize::new(7).unwrap(), None);
    let detached = serde_json::to_vec(&page).unwrap();
    drop(subscriptions);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    drop(relay);
    assert_eq!(serde_json::to_vec(&page).unwrap(), detached);
}

#[test]
fn detached_http_listing_preserves_json_and_base64_order_for_bounded_names() {
    let relay = Relay::new(Config::default()).unwrap();
    let mut names = vec![
        Vec::new(),
        b"abc".to_vec(),
        b"abc\0".to_vec(),
        "π世界".as_bytes().to_vec(),
    ];
    names.extend((0..=255).map(|byte| vec![byte]));
    names.extend((2..=16).map(|len| vec![0x5a; len]));
    let mut subscriptions: Vec<_> = names
        .iter()
        .map(|name| {
            let mut sub = relay.subscribe(name).unwrap();
            sub.outgoing.try_recv().unwrap();
            sub
        })
        .collect();
    relay
        .receive(
            &subscriptions[0].registration.peer,
            protocol::request(4, b"\x82\x43key\xf5"),
        )
        .unwrap();
    subscriptions[0].outgoing.try_recv().unwrap();
    let expected = serde_json::to_vec(&relay.channels()).unwrap();
    let snapshot = relay.channel_listing();
    // Snapshot serialization must not consult membership, values or name views
    // after the original channels and the entire registry have retired.
    drop(subscriptions);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    drop(relay);
    assert_eq!(serde_json::to_vec(&snapshot).unwrap(), expected);
}

#[test]
fn joined_channel_names_share_canonical_storage_and_own_their_input() {
    for name in [
        GLOBAL_CHANNEL,
        b"",
        b"\0",
        b"a\0",
        b"\xff\0",
        b"1234567890abcdef",
        "π世界".as_bytes(),
    ] {
        let relay = Relay::new(Config::default()).unwrap();
        let mut input = name.to_vec();
        let mut first = relay.subscribe(&input).unwrap();
        first.outgoing.try_recv().unwrap();
        input.fill(0xee);
        assert_eq!(first.registration.peer.channel.as_ref(), name);
        let mut second = relay.subscribe(name).unwrap();
        second.outgoing.try_recv().unwrap();
        {
            let channel = relay.channel(name).unwrap();
            assert_eq!(channel.members_len(), 2);
            for member in [&first, &second] {
                assert_eq!(
                    member.registration.peer.channel.as_ptr(),
                    channel.name.as_ptr(),
                    "channel peers must share canonical name storage"
                );
            }
        }
        let retained_name = first.registration.peer.channel.clone();
        let id = first.registration.peer.id.load(Relaxed);
        drop(first);
        assert_eq!(
            second.outgoing.try_recv().unwrap().frame.to_vec(),
            protocol::envelope(BROADCAST, &protocol::uint(id)).as_ref()
        );
        assert_eq!(second.registration.peer.channel.as_ref(), name);
        drop(second);
        assert_eq!(retained_name.as_ref(), name);
        assert_eq!(relay.counts(), (0, 1));
        assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
    }
}

#[test]
fn recreated_channels_have_fresh_variables_while_old_name_views_remain_valid() {
    let relay = Relay::new(Config::default()).unwrap();
    let name = b"\xff\0recreate";
    let mut first = relay.subscribe(name).unwrap();
    first.outgoing.try_recv().unwrap();
    let retained_name = first.registration.peer.channel.clone();
    let mut argument = Encoder::new(Vec::new());
    argument
        .array(2)
        .unwrap()
        .bytes(b"key")
        .unwrap()
        .bool(true)
        .unwrap();
    relay
        .receive(
            &first.registration.peer,
            protocol::request(4, &argument.into_writer()),
        )
        .unwrap();
    first.outgoing.try_recv().unwrap();
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 4);
    drop(first);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
    let mut next = relay.subscribe(name).unwrap();
    next.outgoing.try_recv().unwrap();
    relay
        .receive(
            &next.registration.peer,
            protocol::request(5, &protocol::byte_string(b"key")),
        )
        .unwrap();
    assert_eq!(
        next.outgoing.try_recv().unwrap().frame.to_vec(),
        protocol::reply(200, &[0x82, 0xf4, 0xf6]).as_ref()
    );
    assert_eq!(retained_name.as_ref(), name);
    assert_eq!(next.registration.peer.channel.as_ref(), name);
    drop(next);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}
