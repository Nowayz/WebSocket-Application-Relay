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

Known limits include synchronous queue-pressure fallback and pending global-view
scans during observer revocation. Faults and failed gates belong in the evidence. Source and
client digests, commands, affinity, measurement windows and cleanup semantics
must accompany performance claims. See [the guide](performance-guide.md) and
[historical findings](performance-history.md).
