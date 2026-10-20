//! Large observer publications are owned and advanced by their source worker.
//! The index is used only for registration, retirement and revocation; worker
//! turns lock one publication, never the index or any membership lock.
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
    ) -> Result<(), Disconnect> {
        debug_assert!(!self.active());
        // The caller holds the listener read lock through registration, so a
        // revocation's write lock observes every older capture.
        let mut index = self.registry.index.lock();
        let sequence = index
            .sequence
            .checked_add(1)
            .ok_or(Disconnect::OVERLOADED)?;
        let mut work = self.work.lock();
        work.begin(source, FanoutRecipients::Message(recipients), frame, 0)?;
        index.sequence = sequence;
        index.jobs.insert(sequence, Arc::downgrade(&self.work));
        self.sequence = Some(sequence);
        Ok(())
    }

    pub(super) fn active(&self) -> bool {
        self.sequence.is_some()
    }

    pub(super) fn detach(&mut self) {
        self.work.lock().tolerant = true;
    }

    pub(super) fn resume(&mut self, quantum: usize) -> Result<(), RoutingFailure> {
        let Some(sequence) = self.sequence else {
            return Ok(());
        };
        let mut work = self.work.lock();
        let result = work.resume_with(quantum, Some(sequence));
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
    /// Collect the observer's pending captured messages and advance its
    /// watermark. The caller submits them after releasing every lock.
    pub(super) fn flush(&self, observer: &Peer) -> Vec<Frame> {
        // Caller holds the listener write lock. Publication takes listeners ->
        // index -> job; progress takes only job, then releases it before
        // unregistering.
        let mut pending = Vec::new();
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
                    pending.push(packet.frame.clone());
                }
            }
            // Store while this job is locked: its source cannot deliver this
            // message to the observer once the watermark is visible.
            observer.message_barrier.store(sequence, Relaxed);
        }
        observer.message_barrier.store(index.sequence, Relaxed);
        pending
    }
}

#[cfg(test)]
mod tests;
