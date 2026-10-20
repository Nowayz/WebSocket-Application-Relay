//! Large observer publications are owned and advanced by their source worker.
//! The index is a lock-free map used only for registration, retirement and
//! revocation; worker turns lock one publication, never any shared structure,
//! and direct (unqueued) deliveries never touch the index at all: each
//! observer re-checks the revocation mark under its own output lock.
use super::*;

type ObserverView = Vec<Arc<Peer>>;

pub(super) struct MessageFanouts {
    /// Numbers publications; a barrier inspects them in ascending order.
    sequence: AtomicU64,
    /// Live large observer publications by number. Registration, retirement
    /// and a barrier's walk never wait on each other. Pre-sized so that it
    /// does not resize in ordinary operation; a walk would otherwise help
    /// finish an in-progress resize first.
    jobs: papaya::HashMap<u64, Weak<Mutex<RoomFanout>>>,
}

impl Default for MessageFanouts {
    fn default() -> Self {
        Self {
            sequence: AtomicU64::new(0),
            jobs: papaya::HashMap::with_capacity(4096),
        }
    }
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
        generation: u64,
    ) -> Result<(), Disconnect> {
        debug_assert!(!self.active());
        // The recorded generation covers a capture that raced a revocation
        // and registers only after its scan: the job then skips the observer.
        let sequence = self
            .registry
            .sequence
            .fetch_update(Relaxed, Relaxed, |sequence| sequence.checked_add(1))
            .map_err(|_| Disconnect::OVERLOADED)?
            + 1;
        let mut work = self.work.lock();
        work.begin(source, FanoutRecipients::Message(recipients), frame, 0)?;
        work.generation = generation;
        drop(work);
        self.registry
            .jobs
            .pin()
            .insert(sequence, Arc::downgrade(&self.work));
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
            self.registry.jobs.pin().remove(&sequence);
            self.sequence = None;
        }
        result
    }

    pub(super) fn clear(&mut self) {
        self.work.lock().clear();
        if let Some(sequence) = self.sequence.take() {
            self.registry.jobs.pin().remove(&sequence);
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
    /// watermarks. The caller submits them after releasing every lock. The
    /// revoked generation is stored before the caller enqueues the
    /// acknowledgement, and direct deliveries re-check it under the
    /// observer's own output lock, so none of them can follow the ACK.
    pub(super) fn flush(&self, observer: &Peer, revoked_at: u64) -> Vec<Frame> {
        let mut pending = Vec::new();
        let previous = observer.message_barrier.load(Relaxed);
        // The map is lock-free and unordered: collect, then inspect in ascending
        // sequence. That order is essential, since a watermark must never
        // suppress a lower-sequence job that this barrier has not inspected
        // yet. A job registered during the walk may be missed; its generation
        // then makes it skip the observer, as a capture concurrent with the
        // revocation may.
        let mut jobs: Vec<(u64, Weak<Mutex<RoomFanout>>)> = self
            .jobs
            .pin()
            .iter()
            .filter(|(sequence, _)| **sequence > previous)
            .map(|(sequence, job)| (*sequence, job.clone()))
            .collect();
        jobs.sort_unstable_by_key(|(sequence, _)| *sequence);
        let mut searched: Option<(Weak<ObserverView>, Option<usize>)> = None;
        for (sequence, weak) in jobs {
            let Some(job) = weak.upgrade() else {
                continue;
            };
            let work = job.lock();
            // Captured after the revocation: the observer is not in its view.
            if work.generation > revoked_at {
                continue;
            }
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
        observer
            .message_barrier
            .store(self.sequence.load(Relaxed), Relaxed);
        // A publication captured before the revocation but registered after
        // this walk skips the observer instead of arriving after its ACK.
        observer.message_revoked.store(revoked_at, Relaxed);
        pending
    }

    #[cfg(test)]
    pub(super) fn jobs_len(&self) -> usize {
        self.jobs.pin().len()
    }

    #[cfg(test)]
    pub(super) fn set_sequence(&self, sequence: u64) {
        self.sequence.store(sequence, Relaxed);
    }
}

#[cfg(test)]
mod tests;
