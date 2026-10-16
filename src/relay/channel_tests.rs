use super::*;

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
            let state = relay.state.read();
            let (canonical, channel) = state.channels.get_key_value(name).unwrap();
            assert_eq!(channel.members.len(), 2);
            for member in [&first, &second] {
                assert_eq!(
                    member.registration.peer.channel.as_ptr(),
                    canonical.as_ptr(),
                    "channel peers must share canonical name storage"
                );
            }
        }
        let retained_name = first.registration.peer.channel.clone();
        let id = first.registration.peer.id.load(Relaxed);
        drop(first);
        assert_eq!(
            second.outgoing.try_recv().unwrap().frame.payload(),
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
        next.outgoing.try_recv().unwrap().frame.payload(),
        protocol::reply(200, &[0x82, 0xf4, 0xf6]).as_ref()
    );
    assert_eq!(retained_name.as_ref(), name);
    assert_eq!(next.registration.peer.channel.as_ref(), name);
    drop(next);
    assert_eq!(relay.counts(), (0, 1));
    assert_eq!(relay.metrics.variable_bytes.load(Relaxed), 0);
    assert_eq!(relay.metrics.queued_bytes.load(Relaxed), 0);
}
