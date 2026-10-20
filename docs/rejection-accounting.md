# Routing refusal accounting

Failed initial routing attempts now debit the worker's phase allowance for the
recipient handles actually visited, including skipped and refused recipients.
Cheap validation or staging refusals cost one attempt. A failure after crossing
from a room into its observer segment includes only work in the current turn;
completed preceding turns are not charged again. This closes the rejection path
left open by the [worker scheduler](worker-scheduling.md).

The change carries a small stack error with a reason and visit count. It adds no
payload copy, allocation, connection field or shared counter. Source FIFO and
the existing best-effort delivery contract under output pressure remain intact:
already accepted output is retained when later observer delivery fails. Continued
jobs retain their existing conservative quantum charge. Snapshot construction,
control scans and synchronous teardown are separate costs; a visit allowance is
not a wall-clock latency guarantee.

## Checks and selection

Three deterministic tests cover malformed/unauthorized requests, metadata
refusal, partial private-observer delivery, closed recipient prefixes and failures
across room/observer segments. They check different starting allowances, no
buffered source read-ahead, pointer-identical retained payloads and complete credit
release. The native `scripts/rejection-fairness.py` fixture sends two pipelined
private messages per source, forces rejection during observer publication, and
checks every primary delivery, absence of the buffered follow-up, independent
control/join progress and all shutdown Close/EOF receipts. Its guard checks reject
invalid budgets and reused receipts before launching a server.

A prototype that charged a complete 1,024-recipient turn for every overload was
rejected: at 100,000 observers and 64 publishers on one worker, its worst control
response was 15.55–15.81 ms versus 2.89–2.99 ms in that control pair. Cheap staging
refusals do not justify a full-turn charge.

The counted implementation was screened in control/candidate/candidate/control
order with the same 100,000 observers, 64 publishers and 1 MiB shared credit.
Every run delivered all 64 primary packets, the same 921 partial observer
deliveries, and all 100,000 shutdown Close/EOF receipts. Worst independent control
responses were 3.41–4.33 ms for the control and 4.28–4.30 ms for the candidate;
publisher rejection completion was 7.25–7.60 ms and 6.89–6.91 ms respectively.
These short runs support the accounting correction, not a general speedup.

Eight-worker healthy screens retained exact delivery and zero resources after
cleanup. Two seconds of mixed 100,000-user traffic completed 16/16 fanout windows
in every repeat. The candidate's intended-send maxima were higher in this screen:
8.38–8.75 ms versus 5.70–8.29 ms for observers, and 7.96–8.48 ms versus
5.68–5.75 ms for rooms. Those maxima include generator scheduling; they are not
discarded. An additional fixed-offered room diagnostic without unpaced pairs or
inspectors had overlapping latency ranges and 4.11–4.12 CPU seconds for both
builds. The fix is selected for correct bounded-work accounting; it is not a
latency optimization. Further performance experiments are deferred for release.

All 239 Rust tests and 66 JavaScript tests passed; one manual Rust diagnostic is
intentionally ignored. Formatting, Clippy with warnings denied, native channel,
private, combined and concurrent observer checks, 100,000-observer departure
pressure, fallback, primed shutdown, input corpus and descriptor-pressure recovery
also passed.

Control executable SHA-256:
`a7b2350b129c7882995e3d3dcd3dba14d476f7c7adbdc53f173c733568eabd2a`.
Counted executable SHA-256:
`c7200e767f457d960686a3364803ca9df0aa921fe77bac308af01cf17967cd61`.
Frozen source/build receipts, rejected prototype and raw comparisons are retained
locally under `/home/phil/dev/war_next/artifacts/rejection-accounting-20261019/`.
