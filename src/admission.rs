//! Optional direct-peer admission. One shared entry per active canonical IP;
//! connection leases own its strong references, and the index keeps only weak
//! references. No per-message work or per-connection lease allocation is needed.
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{Arc, Weak},
};

use parking_lot::Mutex;

pub(crate) struct IpLimit {
    limit: usize,
    entries: Mutex<HashMap<IpAddr, Weak<Entry>>>,
}

struct Entry {
    address: IpAddr,
    owner: Weak<IpLimit>,
}

pub(crate) struct IpPermit {
    _entry: Arc<Entry>,
}

impl IpLimit {
    pub fn new(limit: usize) -> Option<Arc<Self>> {
        (limit != 0).then(|| {
            Arc::new(Self {
                limit,
                entries: Mutex::new(HashMap::new()),
            })
        })
    }

    pub fn acquire(self: &Arc<Self>, address: IpAddr) -> Option<IpPermit> {
        let address = address.to_canonical();
        let mut entries = self.entries.lock();
        let entry = if let Some(entry) = entries.get(&address).and_then(Weak::upgrade) {
            entry
        } else {
            let entry = Arc::new(Entry {
                address,
                owner: Arc::downgrade(self),
            });
            entries.insert(address, Arc::downgrade(&entry));
            entry
        };
        // Acquisitions are serialized; existing leases can only decrease this
        // count. The temporary upgraded reference becomes the new lease.
        let admitted = Arc::strong_count(&entry) <= self.limit;
        // A rejected candidate may become the last reference while another
        // thread closes its socket. Its destructor must run outside this lock.
        drop(entries);
        admitted.then_some(IpPermit { _entry: entry })
    }

    #[cfg(test)]
    pub fn address_count(&self) -> usize {
        self.entries.lock().len()
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        let Some(owner) = self.owner.upgrade() else {
            return;
        };
        let mut entries = owner.entries.lock();
        // A concurrent acquire can replace our expired weak reference before
        // this destructor gets the lock. Never remove that replacement.
        if entries
            .get(&self.address)
            .is_some_and(|entry| std::ptr::eq(entry.as_ptr(), self))
        {
            entries.remove(&self.address);
            if entries.capacity() > 256 && entries.len() <= entries.capacity() / 8 {
                let retain = entries.len() * 2;
                entries.shrink_to(retain);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::Barrier,
        time::{Duration, Instant},
    };

    #[test]
    fn addresses_are_canonical_bounded_and_retire_with_their_last_lease() {
        assert!(IpLimit::new(0).is_none());
        let limit = IpLimit::new(2).unwrap();
        let ip = "192.0.2.7".parse().unwrap();
        let first = limit.acquire(ip).unwrap();
        let second = limit.acquire("::ffff:192.0.2.7".parse().unwrap()).unwrap();
        assert!(limit.acquire(ip).is_none());
        let other = limit.acquire("2001:db8::7".parse().unwrap()).unwrap();
        assert_eq!(limit.address_count(), 2);
        drop(first);
        let replacement = limit.acquire(ip).unwrap();
        drop((second, replacement));
        assert_eq!(limit.address_count(), 1);
        drop(other);
        assert_eq!(limit.address_count(), 0);
    }

    #[test]
    fn concurrent_workers_share_one_admission_count() {
        let limit = IpLimit::new(8).unwrap();
        let barrier = Arc::new(Barrier::new(64));
        let mut workers = Vec::new();
        for _ in 0..64 {
            let limit = limit.clone();
            let barrier = barrier.clone();
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                limit.acquire("192.0.2.9".parse().unwrap())
            }));
        }
        let held: Vec<_> = workers
            .into_iter()
            .filter_map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(held.len(), 8);
        assert_eq!(limit.address_count(), 1);
        drop(held);
        assert_eq!(limit.address_count(), 0);
    }

    #[test]
    fn an_expired_entry_destructor_cannot_remove_its_live_replacement() {
        let limit = IpLimit::new(1).unwrap();
        let ip = "192.0.2.11".parse().unwrap();
        let permit = limit.acquire(ip).unwrap();
        let old = Arc::downgrade(&permit._entry);
        let mut entries = limit.entries.lock();
        let worker = std::thread::spawn(move || drop(permit));
        let deadline = Instant::now() + Duration::from_secs(2);
        while old.strong_count() != 0 {
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        // The old destructor is waiting for the index lock. This is the state
        // a new acquisition sees after its Weak::upgrade returns None.
        let replacement = Arc::new(Entry {
            address: ip,
            owner: Arc::downgrade(&limit),
        });
        entries.insert(ip, Arc::downgrade(&replacement));
        drop(entries);
        worker.join().unwrap();
        assert_eq!(limit.address_count(), 1);
        assert!(limit.acquire(ip).is_none());
        drop(replacement);
        assert_eq!(limit.address_count(), 0);
    }

    #[test]
    fn distinct_address_waves_do_not_retain_an_unbounded_index() {
        let limit = IpLimit::new(1).unwrap();
        let permits: Vec<_> = (1..=4096u32)
            .map(|i| limit.acquire(IpAddr::from(i.to_be_bytes())).unwrap())
            .collect();
        let peak = limit.entries.lock().capacity();
        drop(permits);
        let entries = limit.entries.lock();
        assert!(entries.is_empty());
        assert!(entries.capacity() <= 256 && entries.capacity() < peak);
    }
}
