# Changed-deadline timer update screen

Rejected: replacing removal/reinsertion with an in-place deadline change and one directional heap repair does not show a consistent local benefit across deadline patterns. Keep the selected timer implementation and its existing identical-deadline early return. No runtime code is integrated and no service is replaced.

Inspection first ruled out adding another socket writer fast path: the existing output path already coalesces readiness-interest changes, and the saved mixed-workload native profile assigns only 1.59% self sample share to write_batch. That profile is in the [lock-scope screen](../2026-10-18-fanout-unlock-screen/README.md). Sample attribution is not a latency decomposition. The next isolated question was whether changing a live timer in place offers a sufficiently consistent benefit to justify a relay experiment.

The candidate changes only Timers::set for an existing live deadline. It writes the new Instant and moves the entry up for an earlier deadline or down for a later deadline. Identical deadlines still return immediately. Insertion, cancellation, due-pop, slot rekeying, heap representation and storage owners remain unchanged. Both exact sources are retained. This prototype does not add a collection, lock, timer entry or allocation scheme.

Each fresh standalone release process initializes 100,000 timers, then measures eight rounds of 100,000 updates per case. Setup and draining are outside timing. Cases postpone deadlines, advance deadlines, use deterministic mixed deadlines, or repeat unchanged deadlines. The harness uses black_box on slots, then drains every timer and checks the count. Existing independent timer tests check reference ordering, cancellation, equal-deadline order, sparse rekeying and slot reuse. These are timers, not connected users; there is no network traffic in this screen.

Four unpinned processes run control/candidate/candidate/control. A separate four-process repeat uses the same order with each process pinned to CPU 0. Builds, tests and measurements run sequentially. All raw times are retained rather than dropping the slower candidate repeat. Pinned nanoseconds per update, computed from 800,000 updates/case:

| Case | Control repeats | Candidate repeats |
| --- | --- | --- |
| Postpone | 72.63, 74.18 | 71.80, 109.30 |
| Advance | 6.95, 7.01 | 4.35, 4.45 |
| Mixed | 22.73, 22.99 | 24.97, 21.29 |
| Unchanged | 2.69, 2.73 | 2.82, 2.85 |

The earlier-deadline case is faster in both pinned candidate repeats, while mixed results vary and repeated postponement has a large slower candidate repeat. The unpinned postponement cases also favor control. This is sufficient to leave the specialized update unselected, without a full-population follow-up. It does not establish that the candidate would worsen whole-relay CPU or latency. Code layout, operating-system activity and cache state can affect these tiny observations; no significance or broad speed claim is made. The existing fixed-deadline path stays simple and unchanged.

The candidate passes both timer reference-model/rekey tests in 0.00 seconds. A full Cargo build or network acceptance campaign is intentionally not run for an unselected isolated timer prototype. All eight measured process workloads together take under two seconds, plus compilation, setup/draining and test time. Reproduce by placing the four raw Rust sources together, compiling each *-bench.rs with rustc --edition 2024 -O, and running control/candidate/candidate/control, optionally via taskset -c 0. The recorded commands and compiler/binary identities are in builds.json. Run verify.py for an offline audit.

The source base is 65b1671. Only this evidence and the status record are selected changes. Existing manual services and all three pre-existing source overlays remain untouched. The wider goal of reducing fanout-tail latency is still unresolved.
