# Socket receive substitution: rejected short screen

Keep the current native input path. Replacing its general `read` with socket2's safe `Read for &Socket` implementation (which calls `recv`) does not improve the measured relay workload. Both implementations use the existing connection-state lock, initialized receive buffer and Mio readiness handling. No production code, dependency, queue policy, packet format or running service changes are retained.

The prior profile identified connection driving as a substantial cost. The relay already uses socket-specific `send`/`send_vectored` on output. This experiment tests the analogous input change rather than assuming the output result transfers to reading. The candidate calls `SockRef::from(&state.stream)` inside `TcpStream::try_io`, preserving Mio's WouldBlock bookkeeping. It does not add unsafe code or copy the payload. The pinned socket2 0.6.5 source's `Read for &Socket` implementation performs the initialized-buffer conversion internally and delegates to `recv`; existing ordinary reads use Mio's `Read` implementation. The archived patch contains the entire candidate.

## Three-second comparisons

Fresh processes run control/direct/direct/control, each with 8,192 stable rooms, 64 private pairs, 1 KiB cyclic payloads and 16-message windows. Active inspection and churn are disabled equally. Complete census, exact counters and normal closes are verified outside timing. Four server workers are pinned to physical cores 0–3; eight client workers are individually pinned inside 4–7,20–23, on four separate physical cores with SMT. Actual affinity is checked at ready/mixed/done gates.

| Trial | Read path | Million verified messages/s | Server CPU ns/delivery | Client-observed p99 upper, ms | Maximum, ms |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | Current read | 2.383 | 1128.73 | 1.049 | 1.936 |
| 2 | Socket recv | 2.323 | 1165.04 | 0.524 | 1.362 |
| 3 | Socket recv | 2.332 | 1157.71 | 1.049 | 1.322 |
| 4 | Current read | 2.357 | 1148.16 | 0.524 | 1.285 |

The candidate's throughput is lower in both runs, and its CPU cost per verified delivery is higher. Latency buckets/maxima vary across both configurations, without a repeatable candidate advantage. Reject the substitution. These short observations support that decision on this host/workload; they do not prove `read` is universally faster than `recv`, identify a kernel cause, or establish a rare-tail improvement from either path.

All cases verify full sender/body/FIFO and exact setup/traffic/cleanup counters: **28,188,096 timed deliveries**, plus separately counted proof/departure messages. Connection/session/read-buffer/queue/variable gauges return to zero, only the global channel remains, routing copied bytes and error/pressure counters remain zero, both processes exit successfully, and none is forcibly killed. The p99 column is a histogram upper bound, not an exact quantile. Latency includes client windowing and verification. This is a small private-traffic screen, not a 100,000-user capacity/fairness comparison.

An independent input corpus against the candidate passes **776 cases / 2,311 exchanges**, including malformed, fragmented and pipelined inputs, interleaved control frames, policy and terminal Close outcomes, healthy witness traffic, exact counters and cleanup gates. It verifies 387 independent healthy witness messages. Corpus duration is approximately 0.83 seconds; each throughput traffic phase is three seconds. There is no long soak or TLS/WAN claim.

## Build and audit

`build.json` pins clean source revision `09afbab`, build paths, source hashes and derived results. Both release executables were built sequentially in the same clean project/target directory. The candidate changes only `src/server/socket.rs` as shown in `candidate.patch`. The three existing user overlays were excluded and remain byte-identical. Builds/profiles did not overlap the timed comparisons. Running selected native/bridge executable identities were rechecked unchanged.

Run `python3 docs/benchmarks/2026-10-18-socket-recv-screen/verify.py` to verify the archive, replay the one source substitution against Git history, and check exact counter/cleanup receipts. Reproduction uses `git archive 09afbab Cargo.toml Cargo.lock src examples vendor`, `cargo build --release --bin war-relay`, then the archived patch and the same build path. Freeze each executable separately. Full CLI commands, hashes, topology and affinity observations are in the four raw JSON receipts. Repeat them through `scripts/inspection.py` using fresh output names/ports and inspecting the host's topology first. No network download is required by the offline verifier.
