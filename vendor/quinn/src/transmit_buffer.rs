//! Packet construction is synchronous, so an unblocked connection can return
//! its scratch allocation for reuse by the next connection on this thread.
//! A socket-blocked transmit keeps its bytes with its own connection instead.
use std::{
    cell::RefCell,
    mem,
    ops::{Deref, DerefMut},
};

thread_local! {
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct TransmitBuffer(Vec<u8>);

impl TransmitBuffer {
    pub(crate) fn take(retained: &mut Vec<u8>) -> Self {
        let mut bytes = mem::take(retained);
        if bytes.capacity() == 0 {
            if let Ok(Some(scratch)) = SCRATCH.try_with(|slot| {
                slot.try_borrow_mut()
                    .ok()
                    .map(|mut slot| mem::take(&mut *slot))
            }) {
                bytes = scratch;
            }
        }
        Self(bytes)
    }

    pub(crate) fn retain(&mut self, pending: &mut Vec<u8>) {
        *pending = mem::take(&mut self.0);
    }
}

impl Deref for TransmitBuffer {
    type Target = Vec<u8>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for TransmitBuffer {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for TransmitBuffer {
    fn drop(&mut self) {
        if self.0.capacity() == 0 {
            return;
        }
        self.0.clear();
        // A nested/custom socket callback or thread-local teardown must not
        // panic or share a live construction buffer. In that case just free it.
        let _ = SCRATCH.try_with(|slot| {
            if let Ok(mut spare) = slot.try_borrow_mut() {
                if spare.capacity() < self.0.capacity() {
                    mem::swap(&mut *spare, &mut self.0);
                }
            }
        });
    }
}
