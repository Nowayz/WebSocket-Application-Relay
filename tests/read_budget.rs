//! Public decoder admission: account before growth and retain published owners.
use std::{
    io::{self, Cursor, Read, Write},
    sync::{Arc, Barrier},
};
use tokio_tungstenite::tungstenite::{
    Error, Message, WebSocket,
    error::CapacityError,
    protocol::{ReadBudget, Role, WebSocketConfig},
};

struct Paused(Cursor<Vec<u8>>);
impl Read for Paused {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        match self.0.read(buf)? {
            0 => Err(io::ErrorKind::WouldBlock.into()),
            n => Ok(n),
        }
    }
}
impl Write for Paused {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn socket(wire: Vec<u8>, pool: &Arc<ReadBudget>) -> WebSocket<Paused> {
    let lease = pool.try_lease(8192).unwrap();
    let mut ws = WebSocket::from_raw_socket(
        Paused(Cursor::new(wire)),
        Role::Server,
        Some(WebSocketConfig::default().read_buffer_size(4096)),
    );
    ws.set_read_budget(lease).unwrap();
    ws
}

fn frame(opcode: u8, body: &[u8]) -> Vec<u8> {
    let mut wire = vec![opcode];
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
    wire.extend_from_slice(&[0; 4]);
    wire.extend_from_slice(body);
    wire
}

fn pending(ws: &mut WebSocket<Paused>) {
    assert!(matches!(ws.read(), Err(Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock));
}
fn denied(ws: &mut WebSocket<Paused>) {
    assert!(matches!(
        ws.read(),
        Err(Error::Capacity(CapacityError::ReadBufferBudgetExceeded))
    ));
}

fn shared_socket(wire: Vec<u8>, pool: &Arc<ReadBudget>) -> WebSocket<Paused> {
    let mut ws = WebSocket::from_raw_socket(
        Paused(Cursor::new(wire)),
        Role::Server,
        Some(
            WebSocketConfig::default()
                .read_buffer_size(4096)
                .shared_read_buffer(true),
        ),
    );
    ws.set_read_budget(pool.try_lease(0).unwrap()).unwrap();
    ws
}

#[test]
fn worker_storage_moves_between_decoders_without_moving_published_payloads() {
    let pool = ReadBudget::new(1024 * 1024);
    let mut first = shared_socket(frame(0x82, &[0xa5; 2048]), &pool);
    let mut second = shared_socket(frame(0x82, &[0x39; 1024]), &pool);
    assert_eq!(
        pool.reserved(),
        0,
        "idle decoders allocate no receive storage"
    );
    let Message::Binary(published) = first.read().unwrap() else {
        panic!("binary")
    };
    let pointer = published.as_ptr();
    let mut scratch = first.take_read_storage().unwrap();
    let reserved = pool.reserved();
    drop(first);
    assert_eq!(
        pool.reserved(),
        reserved,
        "the allowance travels with storage"
    );
    assert!(second.swap_read_storage(&mut scratch));
    assert_eq!(second.read().unwrap().into_data(), &[0x39; 1024][..]);
    assert_eq!(published.as_ptr(), pointer);
    assert_eq!(published, &[0xa5; 2048][..]);
    scratch = second.take_read_storage().unwrap();
    drop(second);
    assert!(pool.reserved() > 0);
    drop(scratch);
    assert_eq!(
        pool.reserved(),
        0,
        "published owners are outside staging admission"
    );
    assert_eq!(published.as_ptr(), pointer);
}

#[test]
fn storage_lending_keeps_every_partial_masked_header_and_payload_with_its_decoder() {
    let body = vec![0x5a; 65536];
    let wire = frame(0x82, &body);
    for cut in (1..=14).chain([15, 4096]) {
        let pool = ReadBudget::new(1024 * 1024);
        let mut source = shared_socket(wire[..cut].to_vec(), &pool);
        let mut other = shared_socket(frame(0x82, b"other connection"), &pool);
        pending(&mut source);
        assert!(
            source.take_read_storage().is_none(),
            "partial frame cut {cut}"
        );
        let mut scratch = Default::default();
        assert!(other.swap_read_storage(&mut scratch));
        assert_eq!(other.read().unwrap().into_data(), b"other connection"[..]);
        if let Some(storage) = other.take_read_storage() {
            scratch = storage;
        }
        assert!(!source.swap_read_storage(&mut scratch));
        source.get_mut().0.get_mut().extend_from_slice(&wire[cut..]);
        assert_eq!(source.read().unwrap().into_data(), body);
        drop(source);
        drop(other);
        drop(scratch);
        assert_eq!(pool.reserved(), 0, "header/payload cut {cut}");
    }
}

#[test]
fn an_oversized_worker_slab_retires_even_when_a_published_body_keeps_its_allocation_alive() {
    for admission in [false, true] {
        let pool = ReadBudget::new(1024 * 1024);
        let mut source = WebSocket::from_raw_socket(
            Paused(Cursor::new(frame(0x82, &[0x5a; 65536]))),
            Role::Server,
            Some(
                WebSocketConfig::default()
                    .read_buffer_size(4096)
                    .shared_read_buffer(true),
            ),
        );
        if admission {
            source.set_read_budget(pool.try_lease(0).unwrap()).unwrap();
        }
        let published = source.read().unwrap().into_data();
        let pointer = published.as_ptr();
        let mut scratch = source.take_read_storage().unwrap();
        assert!(scratch.discard_if_larger_than(4096));
        assert_eq!(pool.reserved(), 0);
        assert_eq!(published.as_ptr(), pointer);
        assert_eq!(published, &[0x5a; 65536][..]);
        assert!(!scratch.discard_if_larger_than(4096));
    }
}

#[test]
fn a_decoder_still_reserves_before_growth_after_lending_away_its_allocation() {
    let pool = ReadBudget::new(32768);
    let mut source = shared_socket(frame(0x82, b"first"), &pool);
    assert_eq!(source.read().unwrap().into_data(), b"first"[..]);
    let held = source.take_read_storage().unwrap();
    let large = frame(0x82, &[0; 65536]);
    source.get_mut().0.get_mut().extend_from_slice(&large[..14]);
    denied(&mut source);
    assert_eq!(pool.rejected(), 1);
    assert!(pool.peak() <= pool.limit());
    drop(source);
    drop(held);
    assert_eq!(pool.reserved(), 0);
}

#[test]
fn concurrent_leases_never_exceed_the_limit_and_release_on_drop() {
    let pool = ReadBudget::new(4096);
    let held = Arc::new(Barrier::new(33));
    let release = Arc::new(Barrier::new(33));
    std::thread::scope(|scope| {
        for _ in 0..32 {
            let (pool, held, release) = (pool.clone(), held.clone(), release.clone());
            scope.spawn(move || {
                let lease = pool.try_lease(512);
                held.wait();
                release.wait();
                drop(lease);
            });
        }
        held.wait();
        assert_eq!(pool.reserved(), 4096);
        assert_eq!(pool.peak(), 4096);
        assert_eq!(pool.rejected(), 24);
        release.wait();
    });
    assert_eq!(pool.reserved(), 0);
    assert!(pool.try_lease(usize::MAX).is_err());
    assert_eq!(pool.reserved(), 0);
}

#[test]
fn a_header_is_denied_before_body_allocation_and_cannot_resume_unreserved() {
    let pool = ReadBudget::new(32768);
    let wire = frame(0x82, &vec![0xa5; 65536]);
    let mut ws = socket(wire[..14].to_vec(), &pool);
    denied(&mut ws);
    assert_eq!(pool.reserved(), 8192);
    assert_eq!(pool.peak(), 8192);
    assert_eq!(pool.rejected(), 1);
    assert!(!ws.can_read());
    denied(&mut ws);
    assert_eq!(pool.rejected(), 1);
    ws.close(None).unwrap();
    drop(ws);
    assert_eq!(pool.reserved(), 0);
}

#[test]
fn growth_reclaim_and_drop_preserve_the_published_payload_pointer() {
    let body = vec![0xa5; 131073];
    let wire = frame(0x82, &body);
    let pool = ReadBudget::new(1024 * 1024);
    let mut ws = socket(wire[..4111].to_vec(), &pool);
    pending(&mut ws);
    assert_eq!(pool.reserved(), 2 * (body.len() + 4096));
    assert_eq!(ws.reclaim_read_buffer(), None);
    ws.get_mut().0.get_mut().extend_from_slice(&wire[4111..]);
    let Message::Binary(published) = ws.read().unwrap() else {
        panic!("binary")
    };
    let pointer = published.as_ptr();
    assert_eq!(published.as_ref(), body);
    pending(&mut ws);
    assert_eq!(ws.reclaim_read_buffer(), Some(0));
    assert_eq!(pool.reserved(), 8192);
    assert!(published.is_unique());
    assert_eq!(published.as_ptr(), pointer);
    drop(ws);
    assert_eq!(pool.reserved(), 0);
    assert_eq!(published.as_ptr(), pointer);
    assert_eq!(published.as_ref(), body);
}

#[test]
fn fragment_assembly_is_admitted_separately_and_held_until_safe_reclaim() {
    let pool = ReadBudget::new(115000);
    let fragment = vec![0x5a; 16384];
    let mut wire = frame(0x02, &fragment);
    wire.extend(frame(0x00, &fragment));
    let mut ws = socket(wire, &pool);
    pending(&mut ws);
    assert_eq!(pool.reserved(), 40960 + 65536);
    assert_eq!(ws.reclaim_read_buffer(), Some(0));
    assert_eq!(pool.reserved(), 8192 + 65536);
    ws.get_mut().0.get_mut().extend(frame(0x80, &[]));
    let Message::Binary(published) = ws.read().unwrap() else {
        panic!("binary")
    };
    assert_eq!(published.len(), 32768);
    assert!(published.iter().all(|&b| b == 0x5a));
    assert_eq!(pool.reserved(), 8192 + 65536);
    let pointer = published.as_ptr();
    assert_eq!(ws.reclaim_read_buffer(), Some(0));
    assert_eq!(pool.reserved(), 8192);
    assert_eq!(published.as_ptr(), pointer);
    drop(ws);
    assert_eq!(pool.reserved(), 0);

    let mut wire = frame(0x02, &fragment);
    wire.extend(frame(0x00, &fragment));
    wire.extend(frame(0x80, &fragment));
    let mut ws = socket(wire, &pool);
    denied(&mut ws);
    assert_eq!(pool.reserved(), 40960 + 65536);
    assert_eq!(pool.rejected(), 1);
    drop(ws);
    assert_eq!(pool.reserved(), 0);
}

#[test]
fn fragmented_utf8_carries_partial_codepoints_and_control_frames() {
    let pool = ReadBudget::new(65536);
    let mut wire = frame(0x01, &[0xf0]);
    wire.extend(frame(0x00, &[0x9f]));
    wire.extend(frame(0x89, b"probe"));
    wire.extend(frame(0x00, &[0x98]));
    wire.extend(frame(0x80, &[0x80]));
    let mut ws = socket(wire, &pool);
    assert_eq!(
        ws.read().unwrap(),
        Message::Ping(b"probe".as_slice().into())
    );
    assert_eq!(ws.read().unwrap(), Message::text("😀"));
    pending(&mut ws);
    assert_eq!(pool.reserved(), 8192 + 16);
    assert_eq!(ws.reclaim_read_buffer(), Some(0));
    assert_eq!(pool.reserved(), 8192);
    drop(ws);
    assert_eq!(pool.reserved(), 0);
}

#[test]
fn fragment_growth_fits_the_message_limit_and_rejects_only_actual_excess() {
    for limit in [1, 2, 3, 4, 5, 7, 8, 9, 17, 31, 33, 65] {
        let body = vec![0x5a; limit];
        let split = (limit * 3 / 4).max(1);
        let pool = ReadBudget::new(8192 + 2 * limit);
        let mut wire = frame(0x02, &body[..split]);
        wire.extend(frame(0x80, &body[split..]));
        let mut ws = socket(wire, &pool);
        ws.set_config(|config| config.max_message_size = Some(limit));
        let Message::Binary(published) = ws.read().unwrap() else {
            panic!("binary")
        };
        assert_eq!(published.as_ref(), body);
        assert_eq!(pool.reserved(), 8192 + 2 * limit);
        assert_eq!(pool.rejected(), 0);
        let pointer = published.as_ptr();
        assert_eq!(ws.reclaim_read_buffer(), Some(0));
        assert_eq!(pool.reserved(), 8192);
        drop(ws);
        assert_eq!(pool.reserved(), 0);
        assert_eq!(published.as_ptr(), pointer);
        assert_eq!(published.as_ref(), body);

        let mut wire = frame(0x02, &body[..split]);
        let mut tail = body[split..].to_vec();
        tail.push(0x5a);
        wire.extend(frame(0x80, &tail));
        let mut ws = socket(wire, &pool);
        ws.set_config(|config| config.max_message_size = Some(limit));
        assert!(matches!(
            ws.read(),
            Err(Error::Capacity(CapacityError::MessageTooLong { .. }))
        ));
        assert_eq!(
            pool.rejected(),
            0,
            "message validation must precede growth admission"
        );
        drop(ws);
        assert_eq!(pool.reserved(), 0);
    }
}

#[test]
fn text_assembly_caps_capacity_across_every_utf8_split() {
    for text in ["x", "π", "€", "😀", "€x", "😀xy"] {
        let body = text.as_bytes();
        for split in 1..=body.len() {
            let pool = ReadBudget::new(8192 + 2 * body.len());
            let mut wire = frame(0x01, &body[..split]);
            wire.extend(frame(0x80, &body[split..]));
            let mut ws = socket(wire, &pool);
            ws.set_config(|config| config.max_message_size = Some(body.len()));
            assert_eq!(ws.read().unwrap(), Message::text(text));
            assert_eq!(pool.reserved(), 8192 + 2 * body.len());
            assert_eq!(pool.rejected(), 0);
            assert_eq!(ws.reclaim_read_buffer(), Some(0));
            assert_eq!(pool.reserved(), 8192);
            drop(ws);
            assert_eq!(pool.reserved(), 0);
        }
    }
}

#[test]
fn final_growth_uses_the_known_length_with_a_larger_configured_limit() {
    for opcode in [0x01, 0x02] {
        for length in [9, 17, 33, 65, 257] {
            let body = vec![0x5a; length];
            let split = length * 3 / 4;
            let pool = ReadBudget::new(8192 + 2 * length);
            let mut wire = frame(opcode, &body[..split]);
            wire.extend(frame(0x80, &body[split..]));
            let mut ws = socket(wire, &pool);
            ws.set_config(|config| config.max_message_size = Some(length * 4));
            let published = ws.read().unwrap();
            let data: &[u8] = match &published {
                Message::Binary(bytes) => bytes,
                Message::Text(text) => text.as_bytes(),
                _ => panic!("expected assembled data"),
            };
            let pointer = data.as_ptr();
            assert_eq!(data, body);
            assert_eq!(pool.reserved(), 8192 + 2 * length);
            assert_eq!(pool.rejected(), 0);
            assert_eq!(ws.reclaim_read_buffer(), Some(0));
            drop(ws);
            assert_eq!(pool.reserved(), 0);
            assert_eq!(data.as_ptr(), pointer);
            assert_eq!(data, body);
        }
    }
}

#[test]
fn intermediate_growth_stays_within_the_configured_message_ceiling() {
    for opcode in [0x01, 0x02] {
        for length in [9, 17, 33, 65] {
            let body = vec![0x5a; length];
            let split = length * 3 / 4;
            let pool = ReadBudget::new(8192 + 2 * length);
            let mut wire = frame(opcode, &body[..split]);
            wire.extend(frame(0x00, &body[split..]));
            wire.extend(frame(0x80, &[]));
            let mut ws = socket(wire, &pool);
            ws.set_config(|config| config.max_message_size = Some(length));
            assert_eq!(ws.read().unwrap().into_data().as_ref(), body);
            assert_eq!(pool.reserved(), 8192 + 2 * length);
            assert_eq!(pool.rejected(), 0);
            drop(ws);
            assert_eq!(pool.reserved(), 0);
        }
    }
}
