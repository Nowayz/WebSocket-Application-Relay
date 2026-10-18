import json,statistics
from pathlib import Path
s=json.loads(Path('artifacts/mixed100k-final-summary.json').read_text());m=s['medians'];impl=json.loads(Path('artifacts/mixed100k-implementation.json').read_text());quiet=json.loads(Path('artifacts/mixed100k-quiet-final.json').read_text())
labels={'before':'Previous Rust','after':'Selected Rust','cpp':'Modern C++'}
lines=[]
for k in ['before','after','cpp']:
 r=m[k];worst=max(x['latency']['p99_ms']['max'] for x in s['runs'] if x['kind']==k)
 lines.append(f"| {labels[k]} | {r['median_user_p99_ms']:.3f} | {r['p99_user_p99_ms']:.3f} | {worst:.3f} | {r['worst_probe_ms']:.3f} | {r['cpu_seconds']:.2f} | {r['peak_sampled_rss_mib']:.2f} |")
trials=[]
for r in s['runs']:
 trials.append(f"| {labels[r['kind']]} {r['repeat']} | {r['latency']['p99_ms']['median']:.3f} | {r['latency']['p99_ms']['p99']:.3f} | {r['latency']['p99_ms']['max']:.3f} | {r['fanout_windows']['completed']}/480 | {r['fanout']['p99_upper_ms']:.3f} | {r['fanout']['max_ms']:.3f} | {r['sidecar_cleanup_seconds']:.2f} |")
qrows=[]
for r in quiet['runs']:
 qrows.append(f"| {'Before' if r['kind']=='rust' else 'Selected'} {r['repeat']} | {r['server_cpu_seconds']:.2f} | {r['population_rss_kib']/1024:.2f} | {r['p99_ms']['median']:.3f} | {r['p99_ms']['p99']:.3f} | {r['p99_ms']['max']:.3f} |")
text=f'''# 100,000-user mixed-load fairness (2026-10-18)

Selected runtime commit `{impl['runtime_revision']}`; executable SHA-256 `{impl['binary_sha256']}`. This is a latency-first change to the previous selected owner-worker Rust relay, not a runtime or allocator replacement. Original C++ and accepted modern C++ are unchanged. Existing observer-index and C++ counter-layout experiments remain preserved and unselected.

At the same offered mixed load, the median user's p99 falls from **11.99 ms to 2.03 ms**. The 99th percentile across the 100,000 users' individual p99 values falls from **27.29 ms to 4.76 ms**, versus **58.55 ms** for modern C++. Every stable user contributes 120 checked probes per trial. This is an observed finite population distribution, not a hard service-level guarantee.

## Primary comparison

Two fresh 30-second runs per runtime, in order before / selected / C++ / C++ / selected / before. The relay owns eight physical cores. The capacity client owns four different physical cores with both SMT siblings; the fanout client owns the remaining four cores with both siblings. No server/client physical core overlaps. Full topology and actual server-thread affinities are saved. No build, profiler or other benchmark overlaps ordinary measurement.

- 100,000 stable users in 6,250 rooms of 16; every user sends a checked 1 KiB private probe four times per second: **12 million timed probes per run**.
- An additional room has 8,192 verifying readers and one publisher. Four-message windows are offered at 16 Hz: **524,288 intended fanout deliveries/s**, with one complete reader/message matrix acknowledged before the next window.
- Eight independent churn workers each offer four complete join / self-route / exact Close / EOF cycles per second: **960 verified cycles per trial**.
- Two authenticated CBOR inspectors query every 10 ms and verify the full stable room census, ordering, membership and permitted transient rooms. A retained authenticated probe validates setup and cleanup.
- There is no extra saturated private pair in this primary cohort. Total stable connections at the measured gate are **108,196**, plus up to eight temporary churn users.
- Native plaintext TCP/WebSocket/CBOR, heartbeat and per-client rate policies disabled identically; requested kernel buffers 16 KiB send / 64 KiB receive. Rust application queues are 64 messages, 256 KiB/peer and 256 MiB globally; decoder staging has a 256 MiB aggregate limit. C++ has the same applicable socket/queue settings but retains its documented operating-policy gaps, including aggregate decoder-budget enforcement.

Each received sender, complete encoded value and FIFO position is checked. Every stable connection routes a proof before and after measurement. All submitted private probes complete; no stable user or inconvenient observation is omitted. Setup, proof/retirement scans, per-user sorting and CSV output are outside the timed interval. Server CPU is sampled between the private client's ready/measured gates; it is whole-server CPU under all competing work, not isolated forwarding cost.

The following are medians of the two trial summaries, except the two worst-observed columns, which are maxima across both trials. RSS excludes clients and kernel socket memory.

| Runtime | Median user p99, ms | p99 across users' p99, ms | Worst user p99, ms | Worst probe, ms | Server CPU-s / 30 s | Sampled peak RSS, MiB |
|---|---:|---:|---:|---:|---:|---:|
{chr(10).join(lines)}

![Every user and the slowest one percent](raw/mixed100k-user-fairness.png)

The blue curve includes all 100,000 private users, weighted equally; the right panel expands the slowest 1%. Each user's p99 is the nearest-rank 99th percentile of its own 120 timed send-to-receive samples. The 99th percentile of those 100,000 values describes population fairness; it is not a pooled packet p99. Socket, client scheduling and 16-user ring batching contribute to these end-to-end measurements. They are not isolated server-hop latency.

In selected Rust, only 435 and 692 users have individual p99 above 5 ms, compared with 81,783 and 89,773 before (the exact counts are in the summary). The worst selected user p99 is 6.824 ms. A threshold such as 5 ms here is a way to inspect the measured distribution, not an enforced product limit.

![Time cohorts](raw/mixed100k-timeline.png)

Each time cohort contains the probes intended for that second. Intended-to-receive includes generator scheduling delay. The two panels share a vertical scale. Neither a start/end histogram nor a selected subset of users is substituted for these full distributions.

## The scheduling and ownership change

The previous multicast transport already queues shared bodies to socket owners and uses bounded vectored writes. However, one publisher still synchronously visits its entire audience before the event loop can serve another connection. Output-only wakeups also enter the decoder and repeatedly acquire the socket mutex. A diagnostic profile and disassembly locate substantial work at socket-state lock acquisitions, registry/peer reference operations and wake/scheduling bookkeeping; routing-copy counters are zero.

The selected implementation:

1. **Bounds ordinary-room submission to 1,024 recipient visits per source turn.** The source retains one immutable membership view and one shared body. It completes that message before reading its next input frame, preserving source FIFO. It then yields even after the last chunk, giving other ready sockets a scheduling opportunity. Small rooms retain complete immediate submission. Scopes with global observers and global broadcasts retain their existing permission barriers and complete-message submission.
2. **Distinguishes application-output wakeups from input/timer readiness.** Eligible output-only turns flush the existing queue under one state lock, without entering the decoder or swapping its receive storage. A real readiness event upgrades an already queued turn. Fatal/Close/control state, expired output/idle policy, due Ping/Pong/reclamation and shutdown return to the full state machine. A write-quantum continuation retains its output-only reason.
3. **Queues departure notifications to socket owners.** They share the same immutable body and existing FIFO/quota mechanism as data multicast. The previous path attempted every departure write on the retiring worker while holding membership state. The exact, notification-heavy fanout cleanup falls from a median **131.54 s to 28.45 s**; modern C++ is 29.05 s. This cleanup is outside the data timing, and all 33 million-plus expected room-departure receipts remain checked.
4. **Charges unfinished work.** The source body uses the existing per-peer/global reservation until completion or cancellation. Retained membership-view capacity, including a fixed Arc/vector allowance, is conservatively charged per active publisher to the global queue budget, even when publishers share a view. Churn cannot accumulate uncharged generations of old views behind deferred cursors. One lazily allocated continuation box is reused per large publisher; empty boxes retain no payload or peer view and are bounded by admitted connections. The write deadline includes unfinished submission.

The quantum bounds recipient **submission**, not all work or wall-clock duration.
Creating or refreshing the existing cached membership view still captures the
room under a registry read guard and is linear in room size; stable views are
reused. Query/decoder work and global/observer barriers also retain their own
bounds. The implementation does not turn the event loop into a hard real-time
scheduler.

The Frame representation and vendored decoder are unchanged. No new dependency or unsafe code is introduced. Payloads remain borrowed/shared; headers and bodies are submitted as vectored slices. Pointer/lifetime tests establish that the forwarded body is the original received slice. All Rust routing-copy counters remain exactly zero in the primary, quiet, corpus and pressure runs. This describes userspace routing copies; ordinary TCP kernel copies still exist.

Sampled logical queued pressure falls from about **10.02 MiB to 0.78 MiB**, despite the new snapshot accounting. Both runtimes keep their existing global queue admission limit. This is neither an RSS cap nor a count of unique physical payload bytes: shared bodies can have multiple logical delivery reservations, and unfinished work now contributes body/view credit.

## Fanout and quiet-load tradeoffs

The primary cohort has the same *offered* fanout schedule. The client skips a missed timer tick rather than generating an unbounded catch-up burst. Previous Rust completes 480 and 479 windows; selected Rust completes 479 in each trial; C++ completes 480 in each. Every submitted window is fully verified. Consequently, actual fanout rate is approximately 523k–524k deliveries/s; the private workload is exactly 12 million probes in every trial.

Fanout latency is measured from the start of a four-message window and includes client verification/windowing. Histogram p99 values below are **bucket upper bounds**, while maxima are exact observed durations. The larger upper bucket does not establish that true p99 doubled. Selected Rust improves private-user fairness and inspection responsiveness, but does not improve every audience-window tail.

| Trial | Median user p99, ms | p99 across users' p99, ms | Worst user p99, ms | Completed windows | Fanout p99 upper, ms | Fanout max, ms | Checked cleanup, s |
|---|---:|---:|---:|---:|---:|---:|---:|---:|
{chr(10).join(trials)}

Two selected inspection p99 bounds are 4.194 ms; both previous/C++ references are 16.777 ms. Selected churn p99 bounds are 16.777 / 8.389 ms, versus previous 16.777 / 67.109 and C++ 33.554 / 67.109. Mixed server CPU falls about 3.8%; sampled RSS is similar at roughly 115 MiB, while C++ remains smaller at about 70 MiB.

A four-run quiet control retains the exact frozen private client, 100,000 users, 400k probes/s, 120 samples/user, matched kernel buffers and eight server cores. With no sidecar it gives the client eight separate physical cores, so its absolute latency is a separate workload from the mixed case. Fresh order is before / selected / selected / before.

| Quiet trial | CPU-s | Population RSS, MiB | Median user p99, ms | p99 across users' p99, ms | Worst user p99, ms |
|---|---:|---:|---:|---:|---:|
{chr(10).join(qrows)}

Total CPU for the two quiet trials is 103.72 s before and 105.11 s selected, **+1.34%**. Median user p99 changes from approximately 0.895 to 0.924 ms. Population RSS is about 1 MiB lower; tail rankings vary. This small quiet-path cost is retained for the much larger stressed-population gain. It is not reported as a universal private-path speedup.

## Alternatives and diagnostic limits

- The preserved observer-index experiment combined with owner fanout does not improve its first 1,024-observer controlled trial; it stays unselected. The output-only-only experiment reduces CPU but has mixed private tails.
- A 256-visit room continuation improves stressed private latency but worsens fanout-window tails and completes fewer saturated/16-Hz windows. It is retained as an intermediate experiment.
- A 4,096-visit follow-up restores the 67.109 ms fanout p99 upper bucket but raises the private population's p99 threshold to 18.313 ms and worst user p99 to 20.325 ms. It is rejected in favor of the measured 1,024-visit balance. These follow-ups are separate cohorts, not silently pooled into the primary medians.
- Early harness runs put the sidecar on SMT siblings of server cores. They are preserved as placement diagnostics and excluded from the primary comparison. The first two large-audience runs were interrupted during serial quadratic departure verification; their timed observations remain diagnostic, not fully accepted trials. Bounded batches of 32 now amortize cleanup reads while checking the exact same departure sequence.
- One auxiliary pressure helper incorrectly expected the final slow-consumer counter to equal eight after 1,021 healthy readers simultaneously close. The gated correction proves exactly eight live-phase slow consumers; the subsequent departure-only close burst creates 954 additional capacity markings while all intended Close/EOF exchanges complete. The initial failed assertion and corrected run are both retained. Such cumulative markings describe overload intent and can include a closing peer whose normal Close is subsequently honored.
- A separate baseline cycles:user profile is an earlier saturated observer workload. The selected profile uses the final paced room workload. Their symbol shares are diagnostic, not a matched before/after CPU decomposition. Selected sampling records zero lost samples; remaining user-cycle concentrations include connection driving, receive/lookup/refcount work and queue/wakeup bookkeeping. Neither profile measures off-CPU or complete kernel cost.
- The campaign controller was paused between timed work to build the separate 4,096 candidate. Its wall-duration field for one selected run includes that controller wait. Client event timestamps, CPU gates, all latency samples and measured traffic are unaffected; cleanup duration is computed from client event timestamps.

## Correctness, pressure and adoption

Selected source passes **77 library tests**, strict all-target Clippy and formatting; the inspection client's three corruption oracles pass. The integrated working tree passes **80 library tests** plus the three inspection tests and strict Clippy with the preserved observer-index overlay, using a separate target directory to force a fresh build rather than sharing copied-manifest test artifacts. Tests force short writes and WouldBlock, verify body pointers and FIFO, exercise source cancellation and late join/leave during a continuation, reuse its only allocation, reject unbudgeted retained-view metadata, and verify an already queued output wake upgrades to input processing.

The independent wire corpus passes **776 cases / 2,311 exchanges**. Its healthy witnesses, accepted/rejected values, fragmented/pipelined inputs, exact counters and resource release are checked.

The ten-second unread-reader test verifies **6,102,736 healthy deliveries**, all eight unique departure notifications and all eight expected 1013/transport-retirement outcomes. The gated 1,030-client case verifies **8,595,799 deliveries to 1,021 healthy readers** and 8,168 expected unread-peer departure receipts. Exactly eight peers are marked slow before teardown; global pressure, protocol/write errors and routing copies are zero. It crosses the room-continuation threshold before unread recipients leave. Connection, session, queue-capacity, queued-byte, read-buffer and variable gauges return to zero after both tests.

The six primary trials verify **72 million timed private probes**; the four quiet trials verify another **48 million**. Normal setup/postcheck proof traffic, full audience matrices and departure verification add further checked exchanges. Descriptor counts return to the fresh-server baseline and all relevant final resource gauges retire. Profile timings and pressure throughput are not substituted for ordinary comparisons.

The local native service was quiescent before replacement. Its executable hash, readiness, private and room delivery, departure notice, two exact normal Close/EOF exchanges and zero final resource gauges pass after adoption. `raw/mixed100k-adoption.json` records its PID and the runtime revision. Credentials are preserved in memory and not archived. The implementation commit contains the exact runtime source snapshot used for the selected binary; the preservation proof shows the existing unrelated C++ and observer-index changes remain intact.

These are finite plaintext loopback measurements on this host. They do not establish equal WAN/TLS latency, a hard worst-case deadline, a 100,000-member broadcast audience or long-duration production stability for this new revision. The ordinary-room continuation intentionally does not split global/observer dispatch across permission barriers. Multi-message fanout batching and those barrier-preserving paths remain possible future work; the measured audience-window tradeoff is explicit above.

## Reproduce and audit

Primary current-host recipe (use a new output path for each fresh run):

```sh
python3 scripts/mixed-capacity.py \\
  --server artifacts/candidates/mixed-capacity-final/war-relay \\
  --scope room --pairs 0 --churn-cycles-per-second 4 \\
  --fanout-windows-per-second 16 \\
  --output artifacts/mixed-reproduction.json
```

The helper records the exact server/client commands, all binary hashes, affinity/topology, every gate, monitoring snapshots, counters and per-user CSV hashes. Its CPU sets describe this 16-core/32-thread host, not a portable tuning prescription. Use `--kind cpp --server artifacts/candidates/cpp-accounting-layout-control/war-relay-cpp` for the accepted C++ reference. `--profile` denotes a separate diagnostic run. The frozen primary runner is archived in `raw/mixed100k-final-runner.py`; the committed helper additionally returns failure if final shutdown exits are nonzero. The smoke check validates that helper's final form.

`source/after` is the selected runtime snapshot; `source/before` is the previous selected version. `source/client` contains the frozen checked inspection client. `source/capacity-client` pins the exact older capacity example and its build metadata; it differs from the later working-tree example and is not reconstructed by assuming current source matches the executable. `source/vendor` is the unchanged transport tree shared by Rust snapshots; place it as `vendor/` beside a snapshot's Cargo.toml to rebuild. Cargo.lock and compiler/build records are pinned. `source/runtime.patch`, rejected snapshots, raw JSON/traces, all decoded-CSV identities, plots and verification helpers are retained. Executables and large raw perf.data files stay in local artifacts; their hashes/profile reports are archived. The prior evidence dependencies are pinned separately.

Run the archive verifier from the repository:

```sh
python3 docs/benchmarks/2026-10-18-mixed-100k-fairness/verify.py
```
'''
# Preserve exact per-trial counts rather than relying on hand-copied examples.
a=[r for r in s['runs'] if r['kind']=='after'];b=[r for r in s['runs'] if r['kind']=='before']
text=text.replace('only 435 and 692 users',f"only {a[0]['users_with_p99_over_5ms']:,} and {a[1]['users_with_p99_over_5ms']:,} users").replace('81,783 and 89,773',f"{b[0]['users_with_p99_over_5ms']:,} and {b[1]['users_with_p99_over_5ms']:,}")
Path('artifacts/mixed100k-report.md').write_text(text)
