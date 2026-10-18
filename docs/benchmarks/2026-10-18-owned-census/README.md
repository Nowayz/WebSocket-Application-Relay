# Server-owned TCP census — 2026-10-18

Code commit `51f1402` fixes an ownership error in the capacity benchmark's quiescent connection census. The previous counter includes every host TCP row with the relay's local IPv4 address, port and ESTABLISHED state. An unrelated connection can bind that same source endpoint using Linux socket reuse options and connect to another listener. Matching the local endpoint alone therefore does not prove the socket belongs to the tested relay.

## Reproduced failure and correction

The real TCP regression starts a separate server process, opens an unrelated parent-process connection from the server listener's local endpoint, then connects a genuine client. The server also duplicates the accepted descriptor to ensure the census counts sockets, rather than descriptor aliases. The foreign connection remains alive while the genuine connection is closed.

| Fixture state | Host matching ESTABLISHED rows (old count) | Server-owned established sockets (new count) |
|---|---:|---:|
| Foreign source-port alias, no genuine client | 1 | 0 |
| Genuine client accepted, with duplicate descriptors | 2 | 1 |
| Genuine client retired, foreign alias remains | 1 | 0 |

This demonstrates a concrete false-positive mechanism in the original counter. It does **not** establish that this was the cause of the earlier unexplained 100,001-versus-100,000 setup failure; that failed receipt is preserved in the preceding headroom archive and contains insufficient socket identities for a retrospective attribution.

The correction collects socket inodes during the existing server descriptor walk and intersects them with the matching ESTABLISHED TCP inodes. Unique socket identity prevents duplicate descriptors from inflating the census. `established_server_sockets` remains an integer, and its exact expected-population assertion is unchanged. An actual server-owned extra connection still counts; there is no ±1 tolerance, retry-based acceptance or relaxed leak check.

A new `tcp_census` object preserves `owned_established`, `raw_matching_rows`, `unique_matching_inodes`, `foreign_matching_inodes` and `duplicate_rows`. Foreign rows are visible diagnostically rather than folded into the relay population. Resource measurements still count descriptors as descriptors; only the TCP population count uses unique owned sockets. The existing IPv4/local-endpoint/state selection remains unchanged.

There is no second server descriptor walk. The temporary inode set is released after each census, before traffic resumes. All collection remains at the existing quiescent gates. No native server, client runtime, payload, zero-copy ownership, FIFO, admission, queue or wire-format code changes.

## Verification

`python3 scripts/test-capacity-census.py` passes the actual Linux TCP/process regression in less than a second. It verifies exclusion of the foreign source-port alias, inclusion of a genuine server connection, descriptor deduplication and exact retirement while the foreign socket remains connected. The fixture uses disposable ephemeral ports and shuts down its child process.

Two full-scale integration checks each use **100,000 real relay users, 1 KiB private values, 4 Hz and a two-second offered phase**. Each checks 800,000 measured probes plus the pre/post proof rings, body/FIFO correctness, exact counters, normal Close/EOF and complete resource retirement. Both temporary relay/client pairs exit zero. All four live gates report exactly 100,000 owned established sockets, with zero at the done gate. The ordinary host raw row count also happens to agree in these runs; the small regression supplies the deliberate foreign-row test.

The first integration run precedes one lifetime-only refinement: explicitly dropping the gate's temporary inode set before traffic resumes. Its exact helper source is retained as `sources/before-release-capacity.py`. The second integration run uses the final committed source, whose hash is checked against its receipt. Both source variants remain bound to their own results. This is correctness/integration evidence, not a throughput, latency-tail, memory-reduction or soak claim. Each user contributes eight timed observations.

Python compilation and source whitespace checks pass. No Rust source is changed and no new Rust test run is claimed. All three pre-existing user-modified relay/C++ files remain byte-identical. The manual native, bridge and static services are not restarted.

## Provenance and reproduction

The temporary tests use frozen native relay `3db9ebeb0fff87636a1bf2fc5a6e0ae88d48d12aa7a52571cb4b28fa4d911fa9` and clean capacity client `c74a950efc932b5c7dfe8346e5ebfaf4d3d82a80b68d5b6a0e5bd4c9e655e906`. Their build/source provenance is bound by `dependency.json` to the preceding headroom archive. The library/C++ hashes in source-attribution snapshots are workspace observations; the frozen binaries' source associations come from those build records.

The test uses eight individually pinned relay workers on physical CPUs 0–7 and eight individually pinned client workers on physical CPUs 8–15, no physical overlap, as in the headroom private-only study. There is no fanout/churn sidecar in these integration checks. Recorded commands and exact counters are in each JSON receipt. Adapt CPU masks only after checking your machine's topology.

`manifest.json` protects every archived source/result and compressed CSV. `verify.py` additionally checks decompressed CSV hashes, all 100,000 user identities/eight samples, exact population/counter/resource receipts, both actual runner hashes and the frozen executable pins:

```sh
python3 docs/benchmarks/2026-10-18-owned-census/verify.py
python3 scripts/test-capacity-census.py
```

The verifier is offline. The second command opens a few disposable local sockets to reproduce the alias regression. To replay the 100,000-user integration, use the saved receipt's command in the repository with its matching helper version; gate scans and setup/teardown are outside the two-second traffic phase. Historical archives are unchanged.
