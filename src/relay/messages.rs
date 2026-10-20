//! Large observer publications are owned and advanced by their source worker.
//! The index is used only for registration, retirement and revocation; worker
//! turns lock one publication, never the index or membership registry.
use super::*;
use std::collections::BTreeMap;

type ObserverView = Vec<Arc<Peer>>;

#[derive(Default)]
pub(super) struct MessageFanouts {
    index: Mutex<Index>,
}

#[derive(Default)]
struct Index {
    sequence: u64,
    jobs: BTreeMap<u64, Weak<Mutex<RoomFanout>>>,
}

pub(super) struct Publication {
    registry: Arc<MessageFanouts>,
    work: Arc<Mutex<RoomFanout>>,
    sequence: Option<u64>,
}

impl Publication {
    pub(super) fn new(registry: Arc<MessageFanouts>) -> Self {
        Self {
            registry,
            work: Arc::new(Mutex::new(RoomFanout::default())),
            sequence: None,
        }
    }

    pub(super) fn begin(
        &mut self,
        source: &Peer,
        recipients: MessageRecipients,
        frame: Frame,
        timeout: std::time::Duration,
    ) -> Result<(), Disconnect> {
        debug_assert!(!self.active());
        // The caller retains its registry read guard through registration.
        // Revocation's write guard therefore observes every older capture.
        let mut index = self.registry.index.lock();
        let sequence = index
            .sequence
            .checked_add(1)
            .ok_or(Disconnect::OVERLOADED)?;
        let mut work = self.work.lock();
        work.begin(
            source,
            FanoutRecipients::Message(recipients),
            frame,
            timeout,
            0,
        )?;
        index.sequence = sequence;
        index.jobs.insert(sequence, Arc::downgrade(&self.work));
        self.sequence = Some(sequence);
        Ok(())
    }

    pub(super) fn active(&self) -> bool {
        self.sequence.is_some()
    }

    pub(super) fn deadline(&self) -> Option<std::time::Instant> {
        self.sequence.and_then(|_| self.work.lock().deadline)
    }

    pub(super) fn resume(&mut self, quantum: usize) -> Result<(), Disconnect> {
        let Some(sequence) = self.sequence else {
            return Ok(());
        };
        let mut work = self.work.lock();
        let result = work.resume_with::<false>(quantum, Some(sequence));
        if result.is_err() {
            work.clear();
        }
        let finished = !work.active();
        drop(work);
        if finished {
            // Always release the job lock before taking the index lock.
            self.registry.index.lock().jobs.remove(&sequence);
            self.sequence = None;
        }
        result
    }

    pub(super) fn clear(&mut self) {
        self.work.lock().clear();
        if let Some(sequence) = self.sequence.take() {
            self.registry.index.lock().jobs.remove(&sequence);
        }
    }
}

impl Drop for Publication {
    fn drop(&mut self) {
        self.clear();
    }
}

impl MessageFanouts {
    pub(super) fn flush(&self, observer: &Peer) {
        // Caller holds registry write. Publication takes registry -> index ->
        // job; progress takes only job, then releases it before unregistering.
        let index = self.index.lock();
        let previous = observer.message_barrier.load(Relaxed);
        let mut searched: Option<(Weak<ObserverView>, Option<usize>)> = None;
        // Ascending sequence is essential: a watermark must never suppress a
        // lower-sequence job that this barrier has not inspected yet.
        for (&sequence, weak) in index.jobs.range((
            std::ops::Bound::Excluded(previous),
            std::ops::Bound::Unbounded,
        )) {
            let Some(job) = weak.upgrade() else {
                continue;
            };
            let work = job.lock();
            if let (Some(FanoutRecipients::Message(message)), Some(packet)) =
                (&work.recipients, &work.packet)
            {
                let view = &message.observers;
                let position = match &searched {
                    Some((old, position)) if old.as_ptr() == Arc::as_ptr(view) => *position,
                    _ => {
                        let position = view.iter().position(|peer| std::ptr::eq(&**peer, observer));
                        // A weak identity avoids keeping the handle buffer alive
                        // after a completing job has returned its queue credit.
                        searched = Some((Arc::downgrade(view), position));
                        position
                    }
                };
                if position.is_some_and(|position| {
                    message.room().len().saturating_add(position) >= work.cursor
                }) && observer.key != message.excluded_observer
                {
                    let _ = observer.enqueue_barrier(&packet.frame);
                }
            }
            // Store while this job is locked: its source cannot replay the
            // just-submitted message before the watermark becomes visible.
            observer.message_barrier.store(sequence, Relaxed);
        }
        observer.message_barrier.store(index.sequence, Relaxed);
    }
}

#[cfg(test)]
mod tests;
