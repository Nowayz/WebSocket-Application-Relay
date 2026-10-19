# Retirement queue-pressure diagnostic and unselected cursor screen

**The cursor implementation is unselected. The native runtime remains the shared retirement FIFO selected in `d7a5e95`.** This work retains a reproducible queue-pressure mode, failure-time metrics and process-cleanup results, an additional room-recreation regression, and operator guidance. It does not replace the selected runtime with an architectural experiment on the strength of one favorable case.

## What the focused pressure test found

The selected FIFO charges a complete membership snapshot for each active large-room departure. On the measured 64-bit host, 100,000 recipient handles alone need about 800,000 bytes, with vector/job overhead and frame credit in addition. Accounting uses view capacity, not just live member count, and conservatively charges each active job even when a view is shared. Several concurrent large departures can therefore consume substantial queue credit before recipient frames are enqueued.

A deliberately constrained fixture keeps 100,000 recipients in one room, eight self-echo witnesses in unrelated rooms, two large-room sources and two singleton controls. It requests a 256 KiB local/shared application queue budget and 64 KiB maximum messages. The payloads are small; every departure notification is verified. This is an intentional stress profile, not the published 100k operating profile: that profile uses 256 MiB shared queue credit and caps ordinary rooms at 1,024 members. The diagnostic raises the room/admission ceilings for its particular question.

| Tight-budget run | Selected FIFO | Cursor prototype |
| --- | --- | --- |
| First | Fails waiting for a departure notice; 81,548 global-pressure rejections | Passes all 200,001 notices; zero global pressure |
| Repeat | Fails waiting for a departure notice; 72,283 global-pressure rejections | Passes all 200,001 notices; zero global pressure |

The counter includes metadata and frame admission failures; it is not a count of missing notices. Both failing controls time out in the exact notification check, so these are delivery failures rather than merely a strict counter gate. Routing copied bytes stay zero in all four runs. The failure mechanism concerns queue credit and pacing, not added payload copies. It does not establish that the ordinary 100k deployment profile fails.

The new `--queue-budget-bytes` option makes this pressure case directly reproducible through `scripts/departure-fairness.py`. It records the actual server flags. On failure, the fixture captures live metrics before terminating its disposable server, then records the server exit and whether forced killing was needed. Normal/default mode retains the previous server configuration. The pressure preset requires room for its configured 65,536-byte maximum message plus the existing 10-byte envelope allowance; invalid smaller values are rejected before opening output or starting a process.

The final fixture was checked against the unchanged selected executable: default 8,192-recipient mode passes; 100k/256 KiB mode exposes the expected delivery failure and saves `failure_metrics`, including 81,561 global-pressure rejections in that run. Both owned servers exit normally with code 0, without forced killing. Separate CLI checks verify the minimum-budget guard and preservation of existing evidence. A failed pressure result remains `passed: false` with CLI exit 1; the validation wrapper does not turn it into successful relay acceptance.

## Candidate and why it stays unselected

The prototype replaces each channel's membership hash table with an ordered admission-key tree. Ordinary application multicast keeps its cached shared view. Room retirement instead stores the room name, the admission-key cutoff, a cursor and a charged shared frame. It captures at most 1,024 peer handles into reusable stack space for a turn, releases the registry guard, then submits output. A later join has a key beyond the cutoff; leaving members are already ineligible; a deleted and recreated room cannot inherit the old work. Global disconnect-observer retirement retains its existing snapshot semantics.

This removes population-sized room-retirement snapshots and makes each queued room job's metadata constant-sized. It also changes a central index and adds a registry-read/capture phase to retirement turns. Passing the tight-budget case alone cannot establish the combined latency, traffic and memory tradeoff.

Four fresh two-second mixed phases use control/candidate/candidate/control order, the same frozen clients and disjoint fixed CPU masks: 100,000 capacity users, 8,192 additional fanout readers, window 4, 32 private pairs, two inspectors, eight churn workers at 32 cycles/second each, and eight offered fanout windows/second.

| Run | Server CPU-seconds | Verified sidecar private messages/s | Worst capacity-user RTT | Worst fanout RTT | Ready RSS |
| --- | ---: | ---: | ---: | ---: | ---: |
| Control first | 9.28 | 1,197,682 | 4.557 ms | 64.161 ms | 117,284 KiB |
| Candidate first | 9.30 | 1,067,887 | 4.812 ms | 72.925 ms | 120,160 KiB |
| Candidate repeat | 9.29 | 1,063,147 | 5.879 ms | 114.284 ms | 113,624 KiB |
| Control last | 9.38 | 1,091,716 | 8.482 ms | 70.614 ms | 116,440 KiB |

CPU is flat. The candidate's private rates are lower in both mixed runs, and one fanout maximum is considerably higher. Capacity-user ranges overlap, and RSS is too variable here to support a memory-saving claim. These finite samples do not prove that the tree index causes every difference; they do not establish the desired combined win either. The source and binary are preserved for diagnosis, without adding the index to the platform.

A separate private-only screen narrows the attribution: 64 pairs, four native workers and eight client workers, no fanout/inspection/churn, four fresh two-second ABBA cases with disjoint pinned masks.

| Run | Verified private messages/s | Worst observed RTT | Server CPU-seconds |
| --- | ---: | ---: | ---: |
| Control first | 2,460,590 | 1.358 ms | 5.45 |
| Candidate first | 2,335,446 | 3.898 ms | 5.34 |
| Candidate repeat | 2,458,038 | 1.309 ms | 5.46 |
| Control last | 2,450,304 | 1.611 ms | 5.47 |

Those rates overlap. This screen does not demonstrate a consistent basic-send regression and does not resolve the mixed-load result. Normal-budget departure repeats also overlap: the control's per-event worst witness RTT spans 2.070–6.404 ms; the candidate spans 1.522–7.265 ms. The candidate consistently acknowledges the first large departure sooner, but acknowledgments alone are not the user-responsiveness criterion.

## Scope and validation

Every accepted mixed case completes all 16 offered windows, verifies 524,288 timed fanout deliveries and 512 churn cycles, and checks 750,000 capacity departure notifications during cleanup. Both clients and server exit 0. Final sessions, connections, queued bytes, variables, read-buffer reservations and native slots are zero; routing copied bytes are zero. Default-budget departure cases each verify 200,001 notices and exact witness payloads. Probe RTTs include client/kernel scheduling and loopback traffic, not an isolated relay hop; sparse user samples and a few maxima do not establish long-term tails.

The prototype passes 145 library tests, Clippy with warnings denied, and formatting. Its pressure-refusal test was adjusted to fill actual remaining credit: a constant-sized job should no longer fail merely because membership is large. Its membership test replaces hash-capacity assertions with removed-peer reachability checks appropriate to a different container. These changes are part of the **unselected patch**.

The selected source retains only an additional behavioral regression: delete an entire room while departure work is pending, recreate the same name, then prove the new session receives no older notices and all reservations are released. It uses the selected implementation's existing snapshot machinery; it makes no cursor/index assumption. The selected library now passes 145 tests in 10.03 seconds, Clippy and formatting. Production Rust source and the selected executable are unchanged.

All steady traffic phases are two seconds. Each complete 100k mixed command takes about 34 seconds because setup, proofs and millions of checked teardown notices are outside measurement. Focused default-departure commands take about 7–9 seconds. A failing tight-budget case adds its five-second receive deadline. No build, diagnostic profile or other benchmark overlaps timed traffic. No multi-hour or long soak test was used.

## Reproduce and provenance

From the attached checkout, use the explicit selected executable and a fresh output path. This is an **expected failing negative control** for the current relay:

```sh
taskset -c 8-15,24-31 python scripts/departure-fairness.py \
  --server /home/phil/dev/war_next/artifacts/candidates/departure-fifo-20261019-selected/war-relay \
  --audience 100000 --queue-budget-bytes 262144 \
  --output /tmp/departure-pressure-fresh.json
```

Omit `--queue-budget-bytes` for the existing default-budget behavior. Neither mode changes existing services or global machine settings.

Source parent: `d7a5e95`; attached checkout: `/home/phil/.codex/worktrees/departure-quantum/war_next`, branch `codex/departure-quantum`. Selected executable SHA-256: `46f3765fa8a0ee879c33efd79648707fad81f118dd77ba543cf629ef95798dbd`. **Unselected** candidate: `/home/phil/dev/war_next/artifacts/candidates/departure-cursor-20261019-candidate/war-relay`, SHA-256 `fa0f26a0d54d17cf26292c938e81436e069a138dc81cebb77500ae4b3005884b`.

Build receipts, base/candidate source snapshots, `UNSELECTED-candidate.patch`, exact runner sources and compressed receipts are retained. The initial pressure comparisons use `pressure-wrapper.py` to append the recorded flags; their original `server_command` is supplemented by `pressure_options`. The final canonical fixture records those flags directly and is saved as `fixture.py`. Mixed receipts retain the generator's per-user CSV hashes and derived statistics; the complete CSVs remain under the original local artifact stems, and this report does not independently recompute their per-user aggregates. `analyze.py` recomputes the displayed comparison from receipt data. `verify.py` checks identities, source reconstruction, acceptance/negative gates and immutable evidence without launching traffic; `--check-local` additionally verifies current production source, fixture, regression and both executables.

The original checkout's three dirty overlays and all manually running services are preserved. No prototype index change is committed to the selected runtime.
