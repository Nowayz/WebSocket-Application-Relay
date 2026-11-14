use super::*;

#[test]
fn every_partial_record_offset_reconstructs_the_exact_remaining_bytes() {
    for frame in [
        Frame::integer(u64::MAX),
        Frame::envelope(u64::MAX, Bytes::from_static(b"\x44body")),
    ] {
        let original = frame_chunks(&frame, 0).concat();
        for offset in 0..=original.len() {
            assert_eq!(frame_chunks(&frame, offset).concat(), original[offset..]);
        }
    }
}
