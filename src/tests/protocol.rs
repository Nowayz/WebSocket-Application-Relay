use super::*;

#[test]
fn integer_and_array_header_sizes_match_the_encoder_at_every_width_boundary() {
    for value in [
        0,
        1,
        23,
        24,
        255,
        256,
        65_535,
        65_536,
        u32::MAX as u64,
        u32::MAX as u64 + 1,
        u64::MAX,
    ] {
        assert_eq!(uint_len(value), uint(value).len());
        let mut e = Encoder::new(Vec::new());
        e.array(value).unwrap();
        assert_eq!(uint_len(value), e.into_writer().len());
    }
}

#[test]
fn independent_known_cbor_vectors_and_full_width_ids() {
    // RFC 8949 examples: [1, 2, 3], a byte string, text, and a map.
    for value in [
        &[0x83, 1, 2, 3][..],
        &[0x43, 1, 2, 3],
        &[0x62, b'h', b'i'],
        &[0xa1, 0x61, b'x', 0xf5],
        &[0xf9, 0x3e, 0x00],
    ] {
        let packet = envelope(u64::MAX, value);
        let (id, received) = parse(&packet).unwrap();
        assert_eq!(id, u64::MAX);
        assert_eq!(value, received);
    }
    assert_eq!(envelope(1, &[0xf6]).as_ref(), &[0x82, 1, 0xf6]);
}

#[test]
fn simple_values_reject_reserved_expansions_and_preserve_valid_forms() {
    // RFC 8949 section 3.3: expanded simple values must be >=32.
    // Integer nonminimal encodings remain legal; simple values differ.
    for value in 0..=255u8 {
        for prefix in [&[][..], &[0x81], &[0xa1, 0], &[0xc0], &[0xa1]] {
            let mut encoded = prefix.to_vec();
            encoded.extend_from_slice(&[0xf8, value]);
            if prefix == [0xa1] {
                encoded.push(0xf6); // Exercise a malformed/valid map key too.
            }
            let packet = envelope(1, &encoded);
            if value < 32 {
                assert_eq!(parse(&packet), Err(Disconnect::PROTOCOL), "{encoded:02x?}");
            } else {
                assert_eq!(parse(&packet).unwrap().1, encoded);
            }
        }
    }
    for value in 0xe0..=0xf7 {
        assert_eq!(parse(&envelope(1, &[value])).unwrap().1, [value]);
    }
    assert_eq!(parse(&envelope(1, &[0x18, 0])).unwrap().1, [0x18, 0]);
}

#[test]
fn cbor_header_truncation_utf8_and_resource_boundaries() {
    // Independent wire values, including nonminimal definite headers,
    // extreme integers, duplicate keys, opaque tags, NaN, and signed zero.
    let valid: &[&[u8]] = &[
        &[0],
        &[0x1b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        &[0x3b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        &[0x58, 1, 0x80],
        &[0x78, 2, b'p', b'i'],
        &[0x98, 1, 0xf6],
        &[0xa2, 0, 1, 0, 2],
        &[0xd8, 0x18, 0x43, 0, 1, 2],
        &[0xf9, 0x7e, 1],
        &[0xf9, 0x80, 0],
        &[0xfa, 0x7f, 0x80, 0, 0],
        &[0xfb, 0x7f, 0xf8, 0, 0, 0, 0, 0, 1],
        &[0xf8, 0x20],
        &[0xf8, 0xff],
    ];
    for value in valid {
        assert_eq!(parse(&envelope(1, value)).unwrap().1, *value);
        for end in 0..value.len() {
            assert_eq!(
                parse(&envelope(1, &value[..end])),
                Err(Disconnect::PROTOCOL),
                "truncation of {value:02x?} at {end}"
            );
        }
        let mut trailing = value.to_vec();
        trailing.push(0);
        assert_eq!(parse(&envelope(1, &trailing)), Err(Disconnect::PROTOCOL));
    }
    // RFC 8949 Appendix F: reserved additional information in every major
    // type, plus indefinite/break forms excluded by the WAR wire contract.
    for major in 0..8u8 {
        for additional in 28..32 {
            assert_eq!(
                parse(&envelope(1, &[major * 32 + additional])),
                Err(Disconnect::PROTOCOL)
            );
        }
    }
    for text in [
        &[0x80][..],
        &[0xc0, 0xaf],
        &[0xed, 0xa0, 0x80],
        &[0xf4, 0x90, 0x80, 0x80],
        &[0xe2, 0x82],
    ] {
        let mut encoded = vec![0x60 + text.len() as u8];
        encoded.extend_from_slice(text);
        assert_eq!(parse(&envelope(1, &encoded)), Err(Disconnect::PROTOCOL));
    }
    for marker in [0x81, 0xc0] {
        let mut nested = vec![marker; MAX_DEPTH];
        nested.push(0);
        assert!(parse(&envelope(1, &nested)).is_ok());
        nested.insert(0, marker);
        assert_eq!(parse(&envelope(1, &nested)), Err(Disconnect::PROTOCOL));
    }
    for count in [MAX_ITEMS - 1, MAX_ITEMS] {
        let mut array = vec![0x9a];
        array.extend_from_slice(&(count as u32).to_be_bytes());
        array.resize(array.len() + count as usize, 0xf6);
        assert_eq!(parse(&envelope(1, &array)).is_ok(), count < MAX_ITEMS);
    }
}

#[test]
fn byte_strings_keep_exact_storage_and_shared_bytes_at_length_boundaries() {
    let cases: &[(usize, &[u8])] = &[
        (0, b"\x40"),
        (1, b"\x41"),
        (23, b"\x57"),
        (24, b"\x58\x18"),
        (255, b"\x58\xff"),
        (256, b"\x59\x01\x00"),
        (1024, b"\x59\x04\x00"),
        (65535, b"\x59\xff\xff"),
        (65536, b"\x5a\x00\x01\x00\x00"),
    ];
    for &(length, header) in cases {
        let mut input = vec![0x5a; length];
        let encoded = byte_string(&input);
        input.fill(0x11);
        assert_eq!(&encoded[..header.len()], header);
        assert_eq!(encoded.len(), header.len() + length);
        assert!(encoded[header.len()..].iter().all(|&byte| byte == 0x5a));
        let pointer = encoded.as_ptr();
        let shared = encoded.clone();
        drop(encoded);
        assert_eq!(shared.as_ptr(), pointer, "cloning copied the encoded body");
        let owned = shared
            .try_into_mut()
            .expect("last owner can recover storage");
        assert_eq!(owned.as_ptr(), pointer, "recovering ownership moved bytes");
        assert_eq!(
            owned.capacity(),
            owned.len(),
            "spare capacity forces eager ownership metadata"
        );
    }
}

#[test]
fn text_envelopes_and_replies_keep_exact_owned_storage() {
    fn exact_storage(encoded: Bytes) {
        let pointer = encoded.as_ptr();
        let shared = encoded.clone();
        drop(encoded);
        assert_eq!(shared.as_ptr(), pointer, "clone copied the encoded body");
        let owned = shared.try_into_mut().expect("last owner recovers storage");
        assert_eq!(owned.as_ptr(), pointer, "ownership recovery moved bytes");
        assert_eq!(owned.capacity(), owned.len(), "spare encoded capacity");
    }
    for length in [0, 1, 23, 24, 255, 256, 65535, 65536] {
        let value = "x".repeat(length);
        let encoded = text(&value);
        let mut decoder = Decoder::new(&encoded);
        assert_eq!(decoder.str().unwrap(), value);
        assert_eq!(decoder.position(), encoded.len());
        exact_storage(encoded);
    }
    for value in ["é", "🦀", "日本語", "a\0b"] {
        let encoded = text(value);
        assert_eq!(Decoder::new(&encoded).str().unwrap(), value);
        exact_storage(encoded);
    }
    let value = byte_string(b"payload");
    for sender in [
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
        let encoded = envelope(sender, &value);
        assert_eq!(parse(&encoded).unwrap(), (sender, value.as_ref()));
        exact_storage(encoded);
    }
    for op in [0, 23, 24, 255] {
        let encoded = reply(op, &value);
        let mut decoder = Decoder::new(&encoded);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u64().unwrap(), 0);
        assert_eq!(decoder.array().unwrap(), Some(2));
        assert_eq!(decoder.u8().unwrap(), op);
        assert_eq!(decoder.bytes().unwrap(), b"payload");
        assert_eq!(decoder.position(), encoded.len());
        exact_storage(encoded);
    }
}

#[test]
fn text_validation_matches_minicbor_across_headers_and_utf8_boundaries() {
    fn same(input: &[u8]) {
        let mut original = Decoder::new(input);
        let expected = original.str().is_ok();
        let mut candidate = Decoder::new(input);
        assert_eq!(
            validate_value(&mut candidate).is_ok(),
            expected,
            "text header {:x?}",
            &input[..input.len().min(16)]
        );
        if expected {
            assert_eq!(candidate.position(), original.position());
        }
    }
    // Noncanonical length widths remain accepted as before, including empty
    // strings. Header truncation and impossible lengths remain rejected.
    for length in [0usize, 1, 23, 24, 63, 64, 65, 255, 256, 65535, 65536] {
        for (info, width) in [(24, 1), (25, 2), (26, 4), (27, 8)] {
            if width < 8 && length as u64 >= (1u64 << (width * 8)) {
                continue;
            }
            let mut encoded = vec![0x60 | info];
            let argument = (length as u64).to_be_bytes();
            encoded.extend_from_slice(&argument[8 - width..]);
            for cut in 0..encoded.len() {
                same(&encoded[..cut]);
            }
            same(&encoded);
            encoded.resize(encoded.len() + length, b'a');
            encoded.push(0xf6); // next item must remain outside this string
            same(&encoded);
        }
    }
    for info in 28..=31 {
        same(&[0x60 | info]);
    }
    same(&[0x7b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]);
    for pair in 0u32..=65535 {
        same(&[0x78, 2, (pair >> 8) as u8, pair as u8]);
    }
    let sequences: &[&[u8]] = &[
        b"a",
        b"\xc2\xa2",
        b"\xe2\x82\xac",
        b"\xf0\x9f\xa6\x80",
        b"\xf4\x8f\xbf\xbf",
        b"\x80",
        b"\xc0\x80",
        b"\xc2",
        b"\xe0\x80\x80",
        b"\xed\xa0\x80",
        b"\xe2\x82",
        b"\xf0\x80\x80\x80",
        b"\xf4\x90\x80\x80",
        b"\xf5\x80\x80\x80",
    ];
    for sequence in sequences {
        for offset in 0..=129 {
            let mut encoded = vec![0x79, 1, 4]; // 260 content bytes
            encoded.resize(263, b'a');
            encoded[3 + offset..3 + offset + sequence.len()].copy_from_slice(sequence);
            encoded.push(0xf6);
            same(&encoded);
        }
    }
    let mut split = vec![0x78, 64];
    split.resize(66, b'a');
    split[65] = 0xc2;
    split.push(0xa2); // a continuation outside the declared text is invalid
    same(&split);
}

#[test]
fn validated_long_text_still_borrows_its_original_encoded_value() {
    let value = text(&"é".repeat(512));
    let packet = envelope(1, &value);
    let (sender, borrowed) = parse(&packet).unwrap();
    assert_eq!(sender, 1);
    assert_eq!(borrowed, value.as_ref());
    assert_eq!(
        borrowed.as_ptr(),
        packet[packet.len() - value.len()..].as_ptr()
    );
}

#[test]
fn rejects_trailing_indefinite_deep_and_truncated_cbor() {
    for value in [
        &[0x82, 1, 0xff][..],
        &[0x82, 1, 0xf6, 0xf6],
        &[0x82, 1, 0x9f, 0xff],
        &[0x82, 1, 0x63, b'a'],
        &[0x82, 1, 0x61, 0xff],
    ] {
        assert_eq!(parse(value), Err(Disconnect::PROTOCOL));
    }
    let mut deep = vec![0x81; MAX_DEPTH + 1];
    deep.push(0);
    assert_eq!(parse(&envelope(1, &deep)), Err(Disconnect::PROTOCOL));
    let mut huge = vec![0x9b];
    huge.extend_from_slice(&u64::MAX.to_be_bytes());
    assert_eq!(parse(&envelope(1, &huge)), Err(Disconnect::PROTOCOL));
}
