from pathlib import Path
p=Path('src/relay.rs');s=p.read_text();mark='''/// A transport owns ordering and backpressure.'''
addition='''/// A charged synchronous submission borrows its owners. Only deferred output
/// promotes it to an owned envelope; either form releases the same reservation.
pub(crate) struct Submission<'a> {
    pub frame: &'a Frame,
    budget: Option<&'a Arc<Budget>>,
}

impl Submission<'_> {
    pub(crate) fn into_owned(mut self) -> Envelope {
        let budget = self.budget.take().expect("live submission").clone();
        Envelope {
            frame: self.frame.clone(),
            budget,
        }
    }
}

impl Drop for Submission<'_> {
    fn drop(&mut self) {
        if let Some(budget) = self.budget {
            budget.release(self.frame.len());
        }
    }
}

impl Budget {
    fn release(&self, bytes: usize) {
        self.used.fetch_sub(bytes, Relaxed);
        self.metrics.queued_bytes.fetch_sub(bytes, Relaxed);
    }
}

'''
assert mark in s;s=s.replace(mark,addition+mark)
s=s.replace('fn send(&self, packet: Envelope) -> bool','fn send(&self, packet: Submission<\'_>) -> bool')
s=s.replace('self.outgoing.try_send(packet).is_ok()','self.outgoing.try_send(packet.into_owned()).is_ok()')
s=s.replace('''        self.budget.used.fetch_sub(self.frame.len(), Relaxed);
        self.budget
            .metrics
            .queued_bytes
            .fetch_sub(self.frame.len(), Relaxed);''','''        self.budget.release(self.frame.len());''')
s=s.replace('''        let packet = Envelope {
            frame: frame.clone(),
            budget: self.budget.clone(),
        };''','''        let packet = Submission {
            frame,
            budget: Some(&self.budget),
        };''')
p.write_text(s)
p=Path('src/server/socket.rs');s=p.read_text().replace('relay::{Envelope, PeerOutput}','relay::{Envelope, PeerOutput, Submission}')
s=s.replace('fn delivered(&self, packet: &Envelope, state: &mut State)','fn delivered(&self, frame: &Frame, state: &mut State)').replace('packet.frame.len() as u64','frame.len() as u64')
s=s.replace('self.delivered(&pending.packet, &mut state)','self.delivered(&pending.packet.frame, &mut state)')
s=s.replace('fn send(&self, packet: Envelope) -> bool','fn send(&self, packet: Submission<\'_>) -> bool')
s=s.replace('''                &packet.frame,
                &mut offset,''','''                packet.frame,
                &mut offset,''').replace('wire_len(&packet.frame)','wire_len(packet.frame)').replace('self.delivered(&packet, &mut state)','self.delivered(packet.frame, &mut state)')
s=s.replace('''        state.queue.push_back(Pending {
            packet,
''','''        state.queue.push_back(Pending {
            packet: packet.into_owned(),
''');p.write_text(s)
p=Path('src/server/fairness_tests.rs');s=p.read_text().replace('{Envelope, PeerOutput}','{PeerOutput, Submission}').replace('packet: Envelope','packet: Submission<\'_>');p.write_text(s)
p=Path('src/relay/broadcast_tests.rs');s=p.read_text().replace('packet: Envelope','packet: Submission<\'_>').replace('_: Envelope','_: Submission<\'_>').replace('self.received.lock().replace(packet)','self.received.lock().replace(packet.into_owned())');p.write_text(s)
