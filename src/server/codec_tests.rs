use std::io::Cursor;

use bytes::Bytes;
use tokio_tungstenite::tungstenite::{
    Error, Message, WebSocket,
    protocol::{
        Role, WebSocketConfig,
        frame::{
            Frame, FrameSocket,
            coding::{Data, OpCode},
        },
    },
};

#[test]
fn lowering_the_write_target_applies_to_already_buffered_frames() {
    let config = WebSocketConfig::default()
        .write_buffer_size(64)
        .max_write_buffer_size(128);
    let mut socket =
        WebSocket::from_raw_socket(Cursor::new(Vec::new()), Role::Server, Some(config));
    socket
        .write(Message::Binary(Bytes::from_static(&[1, 2])))
        .unwrap();
    assert!(socket.get_ref().get_ref().is_empty());
    socket.set_config(|config| config.write_buffer_size = 0);
    socket
        .write(Message::Binary(Bytes::from_static(&[3, 4])))
        .unwrap();
    assert_eq!(socket.get_ref().get_ref(), &[0x82, 2, 1, 2, 0x82, 2, 3, 4]);
}

#[test]
fn changing_the_write_limit_preserves_queued_frames_and_allows_retry() {
    let config = WebSocketConfig::default()
        .write_buffer_size(64)
        .max_write_buffer_size(128);
    let mut socket =
        WebSocket::from_raw_socket(Cursor::new(Vec::new()), Role::Server, Some(config));
    socket
        .write(Message::Binary(Bytes::from_static(&[1; 16])))
        .unwrap();
    socket.set_config(|config| config.max_write_buffer_size = 72);
    assert!(matches!(
        socket.write(Message::Binary(Bytes::from_static(&[2; 60]))),
        Err(Error::WriteBufferFull(_))
    ));
    assert!(socket.get_ref().get_ref().is_empty());
    socket.set_config(|config| config.max_write_buffer_size = 128);
    socket
        .write(Message::Binary(Bytes::from_static(&[2; 60])))
        .unwrap();
    let mut expected = vec![0x82, 16];
    expected.extend_from_slice(&[1; 16]);
    expected.extend_from_slice(&[0x82, 60]);
    expected.extend_from_slice(&[2; 60]);
    assert_eq!(socket.get_ref().get_ref(), &expected);
}

#[test]
fn raw_frame_socket_keeps_its_default_immediate_write_policy() {
    let mut socket = FrameSocket::new(Cursor::new(Vec::new()));
    socket
        .write(Frame::message(
            Bytes::from_static(&[1, 2]),
            OpCode::Data(Data::Binary),
            true,
        ))
        .unwrap();
    assert_eq!(socket.get_ref().get_ref(), &[0x82, 2, 1, 2]);
}
