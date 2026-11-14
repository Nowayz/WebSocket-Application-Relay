use super::*;
use crate::protocol;

#[test]
fn scalar_control_frames_match_known_vectors_and_every_integer_width() {
    for value in [
        0,
        23,
        24,
        255,
        256,
        65535,
        65536,
        u32::MAX as u64,
        u32::MAX as u64 + 1,
        u64::MAX,
    ] {
        let welcome = Frame::integer(value);
        assert_eq!(welcome.to_vec(), protocol::uint(value));
        assert!(welcome.payload().is_empty());
        for op in [0, 1, 3, 23, 24, 200, 255] {
            let frame = Frame::reply_uint(op, value);
            assert_eq!(frame.to_vec(), protocol::reply(op, &protocol::uint(value)));
            assert_eq!(frame.len(), frame.to_vec().len());
            assert!(frame.prefix().len() <= 14);
            assert!(frame.payload().is_empty());
            for flag in [false, true] {
                assert_eq!(
                    Frame::reply_bool(op, flag).to_vec(),
                    protocol::reply(op, &[if flag { 0xf5 } else { 0xf4 }])
                );
            }
        }
    }
    assert_eq!(
        Frame::integer(u64::MAX).to_vec(),
        b"\x1b\xff\xff\xff\xff\xff\xff\xff\xff"
    );
    assert_eq!(
        Frame::reply_uint(200, u32::MAX as u64).to_vec(),
        b"\x82\x00\x82\x18\xc8\x1a\xff\xff\xff\xff"
    );
    assert_eq!(Frame::reply_bool(4, true).to_vec(), b"\x82\x00\x82\x04\xf5");
}

#[test]
fn variable_replies_preserve_opaque_values_and_missing_is_distinct_from_null() {
    assert_eq!(
        Frame::variable(None).to_vec(),
        b"\x82\x00\x82\x18\xc8\x82\xf4\xf6"
    );
    for value in [
        &b"\xf6"[..],
        &b"\x40"[..],
        &b"\x60"[..],
        &b"\x18\x01"[..],
        &b"\xd8\x20\xa1\x61k\x43\x00\xff\x01"[..],
    ] {
        let bytes = Bytes::copy_from_slice(value);
        let pointer = bytes.as_ptr();
        let frame = Frame::variable(Some(bytes));
        assert_eq!(frame.payload().as_ptr(), pointer);
        assert_eq!(frame.prefix(), b"\x82\x00\x82\x18\xc8\x82\xf5");
        assert_eq!(frame.payload(), value);
        let mut expected = b"\x82\x00\x82\x18\xc8\x82\xf5".to_vec();
        expected.extend_from_slice(value);
        assert_eq!(frame.to_vec(), expected);
    }
}

#[test]
fn routed_owned_views_preserve_aliases_and_survive_original_owner_retirement() {
    let value = protocol::byte_string(&[0xa5; 64]);
    let packet = protocol::envelope(65536, &value);
    let offset = packet.len() - value.len();
    for kind in 0..4 {
        for retain in [false, true] {
            for sender in [1, u64::MAX] {
                let input = match kind {
                    0 => Bytes::copy_from_slice(&packet),
                    1 => packet.to_vec().into(),
                    2 => {
                        let mut storage = Vec::with_capacity(packet.len() + 128);
                        storage.extend_from_slice(&packet);
                        storage.into()
                    }
                    _ => {
                        let mut storage = bytes::BytesMut::with_capacity(256);
                        storage.extend_from_slice(b"padding");
                        storage.extend_from_slice(&packet);
                        storage.extend_from_slice(b"tail");
                        storage.freeze().slice(7..7 + packet.len())
                    }
                };
                let backing = retain.then(|| input.clone());
                let unique = input.is_unique();
                let pointer = input[offset..].as_ptr();
                let frame = Frame::routed(sender, input, offset);
                assert_eq!(frame.payload().as_ptr(), pointer);
                assert_eq!(frame.payload.is_unique(), unique);
                if let Some(backing) = &backing {
                    assert_eq!(backing.as_ref(), packet.as_ref());
                }
                let retained_frame = frame.clone();
                drop(frame);
                drop(backing);
                assert_eq!(retained_frame.payload().as_ptr(), pointer);
                assert_eq!(retained_frame.payload(), value.as_ref());
                assert!(retained_frame.payload.is_unique());
                let mut wire = retained_frame.prefix().to_vec();
                wire.extend_from_slice(retained_frame.payload());
                assert_eq!(protocol::parse(&wire).unwrap(), (sender, value.as_ref()));
            }
        }
    }
}

#[test]
fn routing_reuses_the_body_for_every_integer_width_and_nonminimal_input() {
    // Nonminimal target/array encodings and an opaque tagged value must be
    // accepted without shifting the body when the sender's width changes.
    let input = Bytes::from_static(&[0x98, 2, 0x1b, 0, 0, 0, 0, 0, 0, 0, 42, 0xc1, 0x18, 1]);
    let (_, value) = protocol::parse(&input).unwrap();
    let offset = input.len() - value.len();
    let pointer = value.as_ptr();
    for sender in [
        1,
        23,
        24,
        255,
        256,
        65535,
        65536,
        u32::MAX as u64 + 1,
        u64::MAX,
    ] {
        let frame = Frame::routed(sender, input.clone(), offset);
        assert_eq!(frame.payload().as_ptr(), pointer);
        let mut wire = frame.prefix().to_vec();
        wire.extend_from_slice(frame.payload());
        assert_eq!(protocol::parse(&wire).unwrap(), (sender, value));
    }
}
