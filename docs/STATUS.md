# Continuing goal status

The user's ongoing goal is to port the upstream relay faithfully to Rust and keep
improving simplicity, code, documentation, stability, performance, concurrency,
and adjacent platform management. A later instruction makes CBOR the primary
protocol, and the latest instruction emphasizes preserving the upstream's
zero-copy routing intent. The goal remains active; this file is a reviewable checkpoint, not a
claim that an open-ended improvement effort is complete.

## Implemented and locally exercised

- Upstream downloaded at `881c8daaf2cf05adea8f522b28cf372e8f82d4e9`; source/history
  and MIT attribution retained on local branch `codex/rust-relay`.
- Rust/Tokio CBOR relay: channel and global broadcast, cross-channel private
  messages, listener flags, typed channel variables, entropy-backed IDs, fixed-ID
  takeover, and disconnect events.
- One CBOR routing envelope. Definite-length values are bounded and forwarded
  verbatim. Browser/Node-oriented JS client preserves uint64 IDs as BigInt and
  serializes control requests.
- Separate optional legacy bridge; original `relay.js` exercised unchanged, plus
  modern/legacy byte-string and text interoperability.
- Bounded handshake/join/admission, per-peer and process queue budgets, channel
  and process variable budgets, inbound rates, heartbeat, write and shutdown
  deadlines. RAII cleanup and takeover ownership checks prevent stale removal.
- Separate management listener with liveness/readiness, Prometheus, authenticated
  channel inspection, drain/resume, and administrative disconnection.
- Release socket workload, independent CBOR and legacy vectors, Rust network
  integration checks, client checks, CI workflow, protocol and operations docs.
- Current checks: 68 Rust tests and 7 JavaScript socket cases pass, along with
  strict Clippy and formatting. CI is supplied but has not run on GitHub.
- Read buffer changed from 128 KiB to 4 KiB after repeated 2,000-socket measurements
  showed about 91% lower idle RSS. Throughput variability is documented separately.
- Shutdown suppresses quadratic disconnect traffic; output limits allow channel
  queries larger than incoming application-message limits.
- Sustained checks found and fixed missing Close acknowledgements and delayed
  eviction of unread recipients. Shared close handling covers native CBOR and
  legacy clients, before and after join. Pending writes observe policy cancellation;
  routing membership/queued reservations release before a graceful handshake.
- A 30-second rerun verified 35,328,176 deliveries. A 20-second churn rerun completed
  1,911,994 connection lifecycles with 64 workers. An unread recipient was removed
  by the first 250 ms post-overflow sample while all 530,408 healthy deliveries
  were verified. All cases returned to zero sessions/connections/queued bytes and
  the persistent global channel. See stability.md for method and caveats.
- Repeated, affinity-separated socket measurements reproduced the cancellation
  overhead. Reusing pinned terminal waits recovered it: median 1,623,686 verified
  deliveries/s versus 1,384,284 for the fixed-close control, with 17.1% less server
  CPU per delivery. Close/eviction behavior remains intact. Idle state cost rose
  about 0.50 MiB at 2,000 sockets. See performance.md for raw evidence and limits.
- Native forwarding now shares received application storage through fanout and
  vectored socket output. The small sender prefix is separate, so varying CBOR
  integer widths require no body movement. Tiny views after enlarged input are
  compacted before publication; ordinary small messages share the normal slab.
- Final repeated 64 KiB traffic used 27.4% less server CPU per verified delivery
  with 4.6% higher median throughput. At 1 KiB, throughput ranges overlapped and
  CPU per delivery rose 2.1%. The copy counter stayed zero in all traffic runs;
  pointer checks cover queue and write ownership. Median idle RSS rose 0.86 MiB
  at 2,000 sockets. Raw evidence and limits are in performance.md.
- Final lifecycle checks verified 917,429 cumulative reconnects and 259,154
  healthy deliveries alongside an unread client, with complete cleanup. Finite
  fanout checks verified 31,500 deliveries per run. Fragmented input, interleaved
  controls, partial writes, and all WebSocket output length boundaries are covered.
- Local runtime refreshed on 2026-10-15: native CBOR at `127.0.0.1:1338`, legacy
  bridge at `127.0.0.1:1337`, management at `127.0.0.1:1339`. Both management
  health/readiness checks returned 200. Three socket checks against these running
  release processes exercised the unchanged original client, mixed CBOR/legacy
  values, and clean Close. A separate release workload verified 16 private
  deliveries. Gauges returned to zero afterward. Binary/process evidence is in
  `benchmarks/2026-10-15-zero-copy-live.json`; this is a timestamped observation.
- Sustained mixed-size fanout now verifies every reader's frames, bytes, ordering,
  disconnect events, and simultaneous graceful departure, with bounded templates,
  histograms, and acknowledgements. Optional unread and delayed readers exercise
  concurrent data and control pressure.
- A 128-client departure wave reproduced resets when a capacity signal cancelled
  output before its first poll. The connection now distinguishes untouched output
  from a possibly partial frame and preserves an already queued peer Close on
  the safe closing path. Three old-build runs failed; all three fixed runs passed.
- Final-build 30-second / 128-client mixed-size acceptance verified 3,440,304
  deliveries to 126 healthy readers beside one unread peer and a delayed reader.
  Peak sampled RSS was 13.96 MiB. Routed bytes were 99.784% reused; the existing
  small-view compaction policy accounted for the rest. A repeated 1 KiB cost guard
  recorded 1,582,701 versus 1,536,783 median deliveries/s with the same p99 bucket;
  no general performance gain is attributed to these correctness changes. See
  stability.md for the separate implementation stages, outcomes, and limits.
- The current local release processes were refreshed after that fix. A real Node
  client group completed 128 simultaneous clean closes with code 1000, and three
  additional live checks exercised the original client, native/legacy values, and
  Close before/after joining. Readiness/liveness returned 200 and gauges returned
  to zero. The timestamped binary/process snapshot is in
  `benchmarks/2026-10-15-graceful-close-live.json`.
- A further regression reproduced missing Close acknowledgement after input
  quota exhaustion. Both receive loops now handle a valid Close before rate
  accounting; the regression exercises this before and after joining. Other
  inbound traffic retains its limits.
- A 17-byte incoming application limit also rejected valid 125-byte WebSocket
  control frames. The frame ceiling now permits RFC-sized controls while the
  independent application-message limit still rejects 18-byte complete and
  fragmented data. Both real-socket boundary checks pass in debug and release;
  full-sized Ping/Pong/Close work before and after joining.
- A pinned esbuild development dependency produces a standalone ES2022 browser
  module and a small acceptance page. Actual Chrome 155 execution imported that
  generated library, passed seven checks (typed/large delivery, exact uint64,
  concurrent variables, disconnect, clean Close), and recovered from an invalid
  URL. Desktop and 390-pixel-wide output were inspected. The browser check
  routed 131,173 shared value bytes with no body compaction and returned all
  session, connection, queue, and variable gauges to zero. Source/binary hashes
  and timestamped observations are in
  `benchmarks/2026-10-15-browser-control-acceptance.json`.
- The optional local bridge was restored after one unexplained clean process
  exit. Three live compatibility/Close checks passed through its replacement.
  The recorded exit code was zero, with no error output; no code defect or
  recurrence has been established. Retain this observation for a later lifecycle
  investigation if it recurs.
- A malformed-CBOR campaign found that expanded simple values `f8 00` through
  `f8 1f` passed the decoder and reached a recipient. A check confined to simple
  values now rejects these RFC-invalid forms without changing payload storage.
  The real-socket regression rejects 192 routed/nested/map-key/variable cases,
  including 80 fragmented values with interleaved Ping. It also verifies 248
  valid simple-value deliveries byte for byte and healthy recovery. Independent
  unit vectors cover truncation, reserved headers, malformed UTF-8, nonminimal
  definite headers, duplicate keys, floating-point bits, and exact depth/item
  boundaries. Debug and release network checks pass.
- Repeated receive-slab retention checks now alternate large unknown-target
  primers with tiny output held behind an unread recipient. All six fresh-server
  cases passed full delivery/ordering, Close, counter, and cleanup checks. Raising
  the primer count from 64 to 256 changed median sampled peak RSS from 15,828 to
  15,920 KiB; the large forwarded values reused 8,388,648 encoded bytes in each
  case, while only 211/981 tiny value bytes were compacted. The existing policy
  is unchanged. Method, raw records, and finite-workload limits are in
  `performance.md` and `benchmarks/2026-10-15-retention-pressure.json`.
- Current local native/bridge releases include the simple-value check. Two live
  malformed probes closed cleanly with `1002` without harming a healthy peer or
  storing a variable. Seven Chrome acceptance checks and three live legacy/Close
  checks passed; health/readiness returned 200 and gauges returned to zero. The
  timestamped process/hash snapshot is
  `benchmarks/2026-10-15-cbor-retention-live.json`.
- The retention probe now supports synchronized producers sharing one unread
  destination, per-producer ordering checks, connected-idle observation, and
  automatic idle-close verification. Twelve fresh-server runs across 1, 8, and
  32 producers passed all delivery, counter, Close, and cleanup checks. Empty
  queues can coexist with enlarged receive capacity on still-open sessions;
  connection and input-size budgeting remain necessary alongside queue limits.
- Optional `--idle-timeout-secs` / `WAR_IDLE_TIMEOUT_SECS` retires joined sessions
  without completed application input/output, defaulting to disabled. Ping/Pong
  does not extend it; receive-only application output does. Queued/in-flight
  output retains the existing write/queue policy. Normal Close cleanup removes
  registrations and releases ordinary channel variables when the last member
  leaves. Decoder references release at transport end; published shared bodies
  keep their recipient-owned lifetime.
  Three real-socket regressions cover controls, active input/receive-only output,
  and unfinished fragmented input. All enabled retention cases observed exactly
  the expected 2/9/33 idle decisions and clean Close handshakes.
- The enabled-policy profile exposed per-message retiming: `Sleep::reset`
  accounted for 8.38% of sampled user cycles. The final schedule retains an
  already earlier alarm when activity postpones expiry, checks the live deadline
  on completion, and immediately retimes for an earlier heartbeat deadline.
  Exact expiry is preserved without coarse idle polling. The final enabled
  profile no longer listed reset among symbols at or above the 0.5% cutoff.
- Sixteen balanced twenty-second comparison cases passed. Final disabled-mode
  medians were 1,701,361 deliveries/s and 1,689 CPU ns/delivery, versus 1,610,854
  and 1,780 for `14b9529`. Final enabled-mode medians were 1,606,449 and 1,776,
  with the same 1.049 ms p99 bucket. Delayed rearming reduced enabled CPU by 2.0%
  versus the initial policy; its throughput ranges overlap, so the modest local
  result is not a general speed claim. Enabling the policy retains a measured
  cost against the new disabled default. Full ranges and methods are in
  `performance.md` and `benchmarks/2026-10-15-idle-final-cost.json`.
- Final retention checks passed at 1/32 producers and then 64 producers. The
  65-transport case verified all 4,105 data/barrier deliveries and 65 idle Close
  handshakes, with 8,388,648 encoded large-value bytes reused and 5,249 tiny bytes
  compacted. Sampled RSS fell from 269,212 KiB before acknowledgement to 6,560 KiB
  after retirement. Queue/connection/session/variable gauges returned to zero.
- The JS fixture now verifies six default-policy cases, restarts the temporary
  relay with idle expiry enabled, and verifies clean native/legacy idle Close.
  The native control test also verifies the 18-byte idle Close acknowledgement
  with a 17-byte application limit. Both heartbeat-only and combined idle/
  heartbeat tests confirm the earlier heartbeat deadline still wins.
- The current local native/bridge releases include the final schedule. The
  normal endpoints explicitly keep application-idle expiry disabled. Seven Chrome
  checks and three live legacy/Close checks passed, health/readiness returned
  200, and all gauges returned to zero. Process/hash and verification evidence is
  `benchmarks/2026-10-15-application-idle-live.json`.

## Exact verification boundaries

Unused enlarged native decoder storage now retires after five seconds without
large complete input, with `--read-buffer-idle-secs 0` preserving indefinite
reuse. It shares the existing pinned deadline wait, keeps clients connected and
published values in their original storage, defers busy frames, and preserves
fragment/control state. Three small backend hooks are vendored with original
package/commit checksums and the complete patch; CI audits all 61 package files
against the pinned archives plus that patch. The first-download and cached audit
paths both passed locally.

Eighteen matched connected-idle cases, a disabled-policy case and a 64-producer
control/default pair passed. At 32 producers the median last connected-idle RSS
fell from 140.24 to 12.02 MiB; the 65-connection pair fell from 262.93 to 12.43 MiB
without disconnecting clients. Sampled peak concurrent-input memory remained unchanged.
Small-population allocator variation and a roughly 2.6% observed large-payload
CPU cost are retained in `performance.md`, alongside the overlapping throughput
ranges and every raw record. Small-message traffic cost remained close to the
control, and 2,000 ordinary idle sockets added a median 20 KiB RSS versus control.

All 52 Rust checks, strict Clippy, seven JS socket cases and the original C++
46-case/eight-difference comparison passed with this version. The current local
native/bridge releases include the policy. A live five-second reset retained
both sessions and subsequent typed routing added no body copies; both closed
cleanly. Seven fresh Chrome checks and three live legacy/Close checks passed;
health/readiness returned 200 and final resource gauges were zero. See
`benchmarks/2026-10-15-read-buffer-idle-{build,summary,live}.json`.

The unchanged bundled C++ executable now runs with locally extracted, pinned
Debian TBB/OpenSSL libraries inside a private network namespace. Three fresh
process comparisons passed 46 common cases and eight explicitly asserted
differences per implementation: routing, byte/text and frame-size boundaries,
listeners, variables, fixed IDs, departure, and cleanup. Both implementations
acknowledged their eight remaining client closes; Rust's sessions, connections,
queue and variable gauges returned to zero with no write, Close, or transport
errors. The original's pre-GC membership/variable retention and several documented
behavior fixes were observed directly. See `compatibility.md` and
`benchmarks/2026-10-15-cpp-reference-{1,2,3}.json` for exact scope and provenance.
This compares the preserved executable using these host/local libraries; it
does not reconstruct its original operating system or measure C++ performance.
Native CBOR client tests use Node 24's real WebSocket;
the browser acceptance page has passed through the generated ES2022 module in
Chrome 155 on this Linux host. Other browsers and production bundler integrations
remain unverified. CI builds the bundle but does not run the browser acceptance.

Deployment examples are provided. The systemd unit was verified using the built
workspace executable substituted for its intended `/usr/local/bin/war-relay`
installation path. No system service or public endpoint has been installed.
Caddy is not installed here, so its example was not executed. Measurements are
local single-host plaintext sockets; no production or cluster claim is made.

## Decoder staging admission

The native relay now reserves decoder staging before allocating, in addition to
reclaiming unused storage. `--total-read-buffer-bytes` /
`WAR_TOTAL_READ_BUFFER_BYTES` defaults to 1 GiB; `0` disables admission. A bounded
16 KiB HTTP upgrade has temporary headroom, then separate frame/fragment leases
retain twice their requested capacity. Large frames include one charged read
chunk of read-ahead. Ordinary reads within the high water perform no shared
budget atomics or guard allocations. Refusals return HTTP `503` or a best-effort
WebSocket `1013`, before refused decoder growth. Frame admission precedes body
reads; fragment admission precedes appending the decoded frame. Reclaim/drop/
cancellation release the allowances; completed payloads retain their existing
shared owners.
This controls staging requests, not total process RSS or published body storage.

Three paired 64-source partial-frame runs with a 24 MiB allowance reduced median
held RSS from 148.22 to 8.14 MiB, while independent healthy routing and later
admission remained verified. The default admits most of that cohort; the memory
reduction belongs to the deliberately tighter configuration. Nine final pressure
cases, an eight-producer retention/reclamation case and over three million
selected-server churn lifecycles passed. Exact-size allocation was rejected
after repeated large-message CPU regressions; charged read-ahead recovered the
cost. Selected 1 KiB and 64 KiB CPU/throughput medians stayed within a few percent
of the prior release, with identical p99 bounds. Standing idle RSS increased
940 KiB at 2,000 sessions. See `performance.md` for all costs and limitations.

All 60 Rust tests, strict Clippy, formatting, seven JS socket cases and the
46-case/eight-difference original C++ comparison passed. Vendor verification now
covers 62 files against the original pinned packages plus the complete declared
patch. The host subsequently rebooted: saved runtime sources/releases matched
their recorded hashes; native/bridge/HTTP/Chrome were restarted, and all Rust,
Clippy/format/vendor checks, seven Chrome checks, three live legacy checks and
the five-second live reclamation/shared-routing check passed again. Final live
connection/session/queue/variable/staging gauges and error counters were zero.

At that checkpoint the native release hash began `a420b7a37a1d` and the bridge
began `d82df74b7cf8`. Exact hashes, sources, measurements and post-reboot live
evidence are in `benchmarks/2026-10-15-input-budget-{build,summary,live}.json`.

## Fragment capacity under pressure

A new real-socket check found that a valid 4 MiB message split into 3 MiB/1 MiB
fragments was refused under a 16 MiB staging allowance: geometric assembly
growth requested 6 MiB although the final length was known to be 4 MiB. The
decoder now bounds intermediate growth by the configured message ceiling and
final growth by the complete length, while retaining already sufficient capacity.
Actual-length validation still precedes allocation/admission. Text and binary
assembly preserve ownership and split UTF-8 state; unmetered behavior is retained.
No published application body moves as a result of this change.

Three alternating release pairs used the same frozen verifying client and the
normal 16 MiB message ceiling, eight producers, three 4 MiB waves and independent
healthy traffic. Peak staging allowance fell from 144.07 to 112.07 MiB (22.2%).
Median sampled peak server RSS fell from 90.08 to 77.50 MiB, with wide ranges;
no general RSS, CPU or throughput improvement is claimed. All 24 messages,
12 normal Close handshakes, per-wave resets and final resource release passed
in every run, with zero routed-body compaction. A 64-producer/one-healthy-client
case verified all 192 fragmented messages, 68 normal Close handshakes and all
three returns to the expected 532,480-byte connected allowance.

Fifteen complete-fragment workloads, including the expected old-release refusal,
and a 64-source partial-frame check passed. They cover the default five-second
reset, disabled reset/admission, the exact 64 KiB reset threshold and one byte
above it, tight staging admission, and the README command with a freshly built
client. All 65 Rust tests, strict Clippy, formatting, the 62-file vendor audit,
seven JavaScript socket cases and the 46-common-case/eight-explicit-difference
original C++ comparison passed. Sources, exact hashes, raw observations and
limitations are bound in `benchmarks/2026-10-16-fragment-final-{build,summary}.json`;
`performance.md` describes the measurements and earlier exploratory candidate.

The verified final native/bridge binaries replaced the quiescent post-reboot
processes. A fresh headed Chrome run passed all seven checks with no console
messages, and three live legacy/Close cases passed. Live decoder retirement
returned two connected sessions to their 16 KiB base after 5.019 seconds; typed
routing after retirement added no body copies. Health/readiness returned 200,
and final connection/session/queue/variable/staging gauges and error counters
were zero. `benchmarks/2026-10-16-fragment-final-live.json` records the process
identities, exact release hashes, browser report and live probe source.

At that checkpoint the native SHA-256 began `6f801ea16bdb` and bridge began
`933efb31e760`, using the same local endpoints listed below.

## Fragment scheduling (current checkpoint)

Valid messages with many empty continuations exposed a scheduling problem:
decoder reads could parse many frames before returning to the executor.
The native relay and legacy server now yield after at most 256 parsed frames
without a message. They resume the same incomplete data, UTF-8/control state
and admission leases; published CBOR values retain their shared ownership.
The asynchronous wrapper explicitly wakes itself so buffered input resumes
without requiring another network event. Library defaults remain unchanged;
the quota controls frame count per poll, without a time or total CPU guarantee.

Three alternating single-worker pairs at each read size verified 64 million
empty continuations, 32 complete messages and all five normal Close handshakes
per run. The median of maximum healthy RTTs fell from 10.662 to 0.871 ms with
4 KiB reads and from 322.164 to 0.864 ms with 128 KiB reads. Management RTTs also
fell. Fragment-heavy CPU increased about 1.7–1.8% in these medians. A separate
four-worker/16-source pair at each read size verified all 128 messages and
17 Close handshakes; maximum healthy RTT stayed below 0.8 ms on the candidate.
These are finite local results, without a production latency claim.

Twelve ordinary traffic cases verified 178,598,784 deliveries. The selected
1 KiB/64 KiB CPU medians changed by less than 0.6%, throughput by approximately
1% or less, with unchanged p99 bounds. The frozen traffic client intentionally
drops its TCP handles; every expected departure and final resource release was
accounted for, with normal Close verified independently. The new workload's
19 recorded cases also cover the README command, tight 16 MiB fragment staging
with the default five-second reset, and three reclamation waves with 64 sources
plus healthy traffic. All final staging/session/connection/queue/variable gauges
were zero; routed-body compaction stayed zero.

All 68 Rust tests, strict Clippy, formatting, the 62-file vendor audit and seven
JavaScript socket cases pass. The original C++ comparison now includes binary
and split-UTF-8 text messages with 1,024 empty continuations: all 48 common cases
and eight explicit differences pass. Three new decoder checks cover bounded
progress, explicit wakeup with entirely buffered input, cancellation/resumption,
UTF-8/control preservation, published payload pointers and terminal errors.

The verified release replaced the quiescent local native/bridge processes.
A fresh Chrome run passed all seven checks with no console messages, three
live legacy/Close cases passed, and live decoder retirement returned two
connected sessions to 16 KiB after 5.040 seconds. Subsequent typed routing added
no body copies. Health/readiness returned 200; final resource gauges and error
counters were zero. Exact sources, hashes, counters, workload traces and live
process identities are bound in `benchmarks/2026-10-16-fairness-{build,summary,live}.json`.
See `performance.md` for cost and measurement limits.

Current local endpoints are native `ws://127.0.0.1:1338`, optional bridge
`ws://127.0.0.1:1337`, management `http://127.0.0.1:1339`, and browser check
`http://127.0.0.1:4180/`. Native SHA-256 begins `8bfeb5be88d2`; bridge begins
`17e21d754a6b`. Application-idle Close is disabled for manual testing; normal
five-second retirement and 1 GiB staging admission remain enabled.
The ongoing goal remains active; no hosted deployment or cluster capacity is
claimed.

## Next useful iterations

1. Extend the independent original-runtime comparison when routing, listener,
   variable or ID-management behavior changes. The current 48-case corpus and
   eight explicit difference checks now have real C++ wire evidence; longer
   scheduling windows, adverse networks and its garbage-collection cycle remain
   outside that finite comparison.
2. Profile the remaining small-message CPU cost before changing counters, queue
   representation, or read storage. Current profiles still show signal/waker
   polling; avoid a broad event-loop rewrite without a measured whole-socket win.
   Concurrent retention, staging pressure and delayed storage reclamation now
   cover up to 64 producers plus independent healthy traffic across repeated
   waves. Process-wide reservations precede decoder allocations and cover
   incomplete frames plus fragment assembly; final-fragment growth uses its
   known complete length. Empty-continuation parsing now yields cooperatively;
   extend mixed nonempty fragment sizes, malformed CBOR and cancellation/deadline
   precedence with concurrent healthy traffic. Increase duration and populations
   when the measurements justify the next change. Preserve shared bodies,
   exact peer Close acknowledgements, and immediate dropping of partial output.
3. Extend duration, fanout, active populations, adverse networks, and connection
   mixes. Thirty-second traffic and twenty-second churn are now verified, while
   multi-hour, TLS, and much larger concurrent populations remain unmeasured.
   Examine global counter/registry contention only if those profiles show it;
   keep the current simple ownership model until a measured bottleneck justifies
   a different representation or sharding.
4. Extend browser acceptance across engines and production bundlers. Broaden the
   deterministic malformed-CBOR/fragmentation corpus with reproducible generated
   inputs and concurrent healthy traffic, preserving resource bounds and source
   fidelity.
5. Revisit connection-admission policy for public multi-application operation
   (per-IP limits, origin policy, tenant boundaries) with an explicit compatibility
   and proxy model. The existing global/private scopes intentionally match WAR.
6. Design node ownership and cross-node routing only after single-node capacity
   and operational needs are established. No replicas are currently a cluster.
