//! One timer per live connection. Updating or cancelling never leaves stale
//! entries behind, so repeated traffic cannot grow the timer queue.
use std::time::Instant;

#[derive(Default)]
pub(super) struct Timers {
    heap: Vec<(Instant, usize)>,
    positions: Vec<usize>,
    census: [u64; 6],
    peak_timers: usize,
}

impl Timers {
    pub fn next(&self) -> Option<Instant> {
        self.heap.first().map(|entry| entry.0)
    }

    pub fn set(&mut self, slot: usize, deadline: Option<Instant>) {
        let previous = self.positions.get(slot)
            .filter(|&&index| index != usize::MAX)
            .map(|&index| self.heap[index].0);
        let category = match (previous, deadline) {
            (None, None) => 0,
            (Some(_), None) => 1,
            (None, Some(_)) => 2,
            (Some(old), Some(new)) if new == old => 3,
            (Some(old), Some(new)) if new < old => 4,
            (Some(_), Some(_)) => 5,
        };
        self.census[category] += 1;
        if let Some(deadline) = deadline
            && self
                .positions
                .get(slot)
                .is_some_and(|&index| index != usize::MAX && self.heap[index].0 == deadline)
        {
            return;
        }
        if self.positions.len() <= slot {
            self.positions.resize(slot + 1, usize::MAX);
        }
        self.remove(slot);
        if let Some(deadline) = deadline {
            let index = self.heap.len();
            self.heap.push((deadline, slot));
            self.peak_timers = self.peak_timers.max(self.heap.len());
            self.positions[slot] = index;
            self.up(index);
        }
    }

    pub fn pop_due(&mut self, now: Instant) -> Option<usize> {
        let &(deadline, slot) = self.heap.first()?;
        if deadline > now {
            return None;
        }
        self.remove(slot);
        Some(slot)
    }

    pub fn remove(&mut self, slot: usize) {
        let Some(&index) = self.positions.get(slot) else {
            return;
        };
        if index == usize::MAX {
            return;
        }
        self.positions[slot] = usize::MAX;
        self.heap.swap_remove(index);
        if index < self.heap.len() {
            self.positions[self.heap[index].1] = index;
            let index = self.up(index);
            self.down(index);
        }
    }

    pub fn rekey(&mut self, from: usize, to: usize) {
        let deadline = self
            .positions
            .get(from)
            .and_then(|&index| (index != usize::MAX).then(|| self.heap[index].0));
        self.remove(from);
        self.set(to, deadline);
    }

    pub fn shrink_to_fit(&mut self, slots: usize) {
        self.positions.truncate(slots);
        self.positions.shrink_to_fit();
        self.heap.shrink_to_fit();
    }

    fn swap(&mut self, a: usize, b: usize) {
        self.heap.swap(a, b);
        self.positions[self.heap[a].1] = a;
        self.positions[self.heap[b].1] = b;
    }

    fn up(&mut self, mut index: usize) -> usize {
        while index > 0 {
            let parent = (index - 1) / 2;
            if self.heap[parent] <= self.heap[index] {
                break;
            }
            self.swap(parent, index);
            index = parent;
        }
        index
    }

    fn down(&mut self, mut index: usize) {
        loop {
            let left = index * 2 + 1;
            if left >= self.heap.len() {
                break;
            }
            let right = left + 1;
            let child = if right < self.heap.len() && self.heap[right] < self.heap[left] {
                right
            } else {
                left
            };
            if self.heap[index] <= self.heap[child] {
                break;
            }
            self.swap(index, child);
            index = child;
        }
    }
}

impl Drop for Timers {
    fn drop(&mut self) {
        eprintln!("timer-attribution: {:?} peak={} remaining={}", self.census, self.peak_timers, self.heap.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn retiming_and_slot_reuse_preserve_order_without_stale_entries() {
        let now = Instant::now();
        let mut timers = Timers::default();
        let mut expected = std::collections::BTreeSet::new();
        for round in 0..8 {
            for slot in 0..127 {
                let offset = (slot * 31 + round * 71) % 257;
                let when = now + Duration::from_millis(offset as u64);
                expected.retain(|&(_, old)| old != slot);
                if (slot + round) % 7 == 0 {
                    timers.set(slot, None);
                    timers.set(slot, None);
                } else {
                    timers.set(slot, Some(when));
                    timers.set(slot, Some(when));
                    expected.insert((when, slot));
                }
            }
            assert_eq!(timers.heap.len(), expected.len());
        }
        for (when, slot) in expected {
            assert_eq!(timers.next(), Some(when));
            assert_eq!(timers.pop_due(when), Some(slot));
        }
        assert_eq!(timers.next(), None);
    }

    #[test]
    fn rekeying_sparse_slots_keeps_exact_deadlines_and_equal_deadline_order() {
        let now = Instant::now();
        let mut timers = Timers::default();
        for slot in 0..1024 {
            timers.set(slot, Some(now + Duration::from_millis((slot % 3) as u64)));
        }
        for slot in 1..1021 {
            timers.remove(slot);
        }
        timers.set(1022, None);
        timers.rekey(1023, 1);
        timers.rekey(1022, 2);
        timers.rekey(1021, 3);
        timers.shrink_to_fit(4);
        assert_eq!(timers.positions.len(), 4);
        assert!(timers.positions.capacity() < 1024);
        assert_eq!(timers.pop_due(now), Some(0));
        assert_eq!(timers.pop_due(now), Some(1));
        assert_eq!(timers.pop_due(now), None);
        assert_eq!(timers.pop_due(now + Duration::from_millis(1)), Some(3));
        assert_eq!(timers.pop_due(now + Duration::from_secs(1)), None);
        assert_eq!(timers.next(), None);
    }
}

#[cfg(test)]
mod census_tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn census_classifies_each_setter_transition_once() {
        let now = Instant::now();
        let mut timers = Timers::default();
        timers.set(0, None);
        timers.set(0, Some(now));
        timers.set(0, Some(now));
        timers.set(0, Some(now - Duration::from_secs(1)));
        timers.set(0, Some(now + Duration::from_secs(1)));
        timers.set(0, None);
        assert_eq!(timers.census, [1; 6]);
        assert_eq!(timers.peak_timers, 1);
        assert!(timers.heap.is_empty());
    }
}
