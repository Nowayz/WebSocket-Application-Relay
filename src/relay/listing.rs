use base64::{Engine, engine::general_purpose::STANDARD};
use std::num::NonZeroUsize;

use super::{ChannelSnapshot, MAX_CHANNEL_PAGE_SIZE, Relay};

impl Relay {
    /// Select bounded batches from a lock-free walk of the index: a channel
    /// present throughout the walk is seen once, one created or retired
    /// meanwhile may or may not appear. Each partition discards rows that
    /// cannot enter the smallest page. Scratch space holds at most two pages
    /// of keys and owned metadata, never peer/value bodies.
    pub(crate) fn channel_page(
        &self,
        limit: NonZeroUsize,
        after: Option<[u8; 24]>,
    ) -> Vec<ChannelSnapshot> {
        let limit = limit.get().min(MAX_CHANNEL_PAGE_SIZE);
        let channels = self.live_channels();
        let capacity = (2 * limit).min(channels.len());
        let mut selected = Vec::new();
        for channel in &channels {
            // Cursor filtering needs only the borrowed name's sort key.
            // Construct owned row metadata only after that filter succeeds.
            let mut key = [0; 24];
            STANDARD
                .encode_slice(&channel.name, &mut key)
                .expect("16-byte channel names fit in 24 base64 bytes");
            if after.is_some_and(|after| key <= after) {
                continue;
            }
            if selected.is_empty() {
                selected.reserve_exact(capacity);
            }
            selected.push((key, ChannelSnapshot::of(channel)));
            // Keep one page of candidates and make room for the next batch.
            if selected.len() == 2 * limit {
                selected.select_nth_unstable_by_key(limit, |row| row.0);
                selected.truncate(limit);
            }
        }
        // Ordering and output conversion need no registry guard.
        selected.sort_unstable_by_key(|row| row.0);
        selected.truncate(limit);
        selected.into_iter().map(|(_, snapshot)| snapshot).collect()
    }
}
