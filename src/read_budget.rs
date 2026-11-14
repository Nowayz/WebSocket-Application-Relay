//! Shared admission for receive staging, independent of published message owners.
use std::sync::{
    Arc,
    atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed},
};

/// A process-wide staging allowance. This is not an allocator or an RSS limit.
///
/// Records in progress reserve twice their declared size, covering old and new
/// staging allocations while growing. Published messages are owned by their
/// consumers and are outside this allowance.
#[derive(Debug)]
pub struct ReadBudget {
    limit: usize,
    reserved: AtomicUsize,
    peak: AtomicUsize,
    rejected: AtomicU64,
}

/// The process-wide read budget is exhausted.
#[derive(Debug)]
pub struct ReadBudgetExceeded;

impl std::fmt::Display for ReadBudgetExceeded {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("read buffer budget exceeded")
    }
}

impl std::error::Error for ReadBudgetExceeded {}

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

    /// Reserve before allocating. Dropping the lease releases its allowance.
    pub fn try_lease(self: &Arc<Self>, bytes: usize) -> Result<ReadLease, ReadBudgetExceeded> {
        let mut current = self.reserved.load(Relaxed);
        loop {
            let Some(next) = current.checked_add(bytes).filter(|&n| n <= self.limit) else {
                self.rejected.fetch_add(1, Relaxed);
                return Err(ReadBudgetExceeded);
            };
            match self
                .reserved
                .compare_exchange_weak(current, next, Relaxed, Relaxed)
            {
                Ok(_) => {
                    self.peak.fetch_max(next, Relaxed);
                    return Ok(ReadLease {
                        budget: self.clone(),
                        reserved: bytes,
                    });
                }
                Err(observed) => current = observed,
            }
        }
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

    /// Failed admission attempts.
    pub fn rejected(&self) -> u64 {
        self.rejected.load(Relaxed)
    }
}

/// One non-clonable staging reservation.
#[derive(Debug)]
pub struct ReadLease {
    budget: Arc<ReadBudget>,
    reserved: usize,
}

impl Drop for ReadLease {
    fn drop(&mut self) {
        self.budget.reserved.fetch_sub(self.reserved, Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leases_reserve_until_dropped_and_refusals_are_counted() {
        let budget = ReadBudget::new(10);
        let first = budget.try_lease(6).unwrap();
        assert!(budget.try_lease(5).is_err());
        let second = budget.try_lease(4).unwrap();
        assert_eq!(
            (budget.reserved(), budget.peak(), budget.rejected()),
            (10, 10, 1)
        );
        drop(first);
        drop(second);
        assert_eq!(budget.reserved(), 0);
        assert!(budget.try_lease(0).is_ok());
        assert!(ReadBudget::new(0).try_lease(1).is_err());
    }
}
