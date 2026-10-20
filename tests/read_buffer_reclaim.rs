//! Exercise the narrow decoder hook through its public WebSocket API.
use std::io::{self, Cursor, Read, Write};

use bytes::Bytes;
use tokio_tungstenite::tungstenite::{
    Error, Message, WebSocket,
    protocol::{Role, WebSocketConfig},
};

struct Paused(Cursor<Vec<u8>>);

impl Read for Paused {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buffer)? {
            0 => Err(io::ErrorKind::WouldBlock.into()),
            count => Ok(count),
        }
    }
}

impl Write for Paused {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn socket(bytes: Vec<u8>) -> WebSocket<Paused> {
    WebSocket::from_raw_socket(
        Paused(Cursor::new(bytes)),
        Role::Server,
        Some(WebSocketConfig::default().read_buffer_size(4096)),
    )
}

fn binary(socket: &mut WebSocket<Paused>) -> Bytes {
    let Message::Binary(bytes) = socket.read().unwrap() else {
        panic!("expected binary data");
    };
    bytes
}

fn pending(socket: &mut WebSocket<Paused>) {
    assert!(matches!(socket.read(),
        Err(Error::Io(error)) if error.kind() == io::ErrorKind::WouldBlock));
}

fn masked(body: &[u8]) -> Vec<u8> {
    let mut wire = vec![0x82];
    match body.len() {
        0..=125 => wire.push(0x80 | body.len() as u8),
        126..=65535 => {
            wire.push(0xfe);
            wire.extend_from_slice(&(body.len() as u16).to_be_bytes());
        }
        _ => {
            wire.push(0xff);
            wire.extend_from_slice(&(body.len() as u64).to_be_bytes());
        }
    }
    let mask = [0x17, 0x39, 0x5b, 0x7d];
    wire.extend_from_slice(&mask);
    wire.extend(body.iter().enumerate().map(|(i, byte)| byte ^ mask[i & 3]));
    wire
}

#[test]
fn decoder_releases_ownership_without_copying_published_data_or_losing_partial_headers() {
    let first_body = vec![0xa5; 128 * 1024 + 7];
    let next_body = vec![0x5a; 64 * 1024 + 17];
    let next_wire = masked(&next_body);
    for prefix in 0..14 {
        let mut wire = masked(&first_body);
        wire.extend_from_slice(&next_wire[..prefix]);
        let mut ws = socket(wire);
        let published = binary(&mut ws);
        let pointer = published.as_ptr();
        pending(&mut ws);
        assert!(
            !published.is_unique(),
            "fixture must retain decoder ownership"
        );
        assert_eq!(ws.reclaim_read_buffer(), Some(prefix));
        assert!(
            published.is_unique(),
            "decoder must release its shared allocation"
        );
        assert_eq!(published.as_ptr(), pointer);
        assert_eq!(published, first_body);
        ws.get_mut()
            .0
            .get_mut()
            .extend_from_slice(&next_wire[prefix..]);
        assert_eq!(binary(&mut ws), next_body);
        assert_eq!(published.as_ptr(), pointer);
        assert_eq!(published, first_body);
    }
}

#[test]
fn partially_received_payload_is_preserved_across_a_deferred_reclaim() {
    let body = vec![0x5a; 128 * 1024 + 17];
    let wire = masked(&body);
    for received in [0, 1, 4097] {
        let mut ws = socket(wire[..14 + received].to_vec());
        pending(&mut ws);
        assert_eq!(ws.reclaim_read_buffer(), None);
        ws.get_mut()
            .0
            .get_mut()
            .extend_from_slice(&wire[14 + received..]);
        assert_eq!(binary(&mut ws), body);
    }
}

#[test]
fn a_coalesced_small_message_is_left_in_its_original_storage() {
    let large_body = vec![0xa5; 128 * 1024 + 7];
    let small_body = [1, 2, 3];
    let mut wire = masked(&large_body);
    wire.extend_from_slice(&masked(&small_body));
    let mut ws = socket(wire);
    let published = binary(&mut ws);
    let small_pointer = published.as_ptr().wrapping_add(published.len() + 6);
    assert_eq!(ws.reclaim_read_buffer(), None);
    let small = binary(&mut ws);
    assert_eq!(small.as_ref(), small_body);
    assert_eq!(small.as_ptr(), small_pointer);
    assert_eq!(published, large_body);
}
