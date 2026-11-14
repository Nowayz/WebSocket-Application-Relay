# WebTransport / WebSocket causal profile, 2026-10-21

The evidence supports an implementation and benchmark-library problem, alongside
real QUIC protocol costs. It does not establish that unreliable datagrams are
inherently slower than WebSocket. Earlier reliable-stream throughput results
must not be presented as datagram throughput results.

## Method and boundaries

Server: unchanged release commit `acee0264362d9ce0f57245e44765a7e1fee73032`,
`artifacts/webtransport/binaries/capped-record-relay`, SHA256
`3a8e08cd36d9a064f1a42371cb5dcd3c5f42e967ba20df6fd55a3448dc0818fd`.
Host: AMD Ryzen AI MAX+ 395, Linux x86_64. Four individually pinned server workers
on CPUs 0–3; four client Tokio workers allowed on CPUs 8–11. Sequential fresh
processes, 32 self-routing sessions, 1,024-byte values, one outstanding echo per
session, eight seconds. Every completed response is compared byte for byte;
all 19 ordinary/profile probe invocations exited successfully. Three ordinary
repetitions per comparison, reversing order between repetitions. No builds,
tests or other benchmark jobs ran alongside these measurements.

The first comparison uses the same independent `wtransport` client for both QUIC
lanes and `tokio-tungstenite` for plaintext WebSocket. A second comparison changes
only the QUIC client implementation to `web-transport-quinn`, retaining the
`wtransport` TLS/QUIC configuration builder, server certificate pin, same shared
UDP endpoint topology, request pattern, payload checks and four-worker runtime.
It removes the independent client's extra WebTransport datagram dispatcher.
This diagnostic client is archived as
`artifacts/webtransport/causal-profile/transport_probe_direct.rs`; the original
is `examples/transport_probe.rs`. Both release executables are frozen under
`artifacts/webtransport/binaries/causal-transport_probe*`; hashes and full receipts
are in the companion JSON.

This is a latency-sensitive closed-loop diagnostic, **not** a saturation,
fanout, production WSS, browser, packet-loss, cleanup-gauge or 100,000-session
acceptance run. Application bytes are matched; reliable/unreliable delivery
contracts and client implementations cannot be identical. CPU deltas include
brief setup/teardown, so use them as approximate per-completion costs. RSS after
client exit in these receipts is not live-session memory. The plain WebSocket
control does not pay TLS costs. No conclusion about an optimized native WSS
ceiling follows from this comparison.

## Ordinary results

Medians of three runs; profiled invocations are excluded.

| Lane, original client | Completed echoes/s | RTT p50, us | RTT p99, us | Server CPU, us/echo | User CPU, us/echo | Kernel CPU, us/echo |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| Plain WebSocket | 628,054 | 49.208 | 87.768 | 2.931 | 0.376 | 2.551 |
| WebTransport reliable stream | 529,924 | 58.708 | 160.945 | 5.616 | 3.154 | 2.450 |
| WebTransport datagram | 496,400 | 63.138 | 124.027 | 5.487 | 2.802 | 2.690 |

Datagrams use slightly less server CPU than the reliable stream. Their lower echo
rate is affected by client dispatch/latency, rather than demonstrating greater
server processing cost. At 32 outstanding requests the turnaround path matters;
none of these rates should be described as the transport's maximum throughput.
The large difference from WebSocket is extra user-space work, not a collapse in
UDP kernel efficiency. Kernel and process accounting does not capture every
host-wide softirq/resource cost, and should not be treated as a whole-machine
protocol efficiency audit.

## Controlled client-library intervention

Same server executable for all six runs, alternate client order across three
repetitions:

| Datagram client | Completed echoes/s | RTT p50, us | RTT p99, us | Server CPU, us/echo | Client CPU, average cores |
| --- | ---: | ---: | ---: | ---: | ---: |
| Independent wtransport dispatcher | 495,437 | 63.228 | 120.006 | 5.482 | 3.060 |
| Direct web-transport-quinn session | 588,120 | 53.898 | 91.288 | 4.852 | 2.818 |

The rate increases 18.7%, median RTT falls 14.8%, and client CPU falls while
completing more work. Rate ranges do not overlap: independent 495,064–511,976/s,
direct 576,732–589,409/s. This is causal evidence that the client-library path
contributed to the original datagram deficit. Server CPU per completion also
changes with traffic cadence; do not call that a server-code optimization.
The direct client's p99 varies 90.378–151.656 us; three short runs do not establish
a tail-latency SLA. Its median rate remains below the first comparison's
WebSocket result, and server CPU/completion remains much higher. These runs do
not prove parity.

## Profiles and concrete source paths

Separate diagnostic invocations record **both** server and client with
`perf record -e cycles -F 499 --call-graph dwarf,8192 -p PID`; user and kernel
samples are retained. All eight recordings report zero lost samples. Reports
use self samples, not inclusive stack percentages. Symbol grouping is a
heuristic: inlining and unresolved allocator/libc symbols prevent an exact
accounting of every operation. It cannot predict the speedup from deleting a
stage. Full raw data, flat reports and call trees live in
`artifacts/webtransport/causal-profile/`.

| Server self-cycle samples | WebSocket | WT stream | WT datagram | WT datagram, direct client |
| --- | ---: | ---: | ---: | ---: |
| Kernel | 88.41% | 43.18% | 49.80% | 50.13% |
| Tokio runtime, synchronization, timers | 0.00% | 16.28% | 14.02% | 13.67% |
| Quinn I/O / connection driver | 0.00% | 6.31% | 5.57% | 5.75% |
| Quinn protocol | 0.00% | 10.63% | 9.65% | 9.64% |
| Crypto / TLS | 0.00% | 6.21% | 7.52% | 7.29% |
| Relay symbols | 8.42% | 4.60% | 4.19% | 4.27% |

The rest is Bytes storage and other/unresolved symbols. These are percentages
within different recordings, not additive percentages of a common time budget.
The datagram client profile has 25.45% in Tokio runtime/sync/timers and 3.08% in
wtransport dispatcher symbols. The direct client profile drops the Tokio group
to 15.23% and removes that dispatcher. Profile percentages alone are suggestive;
the six-run intervention above is the evidence that removing the layer helps.

Concrete costs in the pinned implementation:

1. **Driver and application handoffs.** `vendor/quinn/src/endpoint.rs::poll_socket`
   receives into reusable UDP buffers, copies received data into owned `BytesMut`,
   identifies the connection, and enqueues a `ConnectionEvent::Proto`. The separate
   `ConnectionDriver::poll` takes a connection-state lock, processes the event,
   drives transmit/timers and wakes application waiters. The application receives
   and routes, then submits output and wakes transport work again. Tokio timer
   reset, waker operations, Notify polling and the connection driver all appear
   prominently in the profiles. Native WS processes ready TCP connections on
   fixed Mio workers with direct borrowed writes where possible. This is a
   meaningful difference in architecture, beyond protocol selection. The upstream
   design discussion describes the same dispatch/copy/scheduler tradeoff:
   https://github.com/quinn-rs/quinn/issues/914 . It is supporting design context,
   not a substitute for our measurements or proof of a particular redesign's gain.
2. **Datagram framing forces contiguous ownership.** In
   `src/webtransport.rs::Output::submit`, prefix and shared payload are copied to
   scratch, then copied into a new `Bytes`. In pinned
   `web-transport-quinn 0.13.3::Session::send_datagram`, another allocation and copy
   prepends the HTTP/3 WebTransport session ID. Quinn subsequently serializes the
   packet into its transmit buffer and protects it. Thus this output path has
   three application/library payload copy passes before packet construction;
   it is not a shared-body datagram send. Quinn's API accepts one contiguous
   `Bytes`, and the wrapper explicitly references the missing vectored API:
   https://github.com/quinn-rs/quinn/issues/1724 . Source proves the copies exist;
   this profile does **not** prove they dominate the remaining time.
3. **Unreliable payload does not remove QUIC machinery.** Datagram packets still
   undergo encryption/decryption, packet numbering/protection, ACK handling,
   congestion/pacing logic and timers. Payload retransmission and stream assembly
   are avoided; the entire connection pipeline is not bypassed. AES-GCM and Quinn
   packet processing are visible, but sampled crypto alone is not the dominant
   category. No benchmark disables authentication or crypto.
4. **Per-connection memory architecture.** Reinspection of the prior 8,194-session
   replacement-allocator ready heap confirms 253,322,716 requested live bytes.
   Major allocations include 7,088 bytes per Quinn connection inner allocation,
   2,048 per application task, four 576-byte packet-key allocations per connection,
   1,368 per rustls server session, plus stream/packet tracking, buffers, maps,
   HTTP metadata and scheduling storage. The latest ordinary 100k report is
   3,050.28 MiB server RSS versus the historical plaintext WS control's
   112.83 MiB. Those capacity figures were not rerun in this profiling pass;
   the heap is from the preceding direct-chunks checkpoint, not a current
   allocator/RSS measurement. TCP kernel memory is excluded from WS RSS.
   Nevertheless this identifies concrete user-space storage costs rather than
   attributing the memory gap to the datagram payload itself. High post-close RSS
   alone does not prove live-session leakage.

## Resulting engineering decision

Prioritize reducing endpoint/connection/application scheduling and ownership
boundaries, and eliminating the datagram framing copies. A vectored datagram
representation or a single owned construction with header headroom can avoid
some copies; protocol-level integration with the existing relay worker model
could address larger scheduling costs. Both require controlled before/after
whole-workload verification; no unmeasured fork or new arbitrary buffer limit
has been adopted. Client/library measurements must stay separate from server
improvements and actual browser behavior.

The most defensible conclusion is that the **current stack and integration**
carry substantial overhead, with one client-library cause experimentally
confirmed. It would be inaccurate to attribute the entire deficit to encryption,
to datagrams as a concept, or to one allocator/copy function. The separate
contributions of server scheduling, copying and mandatory QUIC work still need
controlled interventions before assigning a precise percentage to each.
Throughput and memory parity remain required; the pivot is not accepted.

## Reproduction and artifacts

The companion `webtransport-causal-profile-2026-10-21.json` retains every ordinary
sample, ranges, grouped profiles and binary/source hashes. Local runner:
`artifacts/webtransport/causal-profile.py`. Its default performs the three-way
ordinary runs and separate profiles; `CAUSAL_OUT`, `CAUSAL_CLIENT`,
`CAUSAL_MODES`, `CAUSAL_REPEATS`, `CAUSAL_PROFILE` select output/client/modes.
Build the original with `cargo build --release --example transport_probe`.
To rebuild the isolated direct diagnostic, copy its archived source into
`examples/transport_probe_direct.rs`, build that release example, and remove
only that temporary copy after archiving the executable. The ordinary production
source was not modified by this investigation. Profiles run with privileged
perf attachment while server/client processes retain normal user ownership.
