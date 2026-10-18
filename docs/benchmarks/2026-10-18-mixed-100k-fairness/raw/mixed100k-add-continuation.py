from pathlib import Path
for path in [Path('src/relay.rs'), Path('artifacts/rust-fanout-work/src/relay.rs')]:
 s=path.read_text()
 s=s.replace('''            .receive_with_totals(peer, bytes, storage_capacity, &mut self.totals)
''','''            .receive_with_totals(peer, bytes, storage_capacity, &mut self.totals, deliver_room)
''',1)
 needle='''    pub(crate) fn publish(&mut self) {'''
 new='''    /// Native workers keep one reusable continuation slot per large publisher.
    pub(crate) fn receive_bounded(
        &mut self,
        peer: &Peer,
        bytes: Bytes,
        storage_capacity: usize,
        pending: &mut Option<Box<RoomFanout>>,
        quantum: usize,
    ) -> Result<usize, Disconnect> {
        self.relay.receive_with_totals(
            peer, bytes, storage_capacity, &mut self.totals,
            |source, recipients, frame| {
                if recipients.len() <= quantum {
                    return deliver_room(source, recipients, frame);
                }
                let work = pending.get_or_insert_with(|| Box::new(RoomFanout::default()));
                work.begin(source, recipients, frame, self.relay.config.write_timeout())?;
                work.resume(quantum)?;
                Ok(quantum)
            },
        )
    }

''';assert needle in s;s=s.replace(needle,new+needle,1)
 s=s.replace('''    fn receive_with_totals(
''','''    fn receive_with_totals<F>(
''',1)
 needle='''        totals: &mut InputTotals,
    ) -> Result<usize, Disconnect> {'''
 new='''        totals: &mut InputTotals,
        room: F,
    ) -> Result<usize, Disconnect>
    where F: FnOnce(&Peer, Arc<Vec<Arc<Peer>>>, Frame) -> Result<usize, Disconnect>,
    {''';assert needle in s;s=s.replace(needle,new,1)
 needle='''            let work = recipients.len().max(1);
            drop(state);
            for recipient in recipients.iter() {
                if recipient.key != peer.key {
                    recipient.enqueue_fanout(&frame)?;
                }
            }
            return Ok(work);'''
 new='''            drop(state);
            return room(peer, recipients, frame);''';assert needle in s;s=s.replace(needle,new,1)
 # Reuse exactly the existing byte reservation contract for staged work.
 needle='''        if self.closing.load(Relaxed) {
            return Ok(());
        }
        let size = frame.len();'''
 new='''        let Some(packet) = self.reserve(frame)? else { return Ok(()); };
        let accepted = if FANOUT {
            self.output.send_fanout(packet)
        } else {
            self.output.send(packet)
        };
        if !accepted { self.overloaded(); }
        Ok(())
    }

    #[inline(always)]
    fn reserve<'a>(&'a self, frame: &'a Frame) -> Result<Option<Submission<'a>>, Disconnect> {
        if self.closing.load(Relaxed) { return Ok(None); }
        let size = frame.len();''';assert needle in s;s=s.replace(needle,new,1)
 # Only first overloaded branch belongs to reserve.
 start=s.index('    fn reserve<');end=s.index('    fn overloaded(',start)
 c=s[start:end].replace('''            return Ok(());''','''            return Ok(None);''',1)
 a=c.index('''        let packet = Submission {''')
 c=c[:a]+'''        Ok(Some(Submission { frame, budget: Some(&self.budget) }))
    }

''';s=s[:start]+c+s[end:]
 needle='''struct Budget {'''
 new='''/// One immutable membership snapshot and one charged shared body. The box is
/// allocated only on a publisher's first oversized room multicast and reused.
/// Empty slots retain no payload or peer ownership; metadata is bounded by the
/// admitted connection count and staged bodies share the normal queue budget.
#[derive(Default)]
pub(crate) struct RoomFanout {
    recipients: Option<Arc<Vec<Arc<Peer>>>>,
    packet: Option<Envelope>,
    cursor: usize,
    source: u64,
    deadline: Option<std::time::Instant>,
}

impl RoomFanout {
    fn begin(&mut self, source: &Peer, recipients: Arc<Vec<Arc<Peer>>>, frame: Frame, timeout: std::time::Duration) -> Result<(), Disconnect> {
        debug_assert!(!self.active());
        let packet = source.reserve(&frame)?.ok_or(Disconnect::OVERLOADED)?.into_owned();
        self.packet = Some(packet);
        self.recipients = Some(recipients);
        self.cursor = 0;
        self.source = source.key;
        self.deadline = Some(std::time::Instant::now() + timeout);
        Ok(())
    }

    pub(crate) fn active(&self) -> bool { self.packet.is_some() }
    pub(crate) fn deadline(&self) -> Option<std::time::Instant> { self.deadline }
    pub(crate) fn clear(&mut self) {
        self.packet = None;
        self.recipients = None;
        self.deadline = None;
    }

    pub(crate) fn resume(&mut self, quantum: usize) -> Result<(), Disconnect> {
        let recipients = self.recipients.as_ref().expect("live fanout snapshot");
        let frame = &self.packet.as_ref().expect("charged fanout body").frame;
        let end = self.cursor.saturating_add(quantum).min(recipients.len());
        for recipient in &recipients[self.cursor..end] {
            if recipient.key != self.source {
                recipient.enqueue_fanout(frame)?;
            }
        }
        self.cursor = end;
        if end == recipients.len() { self.clear(); }
        Ok(())
    }
}

fn deliver_room(source: &Peer, recipients: Arc<Vec<Arc<Peer>>>, frame: Frame) -> Result<usize, Disconnect> {
    let work = recipients.len().max(1);
    for recipient in recipients.iter() {
        if recipient.key != source.key { recipient.enqueue_fanout(&frame)?; }
    }
    Ok(work)
}

''';assert needle in s;s=s.replace(needle,new+needle,1);path.write_text(s)
p=Path('src/server.rs');s=p.read_text().replace('relay::{ReceiveBatch, Registration}','relay::{ReceiveBatch, Registration, RoomFanout}')
s=s.replace('    output_only: bool,','    output_only: bool,\n    room_fanout: Option<Box<RoomFanout>>,',1).replace('            output_only: false,','            output_only: false,\n            room_fanout: None,',1)
s=s.replace('        let mut deadline = self.socket.deadline();','''        let mut deadline = self.socket.deadline();
        if let Some(next) = self.room_fanout.as_ref().and_then(|f| f.deadline()) {
            deadline = Some(deadline.map_or(next, |d| d.min(next)));
        }''',1)
s=s.replace('''        if self.output_only
            && !stopping''','''        if self.output_only
            && !self.room_fanout.as_ref().is_some_and(|f| f.active())
            && !stopping''',1)
s=s.replace('''        if self.closing.is_some() {
            return Drive::Wait;
        }
        if reason.code''','''        if self.closing.is_some() {
            return Drive::Wait;
        }
        if let Some(fanout) = &mut self.room_fanout { fanout.clear(); }
        if reason.code''',1)
s=s.replace('''            if self.socket.deadline().is_some_and(|d| d <= now) {''','''            if self.socket.deadline().is_some_and(|d| d <= now)
                || self.room_fanout.as_ref().and_then(|f| f.deadline()).is_some_and(|d| d <= now)
            {''',1)
s=s.replace('''        let mut route_budget = READ_QUANTUM;''','''        if let Some(fanout) = self.room_fanout.as_mut().filter(|f| f.active()) {
            return match fanout.resume(READ_QUANTUM) {
                Ok(()) => Drive::Again,
                Err(reason) => self.start_close(reason, relay, false, now),
            };
        }
        let mut route_budget = READ_QUANTUM;''',1)
s=s.replace('''match incoming.receive(&registration.peer, bytes, capacity)''','''match incoming.receive_bounded(&registration.peer, bytes, capacity, &mut self.room_fanout, READ_QUANTUM)''',1)
s=s.replace('''        self.socket.end_output();
        self.registration.take();''','''        self.socket.end_output();
        self.room_fanout.take();
        self.registration.take();''',1)
p.write_text(s);Path('artifacts/rust-fanout-work/src/server.rs').write_text(s)
