# Native stability checks

All supported tests exercise Rust/CBOR. Use short explicit phases; longer soaks
are justified by a specific unresolved retention or lifecycle concern.

`cargo test --locked --all-targets` covers protocol validation, private/global
routing, membership snapshots, ID takeover, variables, queue/staging budgets,
partial writes, FIFO, timer/ready fairness, admission, shutdown and physical
transport ownership. The codec patches preserve published owners while staging
storage grows or is reclaimed. `scripts/check-vendor.py` verifies those Rust
sources against pinned package archives and the recorded patch.

The native registration-failure test pre-registers a real descriptor and attempts
a second registration through the ordinary Socket constructor. It checks failure,
TCP EOF and release of physical admission/queue credit. This replaces the useful
registration-cleanup check from the retired C preload helper without adding
unsafe interception or requiring a C compiler. It does not claim to intercept
all failing socket-option syscalls. The real `scripts/accept-pressure.py` runner
uses child-only descriptor limits and checks retry pacing, healthy traffic,
queued-flow recovery, resource cleanup and shutdown with one/four workers.

`bash scripts/check-js.sh` rebuilds the native server, starts a disposable fixture,
runs mock-protocol and real WebSocket JavaScript checks, and requires six resource
gauges to return to zero. It resets deployment-only WAR environment settings
without logging secret values. `scripts/test-js-build-freshness.py` checks source
fidelity, bounded readiness failure and fixture-process cleanup. The browser
check runs the generated CBOR bundle and verifies structured values, bigint IDs,
variables and cleanup. TypeScript contracts are checked separately.

Capacity/mixed checks must verify exact payloads and sender IDs, ordered departures,
completed offered fanout windows, normal Close/EOF and final reservations. A server
exit status alone does not prove clean client closure. A final RSS value alone
does not prove a logical leak. Report decoder/output/variable gauges and native
arena capacity alongside process memory, accounting for intentional warm storage.

Observer-barrier regressions cover partially visited jobs, independent observers,
re-subscription before completion, publication after an emptied queue, counter
exhaustion and a ready full one-slot output queue. The native event fixture checks
old notices before acknowledgments and all audience departure/Close receipts.
Shared snapshot checks also cover reuse across pending jobs, membership
invalidation, release on the last owning job, refusal when a view cannot fit,
and multiple departures fitting one shared view within a fixed byte budget.
Observer lookup checks distinguish equal-sized captured generations where the
peer is absent or occupies a different position, and keep each job's visited
cursor independent when several jobs share a view.
Retirement budget checks cover zero and differently sized turns, exact visit
bounds, cross-turn FIFO and release of all queued credit after completion.
Cutoff checks cover 128/129 recipients in ordinary rooms and disconnect audiences,
FIFO while crossing the cutoff, metadata refusal there, and avoiding a captured
room view for outputs that cannot drive deferred work. The idle-population
fixture verifies declared startup counts and every retained shutdown Close.
Shutdown cache tests distinguish unique and externally captured views, and verify
that reversible drain keeps the live view consistent. The native shutdown fixture
verifies a priming broadcast before checking every Close `1001` and TCP EOF;
terminal shutdown retains its existing flush-and-retire policy without awaiting
peer acknowledgments.

Fallback checks hold an actual source output reservation, verify that disjoint
room jobs stay deferred and preserve FIFO for a global observer shared by both
departures. The native fixture forces that local-budget condition with a blocked
partial echo, then verifies the healthy notice matrix and every audience Close/EOF.
It requires exactly one cancelled write for the administratively removed source.

Global-broadcast checks cover captured owner-view reuse and invalidation, late
joins, ID takeover, stale-session removal, bounded native turns, cross-turn FIFO,
cancellation and storage retirement before credit return. The native global
fixture verifies independent join/echo/Close probes after publication, paginated
membership gates, exact per-reader delivery coverage and final resource totals.

The observer-message fixture checks both message classes with a one-slot
output limit, exact publication-before-unsubscribe-ACK ordering, immediate
re-subscription, later windows and every retained shutdown Close/EOF. Independent
witnesses and joins expose scheduling stalls. Receipt tests reject reused paths
and dangling symlinks before launching a server and verify failed-startup cleanup.
The [observer-message baseline](observer-messages.md) distinguishes live resource
gates from terminal shutdown evidence. Native observer publications now yield
within a message. Deterministic checks pause a job during output submission,
prove that membership is unlocked and race revocation against progress. They
also cover cross-class watermarks, maximum publication sequence, cancellation,
late subscribers, ID takeover and stale registration removal. Combined room and
observer checks require one body reservation, pointer-identical payloads, and
observer revocation while room traversal remains pending. The native combined
case checks 2,048 room recipients plus 2,048 observers with one output slot and
a 2 KiB local byte budget. Private-primary submission remains ahead of a refused
observer snapshot. Two existing asynchronous checks now await worker scratch
reclamation and batched input-statistic publication before asserting exact totals.

Known limits include cold recipient-view capture/invalidation,
distinct captured-view scans during observer revocation and queue-pressure fallback for overlapping channels
and pending global-view scans during observer revocation. Faults and failed gates
belong in the evidence. Source and
client digests, commands, affinity, measurement windows and cleanup semantics
must accompany performance claims. See [the guide](performance-guide.md) and
[historical findings](performance-history.md).
