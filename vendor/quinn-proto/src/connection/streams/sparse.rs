//! Preserve the logical stream map without allocating a slot for every unused
//! remote stream credit. Only mutable access materializes an empty stream slot.
use std::collections::hash_map::Entry;

use rustc_hash::FxHashMap;

use crate::{Side, StreamId, range_set::ArrayRangeSet};

pub(super) struct SparseMap<T> {
    allocated: FxHashMap<StreamId, Option<T>>,
    remote: Side,
    unopened: [ArrayRangeSet; 2],
    empty: Option<T>,
}

impl<T> SparseMap<T> {
    pub(super) fn new(remote: Side) -> Self {
        Self {
            allocated: FxHashMap::default(),
            remote,
            unopened: std::array::from_fn(|_| ArrayRangeSet::new()),
            empty: None,
        }
    }

    fn deferred(&self, id: StreamId) -> bool {
        id.initiator() == self.remote && self.unopened[id.dir() as usize].contains(id.index())
    }

    fn take_deferred(&mut self, id: StreamId) -> bool {
        id.initiator() == self.remote
            && self.unopened[id.dir() as usize].remove(id.index()..id.index() + 1)
    }

    pub(super) fn contains_key(&self, id: &StreamId) -> bool {
        self.allocated.contains_key(id) || self.deferred(*id)
    }

    pub(super) fn get(&self, id: &StreamId) -> Option<&Option<T>> {
        self.allocated
            .get(id)
            .or_else(|| self.deferred(*id).then_some(&self.empty))
    }

    pub(super) fn get_mut(&mut self, id: &StreamId) -> Option<&mut Option<T>> {
        match self.entry(*id) {
            Entry::Occupied(entry) => Some(entry.into_mut()),
            Entry::Vacant(_) => None,
        }
    }

    pub(super) fn entry(&mut self, id: StreamId) -> Entry<'_, StreamId, Option<T>> {
        // Active streams stay on the ordinary one-hash lookup path. Only an
        // absent slot needs to consult and split unused remote credit ranges.
        match self.allocated.entry(id) {
            Entry::Vacant(entry)
                if id.initiator() == self.remote
                    && self.unopened[id.dir() as usize].remove(id.index()..id.index() + 1) =>
            {
                Entry::Occupied(entry.insert_entry(None))
            }
            entry => entry,
        }
    }

    pub(super) fn remove(&mut self, id: &StreamId) -> Option<Option<T>> {
        if self.take_deferred(*id) {
            Some(None)
        } else {
            self.allocated.remove(id)
        }
    }

    pub(super) fn insert(&mut self, id: StreamId, value: Option<T>) -> Option<Option<T>> {
        let old = self.remove(&id);
        if id.initiator() == self.remote && value.is_none() {
            self.unopened[id.dir() as usize].insert_one(id.index());
        } else {
            self.allocated.insert(id, value);
        }
        old
    }

    // Empty logical placeholders have no fields to update. This intentionally
    // exposes only allocated entries to transport-parameter updates.
    pub(super) fn allocated_mut(&mut self) -> impl Iterator<Item = (&StreamId, &mut Option<T>)> {
        self.allocated.iter_mut()
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.allocated.len()
            + self
                .unopened
                .iter()
                .flat_map(|ranges| ranges.iter())
                .map(|r| (r.end - r.start) as usize)
                .sum::<usize>()
    }
}

impl<T> std::ops::Index<&StreamId> for SparseMap<T> {
    type Output = Option<T>;

    fn index(&self, id: &StreamId) -> &Self::Output {
        self.get(id).expect("stream is present")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dir;
    use std::collections::HashMap;

    #[test]
    fn remote_credit_is_logically_present_without_hash_slots() {
        let mut map = SparseMap::<u64>::new(Side::Client);
        for index in 0..1000 {
            for dir in [Dir::Bi, Dir::Uni] {
                assert_eq!(
                    map.insert(StreamId::new(Side::Client, dir, index), None),
                    None
                );
            }
        }
        assert_eq!(map.len(), 2000);
        assert_eq!(map.allocated.capacity(), 0);
        assert_eq!(map.unopened[0].len(), 1);
        assert_eq!(map.unopened[1].len(), 1);
        let id = StreamId::new(Side::Client, Dir::Bi, 500);
        assert_eq!(map.get(&id), Some(&None));
        *map.get_mut(&id).unwrap() = Some(42);
        assert_eq!(map.get(&id), Some(&Some(42)));
        assert_eq!(map.remove(&id), Some(Some(42)));
        assert!(!map.contains_key(&id));
        assert_eq!(map.len(), 1999);
    }

    #[test]
    fn mixed_operations_match_independent_hash_map() {
        let mut map = SparseMap::<u64>::new(Side::Client);
        let mut reference = HashMap::new();
        let mut seed = 0x82c67b491e3a620du64;
        for turn in 0..20000 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let id = StreamId::new(
                if seed & 1 == 0 {
                    Side::Client
                } else {
                    Side::Server
                },
                if seed & 2 == 0 { Dir::Bi } else { Dir::Uni },
                (seed >> 8) % 128,
            );
            match (seed >> 3) % 5 {
                0 => {
                    let value = (seed & 4 != 0).then_some(turn);
                    assert_eq!(map.insert(id, value), reference.insert(id, value));
                }
                1 => {
                    assert_eq!(map.remove(&id), reference.remove(&id));
                }
                2 => {
                    if let Some(slot) = map.get_mut(&id) {
                        *slot = Some(turn);
                    }
                    if let Some(slot) = reference.get_mut(&id) {
                        *slot = Some(turn);
                    }
                }
                3 => {
                    *map.entry(id).or_insert(None) = Some(turn);
                    *reference.entry(id).or_insert(None) = Some(turn);
                }
                _ => {
                    assert_eq!(map.contains_key(&id), reference.contains_key(&id));
                }
            }
            assert_eq!(map.get(&id), reference.get(&id));
            assert_eq!(map.len(), reference.len());
        }
        for (id, value) in &reference {
            assert_eq!(map.get(id), Some(value));
        }
    }
}
