from pathlib import Path
base=Path('artifacts/rust-fanout-work')
p=base/'src/relay.rs';s=p.read_text()
s=s.replace('''        work: &mut RoomFanout,
    ) -> Result<bool, Disconnect>''','''        work: &mut RoomFanout,
        credit: &mut Option<InputCredit>,
    ) -> Result<bool, Disconnect>''')
s=s.replace('''        work.append(peer, frame)?;''','''        work.append(credit.take().expect("reserved buffered input").into_packet(frame));''')
a=s.index('    fn append(&mut self, source: &Peer, frame: Frame)');b=s.index('    pub(crate) fn take_deferred',a)
s=s[:a]+'''    fn append(&mut self, packet: Envelope) {
        debug_assert!(self.can_append());
        debug_assert!(self.length == 1 || (self.cursor == 0 && self.frame_cursor == 0));
        if self.length == 1 {
            self.delivered_prefix = self.cursor;
            self.cursor = 0;
        }
        self.extra[self.length - 1] = Some(packet);
        self.length += 1;
    }
    pub(crate) fn defer(&mut self, bytes: Bytes, credit: InputCredit) {
        debug_assert!(self.deferred.is_none());
        let charge = credit.into_packet(Frame::envelope(0, bytes.clone()));
        self.deferred = Some((bytes, charge));
    }
'''+s[b:]
at='impl Peer {';i=s.index(at)
s=s[:i]+s[i:].replace(at,'''impl Peer {
    /// Optional lookahead admission. Failure leaves input in the decoder and
    /// emits no overload intent: normal routing will retry after older work.
    pub(crate) fn reserve_input(&self, bytes: usize) -> Option<InputCredit> {
        if self.closing.load(Relaxed) { return None; }
        self.budget.used.fetch_update(Relaxed, Relaxed, |used|
            used.checked_add(bytes).filter(|&next| next <= self.budget.limit)).ok()?;
        if self.budget.metrics.queued_bytes.fetch_update(Relaxed, Relaxed, |used|
            used.checked_add(bytes).filter(|&next| next <= self.budget.global_limit)).is_err() {
            self.budget.used.fetch_sub(bytes, Relaxed);
            return None;
        }
        Some(InputCredit { budget: Some(self.budget.clone()), bytes })
    }
''',1)
at='''/// A charged synchronous submission borrows its owners.'''
i=s.index(at)
s=s[:i]+'''/// A bounded lookahead reservation, acquired before decoding. Ownership moves
/// into a normal envelope; abandoning lookahead releases every reserved byte.
pub(crate) struct InputCredit {
    budget: Option<Arc<Budget>>,
    bytes: usize,
}
impl InputCredit {
    fn into_packet(mut self, frame: Frame) -> Envelope {
        assert!(frame.len() <= self.bytes, "buffered input reservation covers routed frame");
        let budget = self.budget.take().expect("live input credit");
        let unused = self.bytes - frame.len();
        if unused != 0 {
            budget.used.fetch_sub(unused, Relaxed);
            budget.metrics.queued_bytes.fetch_sub(unused, Relaxed);
        }
        Envelope { frame, budget }
    }
}
impl Drop for InputCredit {
    fn drop(&mut self) {
        if let Some(budget) = &self.budget {
            budget.used.fetch_sub(self.bytes, Relaxed);
            budget.metrics.queued_bytes.fetch_sub(self.bytes, Relaxed);
        }
    }
}

'''+s[i:]
p.write_text(s)
p=base/'vendor/tungstenite/src/protocol/frame/mod.rs';s=p.read_text();a=s.index('    pub(super) fn buffered_binary_prefix(');b=s.index('    pub(super) fn read_storage_capacity',a)
s=s[:a]+'''    pub(super) fn buffered_binary_prefix(&self, prefix: &[u8], maximum: usize) -> Option<usize> {
        if self.header.is_some() { return None; }
        let mut cursor = Cursor::new(self.in_buffer.as_ref());
        let (header, length) = FrameHeader::parse(&mut cursor).ok()??;
        if !header.is_final || header.rsv1 || header.rsv2 || header.rsv3
            || header.opcode != crate::protocol::frame::coding::OpCode::Data(crate::protocol::frame::coding::Data::Binary)
            || length > maximum as u64 || length < prefix.len() as u64 { return None; }
        let mask = header.mask?;
        let start = cursor.position() as usize;
        let length = usize::try_from(length).ok()?;
        let end = start.checked_add(length)?;
        if end > self.in_buffer.len() || !prefix.iter().enumerate().all(|(i, byte)|
            self.in_buffer[start + i] ^ mask[i % 4] == *byte) { return None; }
        Some(length)
    }

'''+s[b:];p.write_text(s)
p=base/'vendor/tungstenite/src/protocol/mod.rs';s=p.read_text();a=s.index('    /// Whether a complete, unfragmented masked binary frame');b=s.index('    /// Change the configuration.',a)
s=s[:a]+'''    /// Payload length of a complete, unfragmented masked binary frame already
    /// buffered in this server decoder that starts with `prefix`. Never reads or
    /// consumes input; callers must still use the ordinary validated read.
    pub fn buffered_binary_prefix(&self, prefix: &[u8]) -> Option<usize> {
        if self.context.role != Role::Server || self.context.incomplete.is_some()
            || self.context.additional_send.is_some() || self.context.unflushed_additional
            || self.context.read_memory_failed || self.context.state != WebSocketState::Active {
            return None;
        }
        self.context.frame.buffered_binary_prefix(prefix,
            self.context.config.max_message_size.unwrap_or(usize::MAX)
                .min(self.context.config.max_frame_size.unwrap_or(usize::MAX)))
    }

'''+s[b:];p.write_text(s)
p=base/'src/server.rs';s=p.read_text();a=s.index('                                    while fanout.can_append()');b=s.index('                                        let bytes = match ws',a)
s=s[:a]+'''                                    while fanout.can_append() {
                                        let Some(length) = ws.buffered_binary_prefix(PREFIX) else { break; };
                                        let mut limiter = self.limiter.clone();
                                        if !limiter.allow(length, now) { break; }
                                        let mut credit = length.checked_add(2).and_then(|n| registration.peer.reserve_input(n));
                                        if credit.is_none() { break; }
'''+s[b:]
a=s.index('                                        if !self.limiter.allow(bytes.len(), now)');b=s.index('                                        let capacity =',a)
s=s[:a]+'''                                        self.limiter = limiter;
'''+s[b:]
s=s.replace('''                                            fanout,
                                        )''','''                                            fanout,
                                            &mut credit,
                                        )''')
a=s.index('''                                            Ok(false) => {''',s.index('while fanout.can_append()'));b=s.index('''                                                break;''',a)
s=s[:a]+'''                                            Ok(false) => {
                                                fanout.defer(bytes, credit.take().expect("reserved barrier input"));
'''+s[b:]
s=s.replace('struct RateLimiter {','#[derive(Clone)]\nstruct RateLimiter {')
p.write_text(s)
# Update direct reservation tests for explicit ownership transfer.
p=base/'src/relay/continuation_tests.rs';s=p.read_text()
s=s.replace('batch.try_append_room(&source.peer, packet, 0, work)', 'batch.try_append_room(&source.peer, packet, 0, work, &mut source.peer.reserve_input(packet.len() + 2))')
s=s.replace('batch.try_append_room(&source.peer, &packets[1], 0, work)', 'batch.try_append_room(&source.peer, &packets[1], 0, work, &mut None)')
s=s.replace('work.defer(&source.peer, packets[1].clone()).unwrap();','work.defer(packets[1].clone(), source.peer.reserve_input(packets[1].len() + 2).unwrap());')
a=s.index('    assert_eq!(\n        batch.try_append_room(&source.peer, &bytes, 0, work),');b=s.index('    assert!(work.active());',a)
s=s[:a]+'''    assert!(source.peer.reserve_input(bytes.len() + 2).is_none());
    assert!(!source.peer.closing.load(Relaxed));
    assert_eq!(relay.metrics.slow_consumers.load(Relaxed), 0);
'''+s[b:]
p.write_text(s)
p=base/'src/server/fairness_tests.rs';s=p.read_text()
s=s.replace('assert!(peek(input.clone()));','assert_eq!(peek(input.clone()), Some(body.len()));').replace('assert!(!peek(input[..length].to_vec()));','assert!(peek(input[..length].to_vec()).is_none());').replace('assert!(!peek(changed));','assert!(peek(changed).is_none());')
s=s.replace('assert!(ws.buffered_binary_prefix(PREFIX));','assert_eq!(ws.buffered_binary_prefix(PREFIX), Some(body.len()));').replace('assert!(!ws.buffered_binary_prefix(PREFIX));','assert!(ws.buffered_binary_prefix(PREFIX).is_none());')
p.write_text(s)
