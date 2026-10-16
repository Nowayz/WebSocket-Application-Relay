//! One timer per live connection. Updating or cancelling never leaves stale
//! entries behind, so repeated traffic cannot grow the timer queue.
use std::time::Instant;

#[derive(Default)]
pub(super) struct Timers {
    heap: Vec<(Instant, usize)>,
    positions: Vec<usize>,
}

impl Timers {
    pub fn next(&self) -> Option<Instant> {
        self.heap.first().map(|entry| entry.0)
    }

    pub fn set(&mut self, slot: usize, deadline: Option<Instant>) {
        if self.positions.len() <= slot {
            self.positions.resize(slot + 1, usize::MAX);
        }
        self.remove(slot);
        if let Some(deadline) = deadline {
            let index = self.heap.len();
            self.heap.push((deadline, slot));
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
                } else {
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
}
