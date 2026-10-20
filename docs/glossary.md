# Glossary

Terms used across the relay documentation, in the order they usually come up.
The implementation is in [src/relay.rs](../src/relay.rs) and
[src/server.rs](../src/server.rs); the design is in
[architecture.md](architecture.md).

**Session.** One joined WebSocket connection. A session has a 64-bit ID, one
channel for its whole life, an output budget and one FIFO of outgoing frames.
The relay publishes the session to its channel only after the welcome frame
carrying its ID has been written.

**Channel, room.** A named group of sessions. Names are 0 to 16 bytes. The two
words mean the same thing; "room" is used when talking about membership and
delivery, "channel" when talking about names and variables. An ordinary channel
is created by its first member and retired with its last member. Each channel
has its own lock and its own departure queue.

**re_globl.** The persistent global channel. It is never retired. A broadcast
from a `re_globl` member reaches every session in every room. Authenticated
`re_globl` members can also become observers.

**Listener, observer.** An authenticated `re_globl` member that has set one or
more listener flags: channel messages (1), private messages (2) or disconnects
(4). Observers receive copies of the selected traffic from every room. They are
an administrative facility, not an authorization mechanism.

**Departure notice.** The frame `[18446744073709551615, departed_id]` that the
relay sends to the remaining members of a room when a session leaves, and to
disconnect observers. A session's departure notice follows that session's last
messages in each recipient's FIFO.

**Departure queue.** A FIFO of bounded jobs for one recipient population. Each
room has one. `re_globl` room departures and disconnect-observer notices share
one. Workers advance queues in rotation, one turn at a time, so a large job in
one room cannot delay another room's notices.

**Detached fanout.** A broadcast whose source connection closed before the
broadcast reached every recipient in its captured view. The unfinished job is
handed to a departure queue and completed there. For an ordinary room it is
completed before the source's own departure notice. Refusals during a detached
job are ignored because the source is gone.

**Captured view.** An immutable snapshot of a recipient population, taken when
a broadcast or departure starts. The relay routes against the view, not the
live membership, so membership changes during a long delivery do not change
who receives that message. A room caches one view and rebuilds it lazily after
membership changes.

**Staging credit.** The portion of the global output budget reserved for a
captured view and its job metadata while a deferred delivery is in progress.
Staging credit is reserved strictly: if it cannot be obtained, the relay
delivers synchronously instead and counts the refusal in
`war_global_pressure_total`.

**Per-session allowance.** The output bytes (`WAR_QUEUE_BYTES`) and queued
frames (`WAR_QUEUE_MESSAGES`) one session may hold. Exhausting either closes
that session with `1013`; it is the slow consumer.

**Global budget.** The process-wide output byte budget, `WAR_TOTAL_QUEUE_BYTES`.
Every charged frame, body and captured view counts against it; the gauge is
`war_queued_bytes`. Exhausting it drops the frame for the recipient being
charged and never closes the sender.

**Barrier, watermark.** The mechanism that orders an observer's pending notices
before the acknowledgement of a listener-flag change. Each departure queue has
a sequence number; each session records the sequence up to which its notices
have been flushed. A barrier submits the observer's still-pending notices and
advances the watermark, so deferred jobs do not replay them later and
re-subscribing does not deliver them twice.

**Turn, quantum.** The bounded unit of worker progress. A live broadcast
advances 1,024 recipients per turn; a departure job advances 128. A worker
service phase visits at most 1,024 connections and spends at most 4,096
recipient visits before yielding. These constants are `FANOUT_QUANTUM`,
`RETIREMENT_QUANTUM`, `READY_QUANTUM` and `POLL_TICK` in the source.

**Drain versus shutdown.** Drain is reversible: `POST /v1/drain` stops
admitting new connections, readiness returns 503, existing sessions keep
running, and `POST /v1/resume` reopens admission. Shutdown is terminal: a
signal, a worker failure or embedder cancellation stops admission, suppresses
departure notices, clears the departure queues and listener indexes, sends
`1001` to every session within `WAR_SHUTDOWN_TIMEOUT_SECS`, and cannot be
resumed.
