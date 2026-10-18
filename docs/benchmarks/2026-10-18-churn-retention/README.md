# Four-wave connection retirement and allocator screen

Keep the existing allocator configuration. Four fresh processes run glibc / default jemalloc / default jemalloc / glibc, with the identical native executable and client. Default jemalloc preloading increases process RSS materially without a demonstrated CPU advantage. No runtime code, allocator dependency, environment or running service change is adopted.

Each process performs four waves of 16,384 real connections in 8,192 two-user rooms. Every connection routes a proof before and after a one-second paced traffic phase; 10% of users send four 1 KiB probes/second during that phase. Every connection closes normally, and every required departure receipt is checked before the next wave. Each case verifies 65,536 joins and normal closes, 157,312 routing checks and 32,768 departure checks. Four cases therefore verify 262,144 complete connection lifecycles. These are large short bursts, not steady churn or a 100,000-user capacity test.

Four native workers are individually pinned to physical cores 0–3. Eight client workers are individually pinned to eight other physical cores 4–11. The allocator experiment uses the existing server-only `--server-preload` option, verifies `/usr/lib/libjemalloc.so.2` in the server mappings and records its hash. The client allocator is unchanged. The invoking environment had no `LD_PRELOAD`, `MALLOC_CONF`, `GLIBC_TUNABLES` or `MALLOC_ARENA_MAX`. No allocator decay/purge tuning or explicit allocator trim is used. Installed jemalloc package: `1:5.4.0-1`.

## Memory after each fully retired wave

RSS includes the server allocator/library overhead and excludes the generator and kernel socket memory. Sampling occurs immediately at the checked retired gate, without an additional decay wait. It is not allocated-live bytes, a continuous peak, a leak detector or a resident-memory cap.

| Fresh process | Baseline RSS, MiB | Retired wave 1 | Retired wave 2 | Retired wave 3 | Retired wave 4 | Whole campaign server CPU-s |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| glibc, first | 5.15 | 14.16 | 28.20 | 30.83 | 27.34 | 1.85 |
| jemalloc, first | 27.11 | 57.00 | 54.62 | 58.94 | 57.03 | 1.87 |
| jemalloc, second | 27.31 | 62.52 | 64.76 | 60.96 | 57.77 | 1.87 |
| glibc, returning | 5.21 | 14.51 | 25.07 | 25.93 | 28.79 | 1.83 |

The default jemalloc configuration retains more RSS before and after the bursts. The conclusion applies to these binaries, installed allocator build and immediate snapshots. It does not prove glibc is always more efficient or jemalloc cannot be tuned for other workloads. CPU totals cover setup, proof traffic, measured traffic and complete retirement; the difference is too small to support a meaningful forwarding-speed claim. No throughput or rare-tail advantage is claimed.

All 16 retired gates return exactly to the initial 26 descriptors, zero established owned connections, zero sessions/connections/queued bytes/variable bytes/read-buffer reservations/output-queue capacity and one global channel. Exact accepted/joined/disconnected/delivery/byte counters pass. Routing copied bytes and error/rejection/pressure counters remain zero. No descriptor or accounted-resource leak is observed in this finite study; nonzero RSS after cleanup does not by itself establish an application leak. Both child processes exit normally in all four cases.

## Reproduce

Use a fresh output path each time:

```sh
python3 scripts/capacity.py \
  --server artifacts/candidates/mixed-capacity-final/war-relay \
  --client artifacts/candidates/latency-stages-client/capacity \
  --users 16384 --room-size 2 --waves 4 --seconds 1 \
  --active-percent 10 --messages-per-second 4 \
  --workers 4 --client-workers 8 --server-cpus 0-3 --client-cpus 4-11 \
  --pin-workers --pin-client-workers \
  --output artifacts/churn-retention-reproduction.json
```

Add only `--server-preload /usr/lib/libjemalloc.so.2` for the allocator case if that library is already installed. Do not export `LD_PRELOAD` around the runner: that would also change the fixed generator, and the helper refuses it. Inspect the host's topology before using these CPU masks. The explicit frozen paths reproduce this host's selected binaries; to compare new builds, pass their paths and retain their recorded hashes.

`python3 docs/benchmarks/2026-10-18-churn-retention/verify.py` verifies compressed receipts, all waves, exact counters, descriptor retirement and derived RSS/CPU values. Raw receipts include full CLI commands, source/runner/binary/helper hashes, affinity, owned-socket census and resource gates. No runtime or system setting changes were made. The evidence strengthens finite retirement coverage and rules out an unhelpful default allocator switch; it does not replace a long-duration soak.
