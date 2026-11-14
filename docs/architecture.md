# Architecture

This page describes the relay as it is in the source tree. Terms such as
session, channel, departure notice and staging credit are defined in the
[glossary](glossary.md). Dated measurement records for earlier designs are in
[docs/reports](reports/README.md).

## Runtime shape

The process is one binary, `war-relay`. `main` parses the configuration, loads
secrets, binds the relay and management listeners, then starts a single-threaded
Tokio runtime. Tokio owns two things: the management HTTP service in
[src/admin.rs](../src/admin.rs) and the SIGINT/SIGTERM handler. Everything on
the data path runs outside Tokio.

Data sockets belong to a fixed set of Mio readiness workers in
[src/server.rs](../src/server.rs). Each worker is an OS thread named
`war-io-N`. The default count is the number of available CPUs, capped at 16;
`WAR_WORKERS` overrides it and `WAR_PIN_WORKERS=on` pins each thread to one CPU
(the default `auto` pins only WebTransport workers).
On Linux each worker has its own listening socket; the sockets share the port
with `SO_REUSEPORT`, so the kernel spreads new connections across workers. On
other platforms only the first worker listens. It accepts and hands each new
stream to the worker selected by a hash of the peer address, through that
worker's wake signal, and the stream is registered only with the poller of
the worker that owns it. Hashing mirrors the kernel's placement on Linux;
strict rotation was tried first and left workers idle under a client that
opens sender and receiver sockets alternately, because a sender's worker
does that sender's read, routing and inline write.
Registering duplicated handles of one listening socket in several pollers
stalled workers on Windows, which is why the listener is never shared. Either
way a connection stays with one worker for its whole life. That worker owns the
WebSocket decoder, the timers and the output queue for that socket.

A worker loop has four steps. It services fresh input, then buffered output and
continuations, then one turn of departure work, then it polls. Each of the two
service phases visits at most 1,024 connections and spends at most 4,096
recipient visits on routing before yielding. The poll waits for the next timer
but never longer than `POLL_TICK`, which is one second. A missed wakeup can
therefore delay work by at most one tick instead of stalling the worker.

Shutdown starts when the Tokio side is cancelled by a signal, by a worker
failure or by an embedder. The relay enters the stopping state, workers receive
a stop signal and have `WAR_SHUTDOWN_TIMEOUT_SECS` to send `1001` and retire
their sockets. The supervisor waits one extra second for their reports.

## Locking model

There is no global registry lock. [src/relay.rs](../src/relay.rs) splits state
into concurrent indexes, atomic snapshots and independently locked objects:

- Session ownership (ID to session) is a lock-free concurrent map
  (`papaya`). Lookups, joins and departures are single-key operations without
  a registry mutex. Contention and reclamation still have costs; lock-free
  does not promise bounded completion time for an individual operation.
  An ID takeover publishes the new key before retiring the old
  one, so a route that still finds the old key during that step reaches the
  session that owned it, and a route begun after the assignment reply uses
  the new key. The map holds weak handles: a lock-free map reclaims removed
  entries lazily, and a session's socket and budgets are released by its own
  connection the moment it leaves, never by the map's reclamation schedule.
- The channel index (name to channel) is a lock-free concurrent map with an
  atomic channel count. Lookup is a lock-free read. Creation reserves a slot
  in the count, so `max_channels` holds exactly under concurrent creation,
  then inserts; two creators of one name resolve atomically and the loser
  adopts the winner's channel. Retirement marks the channel, releases its
  slot in the count and removes exactly that channel from the index, all
  under the channel's own lock, so a joiner that sees the mark and retries
  never finds a stale entry and a creator that misses the entry always
  finds the slot free. The count is the limit; the index may briefly hold
  an entry whose slot is already released while its removal completes.
  One boundary case follows from having no index lock: exactly at the
  channel limit, a join that races another client's creation of the same
  name may be refused with `1013` instead of joining that channel. The
  creator looks the name up a second time before refusing, so this needs
  the other creator to be between its reservation and its insert at that
  instant. The client retries, as `1013` asks.
- Each channel has one read/write lock covering its membership, its variables
  and its cached recipient view. It also has an atomic reservation counter used
  for admission and its own departure queue.
- Global observers (members of `re_globl` with listener flags) are an
  immutable snapshot that is swapped atomically. Routing and departures load
  it without locking. A flag change or an observer departure builds the next
  snapshot from the current one and publishes it with a compare-and-swap
  loop, so writers do not lock either; two concurrent changes rebuild from
  each other's result. Routing loads the snapshot once per frame and reads
  from it which observer classes are non-empty.
- Admission counters are atomics: the admission state, the session count and
  each channel's reservation count.
- The list of departure queues with pending work is a lock-free queue.
  Workers pop an entry, run one bounded turn and push it back while it has
  work, without a relay-owned mutex around the list.
- Each departure queue is a mutex around a FIFO of jobs. Disconnect notices
  and `re_globl` departures use sixteen such queues, chosen by the departing
  session's room.

Where locks nest, they nest in this order, from outer to inner:

1. channel
2. one publication job
3. one departure queue
4. socket output state

Nothing holds two locks of one kind at once. The channel index, session
ownership, the observer snapshot, the two caches, the list of pending
departure queues and the publication index are all lock-free. Every lock
that remains belongs to one object: a channel, a departure queue (an
observer shard is one), a publication job or a socket.

A path may skip levels but never takes an outer lock while holding an inner
one. Session ownership, the listener snapshot, the two caches (global
recipient view, disconnect snapshot) and the active list of departure queues
are not locks at all; see below.

No socket write happens while a registry lock is held. Output is either
submitted after the guard has been dropped or queued for the socket's owning
worker, which writes it later. The welcome frame is written before the new
session is inserted into its channel, with no registry lock held, so nothing
can address the session before the welcome and the welcome is first in the
session's FIFO. Revocation barriers follow the same rule: when a global
observer clears a listener bit, its still pending notices are collected and its
watermarks are advanced under each affected queue or publication-job lock.
The notices are submitted to the observer's socket after those locks are released, ahead of the
acknowledgement.

### Shared locks that remain, and what bounds them

Process-wide routing indexes and observer snapshots have no registry mutex.
Queue and publication work is chunked, but cold view construction and snapshot
updates still scale with population. Registry guards never span socket I/O:

- The listener snapshot is loaded and replaced atomically. Routing does not
  acquire an observer-list mutex. Flag changes clone the observer lists and
  may rebuild them again after a lost compare-and-swap race. The barrier
  guarantee (registered pending work is flushed before acknowledging and no
  revoked work arrives afterwards) no longer
  comes from a lock. Each snapshot carries a generation. Every observer job
  records the generation it captured. A revocation publishes the next snapshot
  first, then scans the queued jobs for that observer one observer shard,
  or one publication job, at a time, collects the pending notices, and
  records on the
  observer the generation it revoked at. From then on a job captured at or
  before that generation skips the observer. A job that captured the old
  snapshot but registered only after the scan is therefore skipped rather than
  delivered late: the revocation is the linearization point, and a capture
  concurrent with it may omit that one notice for that observer. Direct
  (unqueued) observer deliveries take no shared lock: each recipient
  re-checks the revocation mark under its own output lock, so a frame
  either precedes the acknowledgement in that observer's FIFO or is dropped.
- Session ownership is a lock-free map. A private message's target lookup
  is a lock-free read; a join, a departure or a takeover is a single-key
  write without a session-registry mutex.
- The channel index is a lock-free map (`papaya`) with an atomic channel
  count. Finding a room is a lock-free read; creating or retiring one is a
  single-key write. Walks of the index (the global recipient view, the
  admin listing and the channel-list command) use no index mutex; a channel
  present throughout a walk is seen once, and one created or retired meanwhile
  may or may not appear. Metadata is read under one channel lock at a time.
  Listings omit ordinary rooms with no members or a retirement mark at that
  read, including handles captured before retirement; the permanent `re_globl`
  room is included when empty. Each row is coherent, but the entire listing
  is not a point-in-time snapshot. The index is pre-sized to the channel
  limit, capped at 65,536 requested entries, to reduce resize work. The
  admission count bounds live channels, not the table's allocated capacity.
  `papaya` finishes an in-progress resize before iterating. The session
  map and the publication index are pre-sized the same way (to the
  connection limit capped at 65,536, and to 4,096 publications), and
  the session map is not walked on the private-routing path. A cold global
  broadcast view does walk the channel index and each room's members.
  Beyond the initial capacities maps can grow; lock-free access does not
  remove resize or memory-reclamation costs.
- The global recipient-view cache and the disconnect-snapshot cache are
  atomic pointer swaps (`arc_swap`). Reads and publication are lock-free; the
  global views are built under one room read lock at a time, while disconnect
  views are built from an immutable observer snapshot. A lost publication race
  stores an identical view or defers to the one already published.
- The active list of departure queues is a lock-free queue
  (`crossbeam_queue::SegQueue`). Workers pop an entry and push it back while
  it has work; departures append one entry when a queue becomes non-empty.
  There is no relay-owned mutex around that list.
- Observer notices (disconnect notices and `re_globl` departures) are spread
  over sixteen departure queues by the departing session's room. A
  departure, a worker turn or a revocation barrier locks one of them at a
  time, and the barrier inspects at most 128 jobs per hold: each shard
  numbers its jobs in queue order and an observer keeps one watermark per
  shard, advanced under the lock after every chunk, so no turn can deliver
  a notice the scan has already collected and no watermark can suppress a
  job the scan has not inspected. A long queue therefore costs a departure
  or a turn on that shard at most one chunk of waiting. The synchronous
  fallback a departure takes when staging credit is refused drains the jobs
  queued before it in holds of at most eight turns, for the same reason.
  Notices for sessions of different rooms are unordered relative to each
  other, as room notices already were.
- The index of large observer publications is a lock-free map with an
  atomic sequence. A barrier collects the live publications, sorts them by
  sequence and inspects each under that job's own lock, storing the
  observer's watermark while the job is held. Registration and completion
  are single-key writes.

The test `unrelated_traffic_stays_responsive_under_large_room_churn_and_revocation`
in `tests/relay_integration.rs` exercises all of them at once: a room of 1,100
members receives a stream of broadcasts (each a continuation), members join and
leave so every departure is a deferred job, and a global observer clears and
restores its disconnect and channel-message flags so barriers scan the
observer shards and the publication index, while a
session in another room echoes continuously. The test fails if any echo takes
longer than one second or if shutdown with the crowd connected exceeds the
fixture's three-second bound. On the development machine the worst echo during
that churn was about 6 ms.

A second test, `unrelated_traffic_stays_responsive_under_room_churn_and_global_broadcasts`,
covers the other shared path: two clients create and retire fresh rooms as
fast as they can, which writes the channel index on every join and every
departure, while a `re_globl` session broadcasts continuously, which
rebuilds the invalidated global recipient view from every room on every
broadcast, an admin client pages through the channel listing, which walks
the whole index, a crowd of 130 disconnect observers (more than one
retirement turn) turns every departure into a queued observer job that
worker turns drain shard by shard, and one more observer keeps clearing and
restoring its flag, so every clear is a chunked barrier scan racing those
departures and turns. A session in a stable room must keep echoing, and
every listing must answer, within one second.
On the development machine roughly 5,000 rooms were created and retired in
2.5 s under 157 global broadcasts and about 1,500 listing pages, and the
worst echo was under 5 ms.

The remaining locks isolate objects rather than imposing a process-wide
critical section. A channel's lock protects membership, variables and cached
views. A cache hit clones one handle, but a cold view scans the members, and
removing a member from a uniquely owned cached view searches that view.
Inspection and global-view construction also read channel state. Those costs
scale with the room, with no socket I/O inside. A departure queue's lock is held for one
bounded turn of at most 128 recipient submissions, which are queue pushes,
never socket writes; a publication job's lock is held for one bounded turn or
one barrier inspection; an observer shard's lock is held for one push, one
bounded turn, one chunk of at most 128 barrier inspections, or at most eight
turns of a synchronous fallback. None of them is taken on the private-message
path. A socket's own output lock is the one lock held across I/O, and only
its own: one non-blocking write of at most 256 KiB to that socket, which a
sender performs on the recipient's behalf when the queue is empty. Only
other senders to that same socket can wait on it.

These per-object locks preserve necessary invariants. A room's
membership must be a consistent set when a broadcast view is built, a
queue must hand out jobs in order, a job's cursor must advance under one
owner and a socket's FIFO must be written by one thread at a time. Each
lock is associated with one object and never nested with another of its kind.
This removes global lock contention; it does not eliminate scheduling delays
for other sockets sharing a worker or CPU, or make every operation constant-time.

## The data path of one message

1. The owning worker reads a complete binary WebSocket message from the
   decoder. Decoder storage is charged against `WAR_TOTAL_READ_BUFFER_BYTES`.
2. `Relay::receive` parses the CBOR envelope `[target, value]`. The value is
   validated but never materialized as an object graph.
3. Target `0` is a relay command. It is handled inline and gets one reply.
4. Another target is looked up in the concurrent ownership map. An unknown target drops
   the message. The lookup and an ID reassignment are atomic with respect to
   each other.
5. The relay builds a routed frame. The sender prefix is stored separately and
   the application value stays in the shared receive storage, so recipients
   share one body. The body is copied only when the receive slab is much larger
   than the message.
6. A private message is charged to the recipient's output budget and handed to
   the recipient's socket. If that socket's queue is empty, the sending worker
   writes it immediately. Otherwise the frame is queued and the owning worker
   is woken.
7. A room broadcast takes the channel's cached recipient view. The view is an
   immutable snapshot of membership. Each recipient except the sender is
   charged and queued. The owning workers write queued output in vectored
   batches. A `re_globl` broadcast uses a cached view of every member of every
   room; the cache is rebuilt after any membership change.
8. A broadcast to more than 1,024 recipients becomes a continuation owned by
   the source connection. The source reserves staging credit for the captured
   view, delivers 1,024 recipients per turn and yields between turns, so one
   large broadcast cannot monopolize a worker. The next message from that
   source waits until the broadcast finishes, which keeps per-source FIFO.
9. If staging credit cannot be reserved for the view, the relay delivers the
   whole broadcast synchronously in one longer turn rather than refusing it.
10. When global observers are present, the message also reaches the eligible
    observers. The primary private delivery always precedes observer work.

## Departure and detached queues

A departure notice is the frame `[18446744073709551615, departed_id]` sent to
the remaining members of the departed session's room, and to disconnect
observers in `re_globl`.

Each ordinary room has its own departure queue. Disconnect-observer notices
and `re_globl` departures use sixteen queues selected by the source room's
hash. Notices from one room retain their order; different rooms have no shared
ordering guarantee. The detached queue holds global broadcasts and observer
publications whose source left before the broadcast finished.

When a session leaves, the relay removes it from its room under the room lock
and captures the remaining members. If there are at most 128 of them and the
room's queue is idle, the notice is delivered immediately, after the lock is
dropped. Otherwise a job is appended to the room's queue. A job holds the
captured view, one charged copy of the frame and a cursor.

Workers rotate across queues that have work. One turn advances the front job
of one queue by 128 recipients and then moves the queue to the back of the
rotation. A large job need not finish before another queue gets a turn;
observer rooms that hash to one shard still share that queue. A job that cannot reserve staging credit makes the relay finish that
queue's older jobs synchronously and then deliver the notice directly, so a
direct notice never overtakes an older queued one.

When a connection closes with an unfinished broadcast of more than 1,024
recipients, the unfinished job is detached. The remaining recipients still
receive the message. For an ordinary room the job goes into that room's
departure queue, ahead of the source's own departure notice, because the
notice is queued afterwards. A detached global broadcast or observer
publication goes into the detached queue and is not ordered against the
source's departure notice. Refusals during a detached job are ignored, since
there is no source left to report them to.

Observer barriers use sequence watermarks and generations. Each observer records
one departure watermark per shard, a publication watermark, and the listener generation
at which it last revoked. When a global observer clears a listener bit, the
relay submits that observer's pending notices before the acknowledgement and
advances both watermarks, so deferred jobs skip those notices afterwards,
re-subscribing does not replay them, and a job captured concurrently with the
revocation is skipped instead of arriving late.

## Pressure policy

Three budgets bound retained output. `WAR_QUEUE_MESSAGES` limits queued frames
per session. `WAR_QUEUE_BYTES` is a per-session byte allowance.
`WAR_TOTAL_QUEUE_BYTES` is the global byte budget shared by every session,
every in-flight body and every captured view. A reservation is held from the
moment a frame is charged until its write completes or is cancelled.

The policy is:

- Per-session exhaustion (`WAR_QUEUE_BYTES` or `WAR_QUEUE_MESSAGES`) closes
  that recipient with `1013`. It is the slow consumer.
- Global exhaustion never punishes the sender. The frame for that recipient is
  dropped and `war_global_pressure_total` increments. The recipient is closed
  with `1013` only if it already holds at least a quarter of its own
  `WAR_QUEUE_BYTES` allowance, or if the dropped frame was the reply to its
  own command.
- A session whose own command reply, including the welcome, cannot be funded
  is closed with `1013`, because it would otherwise wait forever.
- Senders are never closed because of a recipient. A routing refusal for one
  recipient does not abort delivery to the others.
- Staging credit for a captured view or a departure job is reserved strictly.
  A refusal falls back to synchronous delivery and counts in
  `war_global_pressure_total`. That counter therefore counts refused
  reservations, not lost messages.

Delivery under pressure is best-effort. Recipients are visited in view order,
so a later recipient of one broadcast can miss a frame that an earlier
recipient received. Already submitted output is never rolled back.

## What is deliberately not guaranteed

- Ordering between senders. Messages from one session arrive in order at
  every recipient; messages from different sessions have no total order.
- Ordering of departure notices across rooms. Each room's queue is advanced
  independently.
- Ordering between a detached global broadcast and its source's departure
  notice. They sit in different queues.
- Delivery under global output pressure. A frame can be dropped for one
  recipient while others receive it.
- Delivery to a session that is being closed. Frames addressed to a closing
  session are discarded.
- Receipt of a departure notice because the departed peer completed its Close
  handshake. That handshake is not an acknowledgement from other recipients.
- Persistence, replay or reconnection. The relay stores nothing across
  sessions except channel variables, and `re_globl` variables only until
  restart.
- Routing state shared between processes. Each process is an independent
  relay.
