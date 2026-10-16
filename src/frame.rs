//! An outgoing message keeps its small routing prefix separate from its body.
//!
//! Receive storage becomes immutable before publication. Queue clones share the
//! body, and the server writes the slices without assembling another message.
use bytes::{Buf, Bytes};
use minicbor::{Encoder, encode::write::Cursor};

#[derive(Clone)]
pub(crate) struct Frame {
    prefix: [u8; 10],
    prefix_len: u8,
    payload: Bytes,
}

impl Frame {
    pub(crate) fn encoded(payload: Bytes) -> Self {
        Self {
            prefix: [0; 10],
            prefix_len: 0,
            payload,
        }
    }

    /// `value_start` comes from validation of this complete CBOR envelope.
    /// A compact copy is reserved for small views of a much larger read slab.
    pub(crate) fn routed(sender: u64, mut input: Bytes, value_start: usize, compact: bool) -> Self {
        let payload = if compact {
            Bytes::copy_from_slice(&input[value_start..])
        } else {
            // Transfer the input owner directly into the body. Advancing this
            // owned view needs no additional shared-buffer reference.
            input.advance(value_start);
            input
        };
        let mut frame = Self::encoded(payload);
        let mut e = Encoder::new(Cursor::new(frame.prefix.as_mut_slice()));
        e.array(2)
            .and_then(|e| e.u64(sender))
            .expect("ten bytes fit an array and uint64");
        frame.prefix_len = e.writer().position() as u8;
        frame
    }

    pub(crate) fn prefix(&self) -> &[u8] {
        &self.prefix[..usize::from(self.prefix_len)]
    }

    pub(crate) fn payload(&self) -> &[u8] {
        &self.payload
    }

    pub(crate) fn len(&self) -> usize {
        usize::from(self.prefix_len) + self.payload.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol;

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
                    let frame = Frame::routed(sender, input, offset, false);
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
            let frame = Frame::routed(sender, input.clone(), offset, false);
            assert_eq!(frame.payload().as_ptr(), pointer);
            let mut wire = frame.prefix().to_vec();
            wire.extend_from_slice(frame.payload());
            assert_eq!(protocol::parse(&wire).unwrap(), (sender, value));
        }
    }

    #[test]
    fn compaction_detaches_a_small_value_from_shared_receive_storage() {
        let input = protocol::envelope(42, &[0xf6]);
        let backing = input.clone();
        let frame = Frame::routed(7, input, 3, true);
        assert_ne!(frame.payload().as_ptr(), backing[3..].as_ptr());
        assert_eq!(frame.payload(), [0xf6]);
        assert_eq!(frame.prefix(), [0x82, 7]);
    }
}
