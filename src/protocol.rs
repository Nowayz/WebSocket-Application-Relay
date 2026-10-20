//! WAR CBOR v1: join with a CBOR string, then send `[target: uint64, value: any]`.
//!
//! Only definite-length CBOR is accepted. Values are validated with bounded stack
//! storage and forwarded verbatim; routing never materializes application objects.
use bytes::Bytes;
use minicbor::{Decoder, Encoder, data::Type};

pub use crate::common::{
    BROADCAST, CHANNEL_MESSAGE, DISCONNECT_MESSAGE, Disconnect, GLOBAL_CHANNEL, PRIVATE_MESSAGE,
    RELAY, secret_matches,
};
pub const SUBPROTOCOL: &str = "war.cbor.v1";
pub const MAX_DEPTH: usize = 64;
pub const MAX_ITEMS: u64 = 65_536;

fn bad<T>(_: T) -> Disconnect {
    Disconnect::PROTOCOL
}

// Called only for a definite text item established by datatype(). Keep short
// canonical strings on the existing decoder; no scratch is needed for them.
fn validate_text(decoder: &mut Decoder<'_>) -> Result<(), Disconnect> {
    let start = decoder.position();
    let input = decoder.input();
    let info = input[start] & 0x1f;
    if info < 24 {
        decoder.str().map_err(bad)?;
        return Ok(());
    }
    let header_len = match info {
        24 => 2,
        25 => 3,
        26 => 5,
        27 => 9,
        _ => return Err(Disconnect::PROTOCOL),
    };
    let body_start = start.checked_add(header_len).ok_or(Disconnect::PROTOCOL)?;
    // Definite string lengths use CBOR's unsigned argument encoding. Translate
    // only the major type in at most nine header bytes, reusing minicbor's
    // length decoder rather than copying or materializing the string body.
    let mut header = [0; 9];
    header[..header_len].copy_from_slice(input.get(start..body_start).ok_or(Disconnect::PROTOCOL)?);
    header[0] = info;
    let length =
        usize::try_from(Decoder::new(&header[..header_len]).u64().map_err(bad)?).map_err(bad)?;
    let end = body_start.checked_add(length).ok_or(Disconnect::PROTOCOL)?;
    let body = input.get(body_start..end).ok_or(Disconnect::PROTOCOL)?;
    simdutf8::compat::from_utf8(body).map_err(bad)?;
    decoder.set_position(end);
    Ok(())
}

/// Validate the next value without recursion or heap allocations.
///
/// Advances `decoder` past that value; following bytes are left for the caller.
/// [`parse`] additionally rejects bytes after its complete routing envelope.
pub fn validate_value(decoder: &mut Decoder<'_>) -> Result<(), Disconnect> {
    let mut pending = [0u64; MAX_DEPTH + 1];
    pending[0] = 1;
    let mut depth = 0;
    let mut items = 0;
    loop {
        if pending[depth] == 0 {
            if depth == 0 {
                return Ok(());
            }
            depth -= 1;
            continue;
        }
        pending[depth] -= 1;
        items += 1;
        if items > MAX_ITEMS {
            return Err(Disconnect::PROTOCOL);
        }
        let children = match decoder.datatype().map_err(bad)? {
            Type::U8 | Type::U16 | Type::U32 | Type::U64 => {
                decoder.u64().map_err(bad)?;
                0
            }
            Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Int => {
                decoder.int().map_err(bad)?;
                0
            }
            Type::Bytes => {
                decoder.bytes().map_err(bad)?;
                0
            }
            Type::String => {
                validate_text(decoder)?;
                0
            }
            Type::Array => decoder.array().map_err(bad)?.ok_or(Disconnect::PROTOCOL)?,
            Type::Map => decoder
                .map()
                .map_err(bad)?
                .ok_or(Disconnect::PROTOCOL)?
                .checked_mul(2)
                .ok_or(Disconnect::PROTOCOL)?,
            Type::Tag => {
                decoder.tag().map_err(bad)?;
                1
            }
            Type::F16 | Type::F32 | Type::F64 => {
                decoder.f64().map_err(bad)?;
                0
            }
            Type::Bool => {
                decoder.bool().map_err(bad)?;
                0
            }
            Type::Null => {
                decoder.null().map_err(bad)?;
                0
            }
            Type::Undefined => {
                decoder.undefined().map_err(bad)?;
                0
            }
            Type::Simple => {
                // minicbor exposes even reserved expanded values. RFC 8949
                // section 3.3 forbids f8 followed by a value below 32.
                let extended = decoder.input()[decoder.position()] == 0xf8;
                let value = decoder.simple().map_err(bad)?;
                if extended && value < 32 {
                    return Err(Disconnect::PROTOCOL);
                }
                0
            }
            _ => return Err(Disconnect::PROTOCOL),
        };
        if children > 0 {
            if depth == MAX_DEPTH
                || children > MAX_ITEMS - items
                || children > (decoder.input().len() - decoder.position()) as u64
            {
                return Err(Disconnect::PROTOCOL);
            }
            depth += 1;
            pending[depth] = children;
        }
    }
}

/// Validate a complete join message and borrow its channel-name bytes.
///
/// Accepts a CBOR text or byte string of at most 16 bytes, without trailing data.
/// The returned slice shares the input's lifetime; no channel buffer is allocated.
pub fn join(bytes: &[u8]) -> Result<&[u8], Disconnect> {
    let mut d = Decoder::new(bytes);
    let channel = string_bytes(&mut d)?;
    if d.position() != bytes.len() {
        return Err(Disconnect::PROTOCOL);
    }
    if channel.len() > 16 {
        return Err(Disconnect::CHANNEL_LENGTH);
    }
    Ok(channel)
}

/// Validate a complete `[target, value]` envelope and borrow its encoded value.
///
/// The returned value slice includes its original CBOR header and shares the
/// input's lifetime. Validation does not allocate, materialize application
/// objects or reencode their bytes; trailing data is rejected.
pub fn parse(bytes: &[u8]) -> Result<(u64, &[u8]), Disconnect> {
    let mut d = Decoder::new(bytes);
    if d.array().map_err(bad)? != Some(2) {
        return Err(Disconnect::PROTOCOL);
    }
    let target = d.u64().map_err(bad)?;
    let start = d.position();
    validate_value(&mut d)?;
    if d.position() != bytes.len() {
        return Err(Disconnect::PROTOCOL);
    }
    Ok((target, &bytes[start..]))
}

fn string_bytes<'a>(d: &mut Decoder<'a>) -> Result<&'a [u8], Disconnect> {
    match d.datatype().map_err(bad)? {
        Type::String => d.str().map(str::as_bytes).map_err(bad),
        Type::Bytes => d.bytes().map_err(bad),
        _ => Err(Disconnect::PROTOCOL),
    }
}

/// Build a contiguous owned envelope containing `sender` and an encoded value.
///
/// `value` must contain exactly one valid definite-length CBOR value. This helper
/// copies those encoded bytes into the result without validating or transcoding
/// them. Cloning the returned [`Bytes`] shares its buffer. Native relay forwarding
/// instead keeps the routing prefix separate and borrows the shared value body.
pub fn envelope(sender: u64, value: &[u8]) -> Bytes {
    let mut e = Encoder::new(Vec::with_capacity(1 + uint_len(sender) + value.len()));
    e.array(2)
        .and_then(|e| e.u64(sender))
        .expect("Vec encoding");
    let mut bytes = e.into_writer();
    bytes.extend_from_slice(value);
    bytes.into()
}

/// Definite array headers use the same argument width as unsigned integers.
pub(crate) fn uint_len(value: u64) -> usize {
    match value {
        0..=23 => 1,
        24..=255 => 2,
        256..=65_535 => 3,
        65_536..=4_294_967_295 => 5,
        _ => 9,
    }
}

/// Encode an unsigned integer into an owned CBOR value.
pub fn uint(value: u64) -> Bytes {
    let mut e = Encoder::new(Vec::with_capacity(9));
    e.u64(value).expect("Vec encoding");
    e.into_writer().into()
}

/// Copy raw bytes into an owned definite-length CBOR byte string.
///
/// The input is the string's contents, not a preencoded CBOR value. The result
/// owns its storage; later clones share the encoded buffer.
pub fn byte_string(value: &[u8]) -> Bytes {
    // Exact storage lets Bytes defer its shared-ownership allocation until cloning.
    let mut e = Encoder::new(Vec::with_capacity(
        uint_len(value.len() as u64) + value.len(),
    ));
    e.bytes(value).expect("Vec encoding");
    e.into_writer().into()
}

/// Copy UTF-8 text into an owned definite-length CBOR text string.
///
/// The CBOR length counts UTF-8 bytes. The result does not borrow `value`;
/// later clones share the encoded buffer.
pub fn text(value: &str) -> Bytes {
    let mut e = Encoder::new(Vec::with_capacity(
        uint_len(value.len() as u64) + value.len(),
    ));
    e.str(value).expect("Vec encoding");
    e.into_writer().into()
}

/// Build an owned control reply, `[0, [op, value]]`.
///
/// `value` must already contain exactly one valid definite-length CBOR value.
/// Its bytes are copied verbatim, without validation or transcoding, into the
/// contiguous result. Cloning the result shares its buffer.
pub fn reply(op: u8, value: &[u8]) -> Bytes {
    let mut e = Encoder::new(Vec::with_capacity(
        3 + uint_len(u64::from(op)) + value.len(),
    ));
    e.array(2)
        .and_then(|e| e.u64(0))
        .and_then(|e| e.array(2))
        .and_then(|e| e.u8(op))
        .expect("Vec encoding");
    let mut bytes = e.into_writer();
    bytes.extend_from_slice(value);
    bytes.into()
}

/// Build an owned relay command with the same encoding contract as [`reply`].
pub fn request(op: u8, value: &[u8]) -> Bytes {
    reply(op, value)
}

pub enum Command<'a> {
    Authenticate(&'a [u8]),
    Listen(u8),
    Channels,
    Assign(u64),
    SetVariable { key: &'a [u8], value: &'a [u8] },
    GetVariable(&'a [u8]),
}

pub fn command(bytes: &[u8]) -> Result<Command<'_>, Disconnect> {
    let mut d = Decoder::new(bytes);
    if d.array().map_err(bad)? != Some(2) {
        return Err(Disconnect::PROTOCOL);
    }
    let op = d.u8().map_err(bad)?;
    let command = match op {
        0 => Command::Authenticate(string_bytes(&mut d)?),
        1 => Command::Listen(d.u8().map_err(bad)?),
        2 => {
            d.null().map_err(bad)?;
            Command::Channels
        }
        3 => Command::Assign(d.u64().map_err(bad)?),
        4 => {
            if d.array().map_err(bad)? != Some(2) {
                return Err(Disconnect::PROTOCOL);
            }
            let key = string_bytes(&mut d)?;
            if key.is_empty() || key.len() > 255 {
                return Err(Disconnect::PROTOCOL);
            }
            let start = d.position();
            validate_value(&mut d)?;
            Command::SetVariable {
                key,
                value: &bytes[start..d.position()],
            }
        }
        5 => {
            let key = string_bytes(&mut d)?;
            if key.is_empty() || key.len() > 255 {
                return Err(Disconnect::PROTOCOL);
            }
            Command::GetVariable(key)
        }
        _ => return Err(Disconnect::PROTOCOL),
    };
    if d.position() != bytes.len() {
        return Err(Disconnect::PROTOCOL);
    }
    Ok(command)
}

#[cfg(test)]
mod tests {
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
}
