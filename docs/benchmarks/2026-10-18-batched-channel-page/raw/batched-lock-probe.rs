use std::num::NonZeroUsize;

use super::{ChannelSnapshot, MAX_CHANNEL_PAGE_SIZE, Relay, SnapshotName};

impl Relay {
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
        let capacity = (2 * limit).min(state.channels.len());
        let mut selected = Vec::new();
        for (name, channel) in &state.channels {
            let name = SnapshotName::new(name);
            let key = name.base64();
            if after.is_some_and(|after| key <= after) {
                continue;
            }
            if selected.is_empty() {
                selected.reserve_exact(capacity);
            }
            selected.push((key, ChannelSnapshot {
                name,
                members: channel.members.len(),
                variables: channel.variables.len(),
                variable_bytes: channel.variable_bytes,
            }));
            if selected.len() == 2 * limit {
                selected.select_nth_unstable_by_key(limit, |row| row.0);
                selected.truncate(limit);
            }
        }
        drop(state);
        let released = std::time::Instant::now();
        if after.is_none() {
            eprintln!("PAGE_LOCK {} {} {}", population, acquired.duration_since(waiting).as_nanos(), released.duration_since(acquired).as_nanos());
        }
        selected.sort_unstable_by_key(|row| row.0);
        selected.truncate(limit);
        selected.into_iter().map(|(_, snapshot)| snapshot).collect()
    }
}
