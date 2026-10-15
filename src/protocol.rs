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
                decoder.simple().map_err(bad)?;
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
