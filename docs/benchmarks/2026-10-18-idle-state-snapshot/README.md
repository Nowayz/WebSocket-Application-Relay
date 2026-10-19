# Coherent native idle decisions — 2026-10-18

Selected correctness fix: idle expiry checks application activity and pending output under the same socket-state mutex acquisition. Connection deadline collection also captures output and idle deadlines under one acquisition. The output-deadline accessor now operates on the already-held state; the existing socket accessor delegates to it. No new state, lock, allocation, timer policy or deadline approximation was added.

Previously the idle check called `socket.deadline()` to capture whether output was pending, released that lock, then acquired the state lock again to inspect activity. Another worker could admit deferred fanout between those reads. Because queued data refreshes activity when written, an expired activity value could coexist with a stale "no pending output" flag, selecting idle closure despite queued work. This contradicts the existing policy that pending output postpones idle retirement while retaining its separate write deadline.

The selected state method evaluates the complete idle predicate while the queue, control deadline and activity are protected together. If expiry is due and output remains pending, it refreshes idle activity as before, without changing the write deadline. A fresh idle timestamp stays unchanged; a due idle socket with no pending output still expires at the exact boundary. Idle tracking remains disabled by default and is checked only for registered sessions. Existing write/control timeout and close policy retain their priority.

The controlled interleaving test captures an empty-output snapshot and an expired activity timestamp, admits a shared multicast on a different thread through the real relay path, then evaluates the state. The former predicate would expire that state using its captured flag; the selected method sees the queued frame, postpones idle expiry, preserves its write deadline and retains the original body pointer. The test then flushes the frame, verifies its bytes and refunds, and confirms private output remains immediate. Another test covers fresh activity, the exact expiry boundary and pending control output. This constructs the legal interleaving directly; it does not claim a deterministic end-to-end reproduction of the pre-fix worker race on a live server.

Verification stays short:

- All 93 working-tree library tests pass in 0.48 seconds, including the strengthened cross-thread shared-frame test and new idle/control boundary test.
- Strict all-target Clippy and formatting pass.
- The clean release native binary is built through the snapshot helper, with Cargo reporting `fresh: false`; the three pre-existing source overlays are excluded.
- All 24 client tests pass against that native binary and the selected bridge in 1.09 seconds. The real one-second application-idle test passes through native and legacy connections. Both processes exit zero with final connection/session/queued-byte/variable-byte gauges at zero.
- The independent socket corpus passes 776 cases and 2,311 exchanges.

Each of the changed idle-enabled paths takes one state lock instead of two. No CPU, throughput or latency speedup is claimed; this change is selected for a coherent decision and preserved deadline semantics. No long population test was needed for this correctness check.

`build.json` pins the source base, exact selected sources and excluded overlays. The selected patch, source files, full test/build logs, helper build receipt, live client fixture/receipt/server logs and corpus receipt are retained. Run `python docs/benchmarks/2026-10-18-idle-state-snapshot/verify.py` for an offline identity and acceptance audit.

Manual services, convenience executables and the three pre-existing dirty source files remain unchanged.
