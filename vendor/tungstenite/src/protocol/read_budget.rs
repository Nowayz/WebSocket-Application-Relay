//! Shared admission for decoder staging, independent of published message owners.
use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
    Arc,
};

use crate::error::{CapacityError, Error, Result};

/// A process-wide staging allowance. This is not an allocator or an RSS limit.
///
/// Decoders retain high-water leases, avoiding shared atomics on ordinary reads.
/// Frame and fragment buffers reserve twice their requested capacity, covering
/// old and new staging allocations while growing. Published messages are owned
/// by their consumers and are outside this allowance.
#[derive(Debug)]
pub struct ReadBudget {
    limit: usize,
    reserved: AtomicUsize,
    peak: AtomicUsize,
    rejected: AtomicU64,
}

impl ReadBudget {
    /// Create an empty budget. A zero limit admits only zero-sized leases.
    pub fn new(limit: usize) -> Arc<Self> {
        Arc::new(Self {
            limit,
            reserved: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            rejected: AtomicU64::new(0),
        })
    }

    /// Reserve before allocating. Dropping a lease releases its allowance.
    pub fn try_lease(self: &Arc<Self>, bytes: usize) -> Result<ReadLease> {
        let mut lease = ReadLease { budget: self.clone(), reserved: 0 };
        lease.grow_to(bytes)?;
        Ok(lease)
    }

    /// Configured allowance in bytes.
    pub fn limit(&self) -> usize {
        self.limit
    }

    /// Current reserved staging allowance, including growth headroom.
    pub fn reserved(&self) -> usize {
        self.reserved.load(Relaxed)
    }

    /// Largest successfully reserved allowance.
    pub fn peak(&self) -> usize {
        self.peak.load(Relaxed)
    }

    /// Failed growth or admission attempts.
    pub fn rejected(&self) -> u64 {
        self.rejected.load(Relaxed)
    }
}

/// One non-clonable staging reservation. No allocation is performed on growth.
#[derive(Debug)]
pub struct ReadLease {
    budget: Arc<ReadBudget>,
    reserved: usize,
}

impl ReadLease {
    pub(super) fn same_budget(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.budget, &other.budget)
    }
    pub(super) fn sibling(&self) -> Self {
        Self { budget: self.budget.clone(), reserved: 0 }
    }

    fn denied(&self) -> Error {
        self.budget.rejected.fetch_add(1, Relaxed);
        CapacityError::ReadBufferBudgetExceeded.into()
    }

    pub(super) fn grow_to(&mut self, bytes: usize) -> Result<()> {
        let Some(delta) = bytes.checked_sub(self.reserved) else { return Ok(()) };
        if delta == 0 {
            return Ok(());
        }
        let mut current = self.budget.reserved.load(Relaxed);
        loop {
            let Some(next) = current.checked_add(delta).filter(|&n| n <= self.budget.limit) else {
                return Err(self.denied());
            };
            match self.budget.reserved.compare_exchange_weak(current, next, Relaxed, Relaxed) {
                Ok(_) => {
                    self.budget.peak.fetch_max(next, Relaxed);
                    self.reserved = bytes;
                    return Ok(());
                }
                Err(observed) => current = observed,
            }
        }
    }

    pub(super) fn for_capacity(&mut self, capacity: usize) -> Result<()> {
        let bytes = capacity.checked_mul(2).ok_or_else(|| self.denied())?;
        self.grow_to(bytes)
    }

    pub(super) fn shrink_to(&mut self, bytes: usize) {
        assert!(bytes <= self.reserved, "a read lease cannot grow while shrinking");
        self.budget.reserved.fetch_sub(self.reserved - bytes, Relaxed);
        self.reserved = bytes;
    }
}

impl Drop for ReadLease {
    fn drop(&mut self) {
        self.budget.reserved.fetch_sub(self.reserved, Relaxed);
    }
}
