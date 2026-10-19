# Four versus eight workers under 100k-user observer/churn load

The mixed-capacity runner now accepts `--workers 1..8`, default eight. It forwards the requested count and checks that exactly that many I/O workers occupy distinct single-CPU affinities. The bound comes from this fixture's fixed CPU pool 0–7; it is not a new runtime relay cap. Client CPU masks and all other default commands stay unchanged. Reproducible worker comparisons no longer require editing copies of the runner.

The recorded CPU pool stays 0–7 for both settings, including the management/main thread. Four native I/O workers pin to 0–3; eight pin to 0–7. Two eight-worker clients retain their disjoint fixed masks and individual thread pinning. The comparison varies the I/O worker count, not the whole server process's allowed CPU mask.

## Same-binary short comparison

One freshly compiled current-source native release runs eight/four/four/eight workers in fresh fixtures. Each offers **two seconds** of traffic to 100,000 private users plus 8,192 authenticated global channel observers, with four 1 KiB private probes/user/second, four fanout messages/window, 16 fanout windows/second, and eight churn workers targeting 16 lifecycles/second each (256 total). There are no timed inspectors or extra private pairs. Configured bounds, 16/64 KiB socket buffers, disabled heartbeat/idle/rate timers and every verification gate are identical. Builds, profiling, other tests and other benchmarks do not overlap timed traffic.

| Run | Fanout windows completed | Churn completed / target | Offer accepted | Median user's worst private ms | Worst private ms | Worst fanout ms |
| --- | --- | --- | --- | --- | --- | --- |
| Eight / 1 | 32 / 32 | 256 / 256 | Yes | 3.104 | 22.543 | 38.496 |
| Four / 1 | 19 / 32 | 166 / 256 | **No** | 35.246 | 50.416 | 137.861 |
| Four / 2 | 20 / 32 | 174 / 256 | **No** | 33.246 | 45.313 | 139.885 |
| Eight / 2 | 32 / 32 | 256 / 256 | Yes | 3.805 | 17.564 | 35.573 |

Both four-worker runs preserve delivery/FIFO/counter correctness and retire cleanly, but fail the offered-load gate. They remain `passed=false, correctness_passed=true, fanout_offered_load_met=false`, with runner exit one. They are overload evidence, not successful equivalent-throughput benchmarks. The initial controller stopped at the first rejected trial; a continuation runs only the two remaining fresh trials and preserves the original failed receipt and acceptance gate. No data is overwritten or relabeled as passing.

All cases verify 800,000 measured private observations. Eight-worker runs complete all 256 target churn lifecycles; four-worker runs complete only 166 and 174, with every completed lifecycle checked. Churn is paced by the configured ceiling, while the runner's acceptance gate explicitly requires the offered fanout rate; these distinct contracts are preserved. Eight-worker runs verify 1,048,576 measured fanout observations each; four-worker runs complete only 622,592 and 655,360, respectively. Every reader receives the same checked complete windows within its run. Private users have eight observations each, so their empirical p99 equals their observed maximum. Fanout sample counts differ across accepted and overloaded runs; no matched-work performance ratio is claimed.

Server CPU totals are 6.33/6.41 seconds for eight workers and 4.90/4.88 for four. The lower four-worker CPU totals do **not** establish better efficiency: those runs complete less offered fanout and churn work and take longer to drain. No claim is made that every provisioned core was saturated or that worker count alone identifies a kernel/lock bottleneck. The directly supported result is that eight workers meet this load and respond much better, while four fail the offer.

## Operating recommendation and limits

Keep the existing eight-worker fixture default and use explicit `--workers 8` for this observer-heavy load on the measured host. The older 30-second private-only study also found eight workers useful for latency at a CPU cost, but this is a distinct fanout/churn comparison. Neither study establishes a universal count for another machine or traffic shape.

No production deployment, example default or live service is reconfigured. The existing four-worker 100k environment example has a different bounded member/fanout envelope (1,024 members/channel), whereas this test explicitly permits 10,000 to admit the 8,192 global observers. Do not treat a user-count example as a guarantee for every broadcast/observer workload.

All four runs pass exact body/FIFO, independent counter/census, zero error/pressure counters, descriptor retirement and zero final connection/session/queue/variable/read-reservation/output-capacity/native-slot gauges. All three disposable processes exit zero, including the correctly rejected offered-load runs. Traffic correctness is kept distinct from rate acceptance. These are plaintext local-loopback finite observations, not indefinite stability, WAN/TLS results or latency SLAs. Setup and exhaustive teardown add time beyond the eight seconds of offered traffic total.

The preflight tests pass output-collision cases and reject worker counts zero/nine before any child starts. Existing profiler-failure and offered-load-gate checks pass. Positive worker counts four and eight are verified by the full socket fixtures and recorded affinity lists. The first local edit had an unterminated help-string quote; it was caught before any child launched and corrected before these tests.

The frozen executable is `artifacts/candidates/worker-count-20261019-current/war-relay`, built from the committed source excluding existing user overlays. Every trial has its same binary hash. Commands, build identity, raw receipts, summaries and preflight logs are retained here; the complete CSVs remain local under recorded artifact paths/hashes. Run `python verify.py` for an offline evidence audit.
