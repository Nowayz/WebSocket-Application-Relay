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
`WAR_WORKERS` overrides it and `WAR_PIN_WORKERS` pins each thread to one CPU.
On Linux each worker has its own listening socket; the sockets share the port
with `SO_REUSEPORT`, so the kernel spreads new connections across workers. On
other platforms only the first worker listens. It accepts and hands each new
stream to a worker in round-robin order through that worker's wake signal, and
the stream is registered only with the poller of the worker that owns it.
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
into independently locked pieces:

- Session ownership (ID to session) is a lock-free concurrent map
  (`papaya`). Lookups, joins and departures are single-key operations that
  never wait. An ID takeover publishes the new key before retiring the old
  one, so a route that still finds the old key during that step reaches the
  session that owned it, and a route begun after the assignment reply uses
  the new key. The map holds weak handles: a lock-free map reclaims removed
  entries lazily, and a session's socket and budgets are released by its own
  connection the moment it leaves, never by the map's reclamation schedule.
- The channel index (name to channel) is one read/write lock. Lookup takes the
  read side; creating or retiring a channel takes the write side.
- Each channel has one read/write lock covering its membership, its variables
  and its cached recipient view. It also has an atomic reservation counter used
  for admission and its own departure queue.
- Global observers (members of `re_globl` with listener flags) are an
  immutable snapshot that is swapped atomically. Routing and departures load
  it without locking; only flag changes serialize among themselves on a small
  writer mutex, build the next snapshot and publish it. An atomic mask records
  which observer classes are non-empty, so ordinary routing does not even load
  the snapshot when nobody is listening.
- Admission counters are atomics: the admission state, the session count and
  each channel's reservation count.
- The list of departure queues with pending work is a mutex that workers only
  ever try to lock. A busy worker never waits on it.
- Each departure queue is a mutex around a FIFO of jobs.

Where locks nest, they nest in this order, from outer to inner:

1. channel index
2. channel
3. listener writer mutex (flag changes and observer departures only)
4. publication index, then one publication job
5. departure queue
6. socket output state

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
watermark is advanced under the listener lock, and the notices are submitted to
the observer's socket only after that lock is released, ahead of the
acknowledgement.

### Shared locks that remain, and what bounds them

Some state is inherently process-wide. Most of it is now lock-free; what is
still a lock is held for a bounded, small amount of work and never across
socket I/O:

- The listener snapshot is wait-free to read. Routing never waits on observer
  changes. The barrier guarantee (a revocation flushes every job that captured
  the observer before acknowledging, and nothing arrives afterwards) no longer
  comes from a lock. Each snapshot carries a generation. Every observer job
  records the generation it captured. A revocation publishes the next snapshot
  first, then scans the queued jobs for that observer under the queue or
  publication-index mutex, collects the pending notices, and records on the
  observer the generation it revoked at. From then on a job captured at or
  before that generation skips the observer. A job that captured the old
  snapshot but registered only after the scan is therefore skipped rather than
  delivered late: the revocation is the linearization point, and a capture
  concurrent with it may omit that one notice for that observer. Direct
  (unqueued) observer deliveries hold the same queue or index mutex for their
  short loop, so they cannot land after the acknowledgement either.
- Session ownership is a lock-free map. A private message's target lookup
  is a wait-free read; a join, a departure or a takeover is a single-key
  write that never blocks a reader.
- The global recipient-view cache and the disconnect-snapshot cache are
  atomic pointer swaps (`arc_swap`). Reads and publication are wait-free; the
  views themselves are built outside any lock, and a lost publication race
  stores an identical view or defers to the one already published.
- The active list of departure queues is a lock-free queue
  (`crossbeam_queue::SegQueue`). Workers pop an entry and push it back while
  it has work; departures append one entry when a queue becomes non-empty.
  Nobody waits on it.

The test `unrelated_traffic_stays_responsive_under_large_room_churn_and_revocation`
in `tests/relay_integration.rs` exercises all of them at once: a room of 1,100
members receives a stream of broadcasts (each a continuation), members join and
leave so every departure is a deferred job, and a global observer clears and
restores its disconnect flag so barriers scan the observer queue, while a
session in another room echoes continuously. The test fails if any echo takes
longer than one second or if shutdown with the crowd connected exceeds the
fixture's three-second bound. On the development machine the worst echo during
that churn was about 6 ms.

A second test, `unrelated_traffic_stays_responsive_under_room_churn_and_global_broadcasts`,
covers the other shared path: two clients create and retire fresh rooms as
fast as they can, which takes the channel index write lock on every join and
every departure, while a `re_globl` session broadcasts continuously, which
rebuilds the invalidated global recipient view from every room on every
broadcast. A session in a stable room must keep echoing within one second.
On the development machine roughly 6,800 rooms were created and retired in
2.5 s under 157 global broadcasts, and the worst echo was under 4 ms.

What the remaining per-object mutexes protect, and why they cannot stall
unrelated work: a channel's lock is held for one map insert or removal, or
for cloning one handle to the cached view, with no I/O inside, and only that
room's traffic ever takes it; a departure queue's lock is held for one
bounded turn of at most 128 recipient submissions, which are queue pushes,
never socket writes; the publication index mutex is held for one registration,
one removal, or one barrier scan, and otherwise only while observers exist.
None of them is taken on the private-message path at all.

## The data path of one message

1. The owning worker reads a complete binary WebSocket message from the
   decoder. Decoder storage is charged against `WAR_TOTAL_READ_BUFFER_BYTES`.
2. `Relay::receive` parses the CBOR envelope `[target, value]`. The value is
   validated but never materialized as an object graph.
3. Target `0` is a relay command. It is handled inline and gets one reply.
4. Another target is looked up in the ownership shard. An unknown target drops
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

Each room has its own departure queue. `re_globl` room departures and
disconnect-observer notices share one queue, because both reach `re_globl`
members and one order keeps barriers simple. A third queue, the detached
queue, holds live broadcasts whose source left before the broadcast finished.

When a session leaves, the relay removes it from its room under the room lock
and captures the remaining members. If there are at most 128 of them and the
room's queue is idle, the notice is delivered immediately, after the lock is
dropped. Otherwise a job is appended to the room's queue. A job holds the
captured view, one charged copy of the frame and a cursor.

Workers rotate across queues that have work. One turn advances the front job
of one queue by 128 recipients and then moves the queue to the back of the
rotation. A large job in one room therefore cannot delay another room's
notices. A job that cannot reserve staging credit makes the relay finish that
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

Observer barriers are watermarks on two axes. Each observer records the queue
sequence up to which its notices have been flushed and the listener generation
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
