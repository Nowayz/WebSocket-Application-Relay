//! Bounded slabs shared by received records; published slices retain their slab.
use bytes::{Bytes, BytesMut};
use std::{cell::RefCell, sync::Arc};
use tungstenite::protocol::{ReadBudget, ReadLease};

thread_local! {
    static PAGE: RefCell<Option<Page>> = const { RefCell::new(None) };
    static SPARE: RefCell<Option<Page>> = const { RefCell::new(None) };
}

pub(crate) struct ReceiveArena {
    pub(crate) budget: Arc<ReadBudget>,
    page_bytes: usize,
}

struct Page {
    budget: Arc<ReadBudget>,
    storage: BytesMut,
    lease: Arc<PageLease>,
}

struct PageLease {
    _reservation: ReadLease,
    budget: Arc<ReadBudget>,
    bytes: usize,
}

pub(crate) struct ArenaBuffer {
    storage: BytesMut,
    _lease: Arc<PageLease>,
}

impl AsRef<[u8]> for ArenaBuffer {
    fn as_ref(&self) -> &[u8] {
        &self.storage
    }
}

impl Drop for ArenaBuffer {
    fn drop(&mut self) {
        // The last slice can recover the whole allocation without touching live views.
        let _ = SPARE.try_with(|slot| {
            let Ok(mut spare) = slot.try_borrow_mut() else {
                return;
            };
            if spare.is_none()
                && self
                    .storage
                    .try_reclaim(self._lease.bytes - self.storage.len())
            {
                self.storage.resize(self._lease.bytes, 0);
                *spare = Some(Page {
                    budget: self._lease.budget.clone(),
                    storage: std::mem::take(&mut self.storage),
                    lease: self._lease.clone(),
                });
            }
        });
    }
}

impl ArenaBuffer {
    pub(crate) fn as_mut(&mut self) -> &mut [u8] {
        &mut self.storage
    }

    pub(crate) fn publish(self) -> Bytes {
        Bytes::from_owner(self)
    }
}

impl ReceiveArena {
    pub(crate) fn new(bytes: usize, page_bytes: usize) -> Self {
        Self {
            budget: ReadBudget::new(bytes),
            page_bytes,
        }
    }

    pub(crate) fn allocate(&self, size: usize) -> Option<ArenaBuffer> {
        if size == 0 || size > self.page_bytes || self.budget.limit() == 0 {
            return None;
        }
        PAGE.with(|slot| {
            let mut cached = slot.borrow_mut();
            if cached
                .as_ref()
                .is_some_and(|p| !Arc::ptr_eq(&p.budget, &self.budget))
            {
                *cached = None;
            }
            if let Some(page) = cached.as_mut()
                && page.storage.len() < size
                && page
                    .storage
                    .try_reclaim(self.page_bytes - page.storage.len())
            {
                page.storage.resize(self.page_bytes, 0);
            }
            if cached.as_ref().is_none_or(|p| p.storage.len() < size) {
                // Release an exhausted tail before testing the process-wide cap.
                *cached = None;
                *cached = SPARE.with(|slot| slot.borrow_mut().take()).filter(|p| {
                    Arc::ptr_eq(&p.budget, &self.budget) && p.storage.len() == self.page_bytes
                });
                if cached.is_none() {
                    let lease = self.budget.try_lease(self.page_bytes).ok()?;
                    *cached = Some(Page {
                        budget: self.budget.clone(),
                        storage: BytesMut::zeroed(self.page_bytes),
                        lease: Arc::new(PageLease {
                            _reservation: lease,
                            budget: self.budget.clone(),
                            bytes: self.page_bytes,
                        }),
                    });
                }
            }
            let page = cached.as_mut().expect("allocated page");
            Some(ArenaBuffer {
                storage: page.storage.split_to(size),
                _lease: page.lease.clone(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clear() {
        PAGE.with(|p| *p.borrow_mut() = None);
        SPARE.with(|p| *p.borrow_mut() = None);
    }

    #[test]
    fn retained_slices_cannot_be_overwritten_and_exhaustion_falls_back() {
        clear();
        let arena = ReceiveArena::new(128, 64);
        let mut first = arena.allocate(32).unwrap();
        first.as_mut().fill(1);
        let first = first.publish();
        let held = first.slice(3..9);
        drop(first);
        let mut second = arena.allocate(32).unwrap();
        second.as_mut().fill(2);
        let second = second.publish();
        let mut third = arena.allocate(64).unwrap();
        third.as_mut().fill(3);
        let third = third.publish();
        assert!(arena.allocate(1).is_none());
        assert_eq!(arena.budget.reserved(), 128);
        assert_eq!(&held[..], &[1; 6]);
        assert_eq!(&second[..], &[2; 32]);
        assert_eq!(&third[..], &[3; 64]);
        drop((held, second, third));
        clear();
        assert_eq!(arena.budget.reserved(), 0);
        assert!(arena.allocate(129).is_none());
        clear();
    }

    #[test]
    fn consumed_page_is_reclaimed_without_growing_the_budget() {
        clear();
        let arena = ReceiveArena::new(64, 64);
        for n in 0..100 {
            let mut body = arena.allocate(17).unwrap();
            body.as_mut().fill(n);
            assert_eq!(&body.publish()[..], &[n; 17]);
        }
        assert_eq!(arena.budget.peak(), 64);
        assert_eq!(arena.budget.rejected(), 0);
        clear();
        assert_eq!(arena.budget.reserved(), 0);
        assert!(ReceiveArena::new(0, 64).allocate(1).is_none());
    }

    #[test]
    fn last_slice_returns_a_completed_page_after_the_worker_moves_on() {
        clear();
        let arena = ReceiveArena::new(128, 64);
        let mut first = arena.allocate(32).unwrap();
        first.as_mut().fill(7);
        let first = first.publish();
        let second = arena.allocate(32).unwrap().publish();
        let third = arena.allocate(64).unwrap().publish();
        drop(first);
        assert_eq!(&second[..], &[0; 32]);
        drop(second);
        assert!(SPARE.with(|p| p.borrow().is_some()));
        assert_eq!(arena.budget.reserved(), 128);
        assert!(arena.allocate(64).is_some());
        assert_eq!(arena.budget.rejected(), 0);
        drop(third);
        clear();
        assert_eq!(arena.budget.reserved(), 0);
    }
}
