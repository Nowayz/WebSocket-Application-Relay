use super::*;

#[test]
fn templates_preserve_default_wire_bytes_and_equal_encoded_sizes() {
    for kind in [ValueKind::Bytes, ValueKind::Ascii, ValueKind::Utf8] {
        let mut payload = kind.payload();
        for sequence in 0u64..64 {
            payload[..8].copy_from_slice(&sequence.to_le_bytes());
            let encoded = kind.encode(&payload);
            assert_eq!(encoded.len(), 1027);
            let mut decoder = Decoder::new(&encoded);
            if kind == ValueKind::Bytes {
                let mut original = [0x5a; 1024];
                original[..8].copy_from_slice(&sequence.to_le_bytes());
                assert_eq!(encoded, protocol::byte_string(&original));
                assert_eq!(decoder.bytes().unwrap(), original);
            } else {
                let text = decoder.str().unwrap();
                assert_eq!(text.as_bytes(), payload);
                assert_eq!(text.len(), 1024);
                assert_eq!(
                    text.chars().count(),
                    if kind == ValueKind::Utf8 { 516 } else { 1024 }
                );
            }
            assert_eq!(decoder.position(), encoded.len());
            let packet = Wire::Cbor.envelope(42, &encoded);
            assert_eq!(protocol::parse(&packet).unwrap(), (42, encoded.as_ref()));
        }
    }
}
