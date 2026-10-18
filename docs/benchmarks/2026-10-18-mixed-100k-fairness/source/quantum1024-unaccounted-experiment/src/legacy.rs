//! The original x86 wire format is explicitly little-endian on every architecture.
use base64::{Engine, engine::general_purpose::STANDARD};
use bytes::Bytes;
use tokio_tungstenite::tungstenite::Message;

pub use crate::common::{BROADCAST, Disconnect};

pub fn binary_target(bytes: &[u8]) -> Result<u64, Disconnect> {
    if bytes.len() <= 8 {
        return Err(Disconnect::PROTOCOL);
    }
    Ok(u64::from_le_bytes(
        bytes[..8].try_into().expect("checked header"),
    ))
}

pub fn text_target(text: &str) -> Result<u64, Disconnect> {
    let bytes = text.as_bytes();
    if bytes.len() <= 12 {
        return Err(Disconnect::PROTOCOL);
    }
    // decode_slice needs room for its conservative upper bound (9, even for 8 bytes).
    let mut header = [0; 9];
    if STANDARD.decode_slice(&bytes[..12], &mut header).ok() != Some(8) {
        return Err(Disconnect::PROTOCOL);
    }
    Ok(u64::from_le_bytes(
        header[..8].try_into().expect("fixed header"),
    ))
}

pub fn disconnect_event(id: u64) -> Message {
    let mut bytes = [255; 16];
    bytes[8..].copy_from_slice(&id.to_le_bytes());
    Message::Binary(Bytes::copy_from_slice(&bytes))
}

pub fn variable_reply(value: &[u8]) -> Message {
    let mut bytes = Vec::with_capacity(9 + value.len());
    bytes.extend_from_slice(&[0; 8]);
    bytes.push(200);
    bytes.extend_from_slice(value);
    Message::Binary(bytes.into())
}

#[derive(Debug, PartialEq, Eq)]
pub enum Command<'a> {
    Authenticate(&'a [u8]),
    Listen(u8),
    Channels,
    Assign(u64),
    SetVariable { key: &'a [u8], value: &'a [u8] },
    GetVariable(&'a [u8]),
    Ignore,
}

pub fn command(bytes: &[u8]) -> Result<Command<'_>, Disconnect> {
    let (&opcode, payload) = bytes.split_first().ok_or(Disconnect::PROTOCOL)?;
    Ok(match opcode {
        0 => Command::Authenticate(payload),
        1 if payload.len() == 1 => Command::Listen(payload[0]),
        2 if payload.is_empty() => Command::Channels,
        3 if payload.len() == 8 => {
            Command::Assign(u64::from_le_bytes(payload.try_into().expect("checked ID")))
        }
        1..=3 => Command::Ignore,
        4 => {
            let (&length, rest) = payload.split_first().ok_or(Disconnect::PROTOCOL)?;
            let length = usize::from(length);
            if length == 0 || rest.len() < length {
                return Err(Disconnect::PROTOCOL);
            }
            Command::SetVariable {
                key: &rest[..length],
                value: &rest[length..],
            }
        }
        5 if !payload.is_empty() => Command::GetVariable(payload),
        5 => Command::Ignore,
        _ => return Err(Disconnect::PROTOCOL),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_little_endian_and_base64_vectors() {
        assert_eq!(
            binary_target(&[1, 2, 3, 4, 5, 6, 7, 8, 42]),
            Ok(0x0807060504030201)
        );
        assert_eq!(text_target("AQIDBAUGBwg=hello"), Ok(0x0807060504030201));
        assert_eq!(text_target("//////////8=x"), Ok(BROADCAST));
        for text in ["AAAAAAAAAAA=", "!!!!!!!!!!!!x", "ééééééx", "AAAAAAAAAAB=x"] {
            assert_eq!(text_target(text), Err(Disconnect::PROTOCOL));
        }
    }

    #[test]
    fn truncated_variable_packets_are_rejected_without_reading_outside_payload() {
        for bytes in [&[4][..], &[4, 3, b'a'][..], &[4, 0][..]] {
            assert_eq!(command(bytes), Err(Disconnect::PROTOCOL));
        }
        assert_eq!(
            command(&[4, 1, b'x']),
            Ok(Command::SetVariable {
                key: b"x",
                value: b""
            })
        );
    }
}
