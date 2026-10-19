# Correct process CPU accounting and expose user/kernel time

Selected: share one robust /proc/PID/stat CPU parser between the common benchmark helper and mixed-capacity snapshots. The common helper previously split the entire stat record on whitespace, even though its parenthesized comm can contain spaces. That shifts the utime/stime positions and can report a wrong process total. Its per-thread parser and the mixed-capacity process parser already used the final closing parenthesis correctly. This fixes the common process path and unifies their field interpretation.

Each process snapshot now also records user_cpu_seconds and system_cpu_seconds from the same existing stat read. cpu_seconds retains its combined meaning and its original arithmetic for normal command names. Thread totals keep their existing map. There is no added proc read, sampling loop, per-packet instrumentation, native code change, CLI flag or dependency.

## Parser checks

The installed Linux proc_pid_stat(5) documentation specifies field 14 utime and field 15 stime in SC_CLK_TCK units. Children use separate later fields and are excluded. User time includes guest time where applicable; system time is the process's charged kernel-mode CPU, not time waiting for I/O or all network work on the machine.

The two focused tests cover four comm shapes on both snapshot paths (including spaces and embedded closing parentheses) and user/system/total scaling at three clock rates with large child counters. Three common-helper name cases fail before the fix; the new split-field expectations also fail because those fields did not exist. All 14 subcases pass afterward in 0.001 seconds. Existing six timing/FD-snapshot tests and the three inspection/mixed cleanup tests pass. The timed mixed monitor still avoids descriptor enumeration.

An actual Linux child sets its own comm to relay ) probe and blocks after reporting os.times(). It reports 0.06 user + 0.10 system = 0.16 CPU-seconds. The original common snapshot reports process CPU 0.00 while its own thread map reports 0.16. Both selected process snapshot paths report 0.16 with the correct 0.06/0.10 split. The child exits normally with status zero. This confirms the bug through an independent OS accounting API, not only synthetic stat text. [Receipt](live.json).

The default native war-relay command name has no spaces. This finding does not itself invalidate earlier correctly named relay totals or establish a new performance result; it fixes a general benchmark-name boundary.

## Two short native traffic observations

The unchanged frozen native binary metrics-static-20261019-native-candidate/war-relay and existing client client-read-window-20261019-control/inspection run sequentially. Both cases use four pinned native workers, eight pinned client workers on disjoint core sets, 1 KiB byte-string values, no inspection/churn/heartbeats/rate policy, two traffic seconds plus setup/checked teardown. The private case has 64 pairs; the separate fanout case has 2,048 readers and four-message windows. These are different workloads, not a matched implementation comparison. No builds, tests or profiler overlap traffic; no runtime instrumentation is added.

Raw ready/measured process snapshots supply the following deltas:

| Workload | User CPU seconds | Kernel CPU seconds | Total CPU seconds | Kernel share of process CPU |
| --- | ---: | ---: | ---: | ---: |
| Private pairs | 1.01 | 4.40 | 5.41 | 81.33% |
| Fanout | 0.76 | 3.28 | 4.04 | 81.19% |

CPU-seconds sum across threads and may exceed two seconds of elapsed wall time. These counters have 100 ticks/second resolution on this host. User plus kernel deltas reconcile with total CPU, subject to floating-point rounding. The snapshots span the existing measurement gates and their settling; they are not isolated relay-hop timers.

Both cases pass exact body/sender/FIFO/counter/census checks, zero native routing-copy bytes and all six checked resource gauges zero, with normal client/server exits. Private verifies 4,816,272 messages; fanout verifies 3,801,088 deliveries. The private worker set retains 192 small native arena slots at the empty gate under the existing warm-arena policy; the fanout case retires its arena to zero. Neither is an RSS/leak claim.

About 81% kernel time makes kernel-facing socket work a useful next investigation for these workloads. A user-only cycle profile cannot by itself attribute that portion of total process CPU. This does not identify a specific syscall, prove a kernel bottleneck or contention, compare Rust with C++, select a batch size, or prove a latency win. Changes that reduce kernel calls still need the existing fairness gates.

## Evidence and reproduction

The report contains original/selected helper sources, patch, test, failed-before and passing logs, live-child driver/receipt, traffic driver, both raw traffic receipts and a summary. Source base is adc4d08. Process snapshots use the new helper; native/bridge/client runtime source and existing manually running services are unchanged. Three pre-existing dirty source overlays were preserved.

```sh
python scripts/test-cpu-accounting.py
python scripts/test-fanout-window-timing.py
python docs/benchmarks/2026-10-19-process-cpu-accounting/verify.py
```

The archived drivers record the original fixed artifact paths and CPU masks. Preserve old outputs and choose fresh paths before replaying; run traffic sequentially and adapt masks to another host. The live driver imports the original common helper saved under artifacts/cpu-split-20261019-original-benchmark.py, whose exact original source is compressed here. It creates and retires only its own child. The traffic driver pins executable identities and all commands through the inspection receipts. This adds raw user/system fields; historical readers can continue using cpu_seconds, and older receipts do not acquire the new breakdown retroactively.
