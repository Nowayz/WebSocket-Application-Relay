use super::*;

struct Fragmented {
    bytes: Vec<u8>,
    position: usize,
    pause: bool,
}

impl AsyncRead for Fragmented {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        read: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.pause {
            this.pause = false;
            cx.waker().wake_by_ref();
            return Poll::Pending;
        }
        let count = read
            .remaining()
            .min(3)
            .min(this.bytes.len() - this.position);
        read.put_slice(&this.bytes[this.position..this.position + count]);
        this.position += count;
        this.pause = true;
        Poll::Ready(Ok(()))
    }
}

#[test]
fn fragmented_record_completion_and_cancellation_preserve_the_budget_and_eof_contract() {
    for arena_enabled in [false, true] {
        let arena = crate::receive_arena::ReceiveArena::new(32768, 16384);
        let arena = arena_enabled.then_some(&arena);
        let payload: Vec<_> = (0..9000).map(|index| (index % 251) as u8).collect();
        let mut bytes = (payload.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(&payload);
        bytes.extend_from_slice(&[0; 4]);
        let budget = crate::read_budget::ReadBudget::new(payload.len() * 2);
        let mut cx = Context::from_waker(std::task::Waker::noop());
        let mut cancelled = RecordReader::default();
        let mut stream = Fragmented {
            bytes: bytes.clone(),
            position: 0,
            pause: false,
        };
        while budget.reserved() == 0 {
            assert!(
                cancelled
                    .poll(&mut stream, payload.len(), Some(&budget), arena, &mut cx)
                    .is_pending()
            );
        }
        assert_eq!(budget.reserved(), payload.len() * 2);
        drop(cancelled);
        assert_eq!(budget.reserved(), 0);

        let mut reader = RecordReader::default();
        let mut stream = Fragmented {
            bytes,
            position: 0,
            pause: false,
        };
        let received = loop {
            if let Poll::Ready(result) =
                reader.poll(&mut stream, payload.len(), Some(&budget), arena, &mut cx)
            {
                break result.unwrap();
            }
        };
        assert_eq!(received.as_ref(), payload.as_slice());
        assert_eq!(budget.reserved(), 0);
        assert_eq!(budget.peak(), payload.len() * 2);
        let close = loop {
            if let Poll::Ready(result) =
                reader.poll(&mut stream, payload.len(), Some(&budget), arena, &mut cx)
            {
                break result.unwrap();
            }
        };
        assert!(close.is_empty());
        let eof = loop {
            if let Poll::Ready(result) =
                reader.poll(&mut stream, payload.len(), Some(&budget), arena, &mut cx)
            {
                break result.unwrap_err();
            }
        };
        assert_eq!(
            eof.downcast_ref::<std::io::Error>().unwrap().kind(),
            std::io::ErrorKind::UnexpectedEof
        );
        assert_eq!(budget.reserved(), 0);
    }
}
