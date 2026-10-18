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

/// Validate exactly one value without recursion or heap allocations.
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
                decoder.str().map_err(bad)?;
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

/// Encode a validated value. No decode/re-encode occurs in its application payload.
pub fn envelope(sender: u64, value: &[u8]) -> Bytes {
    let mut e = Encoder::new(Vec::with_capacity(10 + value.len()));
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

pub fn uint(value: u64) -> Bytes {
    let mut e = Encoder::new(Vec::with_capacity(9));
    e.u64(value).expect("Vec encoding");
    e.into_writer().into()
}

pub fn byte_string(value: &[u8]) -> Bytes {
    let mut e = Encoder::new(Vec::with_capacity(9 + value.len()));
    e.bytes(value).expect("Vec encoding");
    e.into_writer().into()
}

pub fn text(value: &str) -> Bytes {
    let mut e = Encoder::new(Vec::with_capacity(9 + value.len()));
    e.str(value).expect("Vec encoding");
    e.into_writer().into()
}

pub fn reply(op: u8, value: &[u8]) -> Bytes {
    let mut e = Encoder::new(Vec::with_capacity(13 + value.len()));
    e.array(2)
        .and_then(|e| e.u64(0))
        .and_then(|e| e.array(2))
        .and_then(|e| e.u8(op))
        .expect("Vec encoding");
    let mut bytes = e.into_writer();
    bytes.extend_from_slice(value);
    bytes.into()
}

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
