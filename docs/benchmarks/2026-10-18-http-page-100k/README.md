# Short 100,000-user integration with HTTP pages

Runner change `57d479a` adds opt-in `--http-page-size 1..1000` to the existing
mixed-capacity test. Default CBOR inspection stays unchanged. HTTP mode uses
the existing inspector client, authenticates disposable HTTP requests, checks
first pages during traffic, and verifies complete pagination at quiescent
gates. Its connection census accounts for the absence of persistent inspector
WebSockets and the protocol-only probe. Unsupported C++, invalid limits and
fanout-membership-churn combinations fail before launching any server.

The runner also prints `fanout_windows` and `fanout_offered_load_met` in the
console summary and records the latter explicitly. Existing `passed` semantics
remain content/counter/resource correctness for the work actually submitted;
it is not an offered-load or latency guarantee. The new flag is `null` for
unpaced fanout, otherwise the comparison of completed versus scheduled windows.
No native server code or production default changed in this experiment.

## Why this follow-up

The [batched selection change](../2026-10-18-batched-channel-page/README.md)
keeps its result allocation alive only through synchronous JSON serialization:
`src/admin.rs::channels` drops the owned snapshot vector before returning the
HTTP response body. A slow HTTP client therefore does not retain that metadata
buffer across network writes. This source finding did not justify another
shrink/reallocation step. It is not a measurement of long-term allocator RSS.

The existing 100,000-user fixture used CBOR inspection and did not call
`channel_page`. A successful run of that fixture alone could not verify the
changed HTTP path under the full user population. The new option closes that
test-coverage gap without a second load generator or new runtime machinery.

## Configuration and finite scope

Four large runs use 100,000 private users in 6,250 sixteen-member rooms and
8,192 additional fanout readers. Each generator offers six seconds of traffic:
24 probes/private user at 4 Hz, four fanout messages/window at 16 windows/s,
eight churn workers at 4 Hz and two inspectors at their existing 10 ms cadence.
Setup, exhaustive Close/departure checks, CSV output and teardown are outside
the nominal six-second offer. Controller gate spans are approximately 6.57–6.62
seconds and are recorded exactly; establishing/retiring 100,000 sockets makes
the whole invocation longer than six seconds. No multi-hour soak was run.

Eight native workers use physical cores 0–7. The capacity generator uses
8–11,24–27 and the sidecar 12–15,28–31, with all 16 client I/O workers pinned
individually. Clients do not share a physical core with the server. Exact
latency matrices and stage clocks are enabled equally. Rates/heartbeat are
disabled for the fixture; configured queues and receive-memory budgets remain
bounded. No builds, profiles or other benchmarks overlap the measured phases.

Baseline is the selected old native executable (runtime `279c3c1`); current is
the clean batched-page candidate adopted in source at `3ee013c`. Exact executable
hashes are in receipts. The native source delta also includes the earlier
exact-capacity CBOR byte-string helper, so this is not a one-change algorithm
ablation. CBOR trials do not execute the HTTP page selector at all.

## Large-run observations

Private columns rank users by their observed worst probe during the trial.
With only 24 samples/user, nearest-rank per-user p99 equals that maximum. A
population p99 across those maxima is not a long-term per-user p99 or deadline.
Fanout columns similarly include client windowing and verification, not solely
server processing; stage clocks cannot isolate server-only latency.

| Trial, inspection | Private median worst ms | Population p99 of private worst ms | Worst private user ms | Worst fanout window ms | Completed/offered windows |
| --- | ---: | ---: | ---: | ---: | ---: |
| Old baseline, CBOR | 1.569 | 7.106 | 11.042 | 68.924 | 96/96 |
| Current, CBOR | 2.225 | 14.843 | 17.923 | 94.139 | **95/96** |
| Return old baseline, CBOR | 1.547 | 14.667 | 18.324 | 74.163 | 96/96 |
| Current, HTTP pages of 1,000 | 1.596 | 6.258 | 9.145 | 66.635 | 96/96 |

All four passed exact content/FIFO, submitted-work counters, normal departures
and retirement checks. The current CBOR trial nonetheless missed one scheduled
fanout window: its pacing loop skipped an opportunity rather than silently
losing an already-submitted delivery. It consequently verified 380 samples per
fanout reader rather than 384. This is a workload shortfall, not a full-offer
performance pass. Historical receipts are preserved with their original fields;
the new console flag exposes such a shortfall in subsequent runs.

The return baseline also had elevated private tails, so this sequence does not
establish a unique code regression or improvement. The HTTP run uses different
inspection work and cannot be compared to CBOR as a pure native speedup.
Do not pool these trials, hide the missed window or use the best run as an SLA.

## HTTP-path acceptance

The HTTP run verifies all 2.4 million timed private probes, all 96 fanout windows
(3,145,728 timed fanout deliveries), and 1,200 first-page queries during traffic.
Full sorted pagination and exact metadata are checked outside the timed phase.
There are 108,193 stable native connections at readiness: 100,000 private users,
8,192 readers and one fanout publisher. HTTP admin sockets are separate from
the native WebSocket census.

Native sampled RSS was 114,420 KiB before and 117,448 KiB after the workload.
Those are boundary samples, not peak process memory or total machine/socket
memory. The final connection/session/queue/output-capacity/read-buffer/variable
gauges and routing-copied-byte counter are zero; only the global channel remains.
All exact protocol error/pressure/rejection checks pass, descriptors return to
the server baseline, and all three child processes exit zero. Both generator
placement and delay checks pass; no private sender exceeds its 250 ms period.
This remains finite loopback acceptance without TLS, not proof of production
capacity or uniformly low future latency.

Two additional two-second integration runs use 1,600 private users and 512
fanout readers. The new HTTP mode and unchanged default CBOR mode each pass
all gates, record all 16 client worker assignments and complete 32/32 offered
windows. Four invalid-option checks reject before process launch. Python syntax
and diff whitespace checks pass. Native source did not change, so unrelated
build/test suites were not repeated.

## Reproduction and audit

```sh
python scripts/mixed-capacity.py \
  --server artifacts/candidates/page-partition/war-relay \
  --capacity-client artifacts/candidates/latency-stages-client/capacity \
  --users 100000 --seconds 6 --readers 8192 --scope room --pairs 0 \
  --churn-cycles-per-second 4 --fanout-windows-per-second 16 \
  --exact-fanout-latency --latency-stages --pin-client-workers \
  --http-page-size 1000 --output artifacts/http-page-reproduction.json
```

Use a fresh output prefix; the script rejects existing receipts. Omit only
`--http-page-size` to retain default CBOR inspection, acknowledging the changed
inspection workload. For a quick harness check use `--users 1600 --seconds 2
--readers 512`. Preserve the disjoint CPU topology when adapting to another host.

`summary.json` extracts the observations. All raw receipts, private/fanout CSVs,
window timelines and console output are losslessly gzip-compressed. Decoded
hashes match the measured receipts; the original and updated runners are
archived separately rather than rewriting historical attribution.

Run `python docs/benchmarks/2026-10-18-http-page-100k/verify.py` to verify archive
hashes, decoded runner/CSV/timeline identities, user/sample counts, finite
quantiles, offered-load flags and retirement gauges. This is an offline
consistency check, not a new network run. All three pre-existing user-modified
files remain byte-identical, and selected live services were not restarted.
The result strengthens the acceptance harness; it does not select a new live
native executable or resolve the observed tail variability.
