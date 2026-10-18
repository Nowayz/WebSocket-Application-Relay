use std::{collections::BinaryHeap, num::NonZeroUsize};

use super::{ChannelSnapshot, MAX_CHANNEL_PAGE_SIZE, Relay, SnapshotName};

impl Relay {
    /// Keep only the smallest qualifying keys while scanning one coherent view.
    /// Capacity depends on the page, not the registry population. Names and
    /// numeric metadata are copied; peers and variable bodies stay in place.
    pub(crate) fn channel_page(
        &self,
        limit: NonZeroUsize,
        after: Option<[u8; 24]>,
    ) -> Vec<ChannelSnapshot> {
        let limit = limit.get().min(MAX_CHANNEL_PAGE_SIZE);
        let waiting = std::time::Instant::now();
        let state = self.state.read();
        let acquired = std::time::Instant::now();
        let population = state.channels.len();
        let capacity = limit.min(state.channels.len());
        let mut selected = BinaryHeap::<([u8; 24], usize)>::new();
        let mut snapshots = Vec::new();
        for (name, channel) in &state.channels {
            let name = SnapshotName::new(name);
            let key = name.base64();
            if after.is_some_and(|after| key <= after) {
                continue;
            }
            let snapshot = || ChannelSnapshot {
                name,
                members: channel.members.len(),
                variables: channel.variables.len(),
                variable_bytes: channel.variable_bytes,
            };
            if selected.len() < limit {
                // A cursor past the last channel needs no page storage.
                if selected.is_empty() {
                    selected.reserve_exact(capacity);
                    snapshots.reserve_exact(capacity);
                }
                selected.push((key, snapshots.len()));
                snapshots.push(snapshot());
            } else if let Some(mut largest) = selected.peek_mut()
                && key < largest.0
            {
                snapshots[largest.1] = snapshot();
                largest.0 = key;
            }
        }
        drop(state);
        let released = std::time::Instant::now();
        if after.is_none() {
            eprintln!("PAGE_LOCK {} {} {}", population, acquired.duration_since(waiting).as_nanos(), released.duration_since(acquired).as_nanos());
        }
        // Small owned rows can be copied into output order after unlocking.
        // Both temporary vectors retire before JSON serialization starts.
        selected
            .into_sorted_vec()
            .into_iter()
            .map(|(_, slot)| snapshots[slot])
            .collect()
    }
}
