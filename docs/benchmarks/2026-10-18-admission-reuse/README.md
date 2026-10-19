# Failed-upgrade admission reuse

Selected tooling: `scripts/admission-reuse.py` provides a short native lifecycle check. No admission-runtime change was necessary. The shared independent raw client accepts an optional source address; its default connection behavior remains unchanged, avoiding a second handshake/parser implementation in this probe.

A disposable four-worker server has a global cap of three connections, a per-IP cap of one, and a one-second handshake deadline. A healthy joined user uses 127.0.0.1. Two waves of malformed HTTP, early EOF and timeout use 127.0.0.2. While each failed upgrade owns its slot, an overlapping connection from that IP must be refused, incrementing the per-IP counter. After failure, the same IP must upgrade, join, verify exact self-delivery and complete normal Close/EOF. The healthy user verifies traffic throughout timeout waits and after every replacement.

The final probe passes in 2.012 seconds. It checks six IP refusals, six failed upgrades, six successful replacements and 104 healthy witness messages, with 49 healthy packets verified during each timeout wait. Exact totals are 19 accepted transports, seven joins/seven normal retirements, 110 application inputs and 117 binary deliveries (including welcomes). Delivered bytes match independently received bytes. Final connection/session/queue/variable/read-staging/output-slot gauges are zero; the persistent global channel remains. The server exits normally with no write/Close/transport errors or routing payload copies.

An initial fixture assumed malformed HTTP must return a 400 body. The native implementation may retire that failed upgrade with EOF; the probe accepts either EOF or an HTTP 400 response and checks lease/counter behavior instead. This was a fixture correction, not a relay defect or a new HTTP-response policy.

The shared raw helper also passes its full 776-case / 2,311-exchange input corpus in 0.835 seconds with eight clients and four native workers. This verifies its unchanged default source-address path. It is finite regression evidence, not exhaustive concurrency verification, connection-rate measurement or a 100,000-user latency qualification.

Reproduce after building release war-relay:

```sh
python3 scripts/admission-reuse.py --server target/release/war-relay \
  --output artifacts/admission-reuse.json
```

The probe requires Linux loopback addressing, uses fresh exclusive output/log files, strips inherited WAR runtime overrides, and launches/cleans only its own server. Its existing-corruption corpus helpers and source hashes are retained. `build.json` identifies the clean selected native snapshot and frozen executable; pre-existing dirty overlays were excluded.

The operations/status/performance introductions now point to current evidence and distinguish source from manually running binaries. `manual-processes.json` records the executable hashes actually inspected through /proc: native PID 966632 remains 279c3c1/3db9ebeb…, and bridge PID 1106809 remains 4c9113a/bb2392f5…. No service or convenience binary was promoted. Older status entries remain historical records.

The shared-helper patch applies to a61fa5c; the new probe source is retained in full. Run `python docs/benchmarks/2026-10-18-admission-reuse/verify.py` to audit source identities, lifecycle counters, witness traffic, corpus and process snapshot offline.
