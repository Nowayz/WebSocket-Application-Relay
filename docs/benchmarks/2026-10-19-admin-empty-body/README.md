# Empty HTTP page body: measured allocation saving, runtime candidate unselected

Keep the existing JSON encoder and retain one focused HTTP wire regression. The candidate special-cases an empty channel snapshot with Bytes::from_static(b"[]") instead of allocating a two-byte serialization buffer. It removes one allocation per terminal page but does not establish a request-time benefit in the short screen. No static-body branch is adopted. Selected production source before cfg(test) is byte-identical to the original; only the real-HTTP regression is added.

This distinguishes the earlier selected allocation-free empty channel snapshot from the complete HTTP response: query parsing, HTTP framing and response handling still allocate. The fixture has only the persistent global channel and requests limit=1 after its Base64 cursor, producing the actual terminal page. Every measured request must return 200, application/json, Content-Length: 2 and exactly []. A warmup of 100 verified requests precedes the measured count. Populated-page and unauthorized-request checks follow, outside the allocation/timing phase, and six final data-plane resource gauges must be zero. Each disposable relay exits normally.

## Allocation diagnostic

Two sequential server-only tcmalloc runs measure 1,000 verified terminal-page requests each. Quiescent dumps before/after the request phase have identical live heap: 232 objects / 131,386 bytes in both builds. Every raw allocation row sums exactly to its profile header.

| Cumulative request-phase allocation | Control | Candidate |
| --- | ---: | ---: |
| Objects | 23,000 | 22,000 |
| Requested bytes | 10,220,000 | 10,218,000 |

The 1,000-object / 2,000-byte reduction matches one two-byte allocation per response. A control allocation group with 1,000 objects and 2,000 bytes disappears in the candidate. These are requested allocation counts/bytes under a replacement allocator, not ordinary RSS or allocated-memory peaks. There is no retained-heap saving. Diagnostic timing/CPU is excluded from ordinary comparisons.

## Ordinary fixed-work comparison

Six fresh-process runs each verify 50,000 requests with the normal allocator. Order is control/candidate/candidate/control, followed by a candidate/control pair because the initial timing result was uncertain. One native worker is requested; client/admin processes use ordinary OS CPU placement. There are no WebSocket users, churn, TLS, remote network or concurrent builds/tests/profiles/analysis. Python synchronous HTTP/1 keepalive request/reply and checks contribute to elapsed time; this is not isolated server execution or a per-request latency distribution.

| Run | Build | Elapsed seconds | Server CPU seconds |
| --- | --- | ---: | ---: |
| 0 | Control | 1.404318 | 0.16 |
| 1 | Candidate | 1.404910 | 0.15 |
| 2 | Candidate | 1.451535 | 0.16 |
| 3 | Control | 1.393948 | 0.15 |
| 4 | Candidate | 1.445113 | 0.16 |
| 5 | Control | 1.399259 | 0.15 |

The candidate gives no repeated elapsed-time benefit; two of its three runs are noticeably slower than controls, while the first is nearly equal. CPU is coarse process-tick accounting and does not isolate a two-byte allocation cost. This finite, unpinned screen cannot prove the static body causes a universal regression. It provides insufficient reason to add a special case for a 0.02% requested-byte saving when the stated priority is low latency and simplicity. The encoder remains selected; no further tuning campaign is warranted for this tiny allocation.

The retained real-TCP unit checks exact empty body, complete content-type/content-length header lines, unauthorized 401, invalid-query 400, a populated JSON page and unchanged census, under a two-second failure guard. It is a behavior regression guard and passes the existing encoder; it does not demonstrate an old protocol bug. The final selected test-only source passes all 117 library tests in 0.53 seconds, strict bins/library/test Clippy and all 65 JavaScript/native/legacy/idle cases in 1.124 seconds, with all six checked resource gauges zero. The final candidate also passes its focused wire check and strict Clippy. Build/install/startup and cleanup add wall time.

Original/selected/candidate source, both patches, frozen candidate/control build identities, all eight raw receipts, four heap captures, allocation summary, controller and test logs are retained. Run python verify.py for the saved-evidence check. To rerun, copy probe.py into artifacts and provide a frozen server plus fresh output; add --heap-prefix for a separate allocation diagnostic, or --requests 50000 for an ordinary fixed-work case. The controller resolves existing scripts from repository-root cwd. No throughput/100,000-user/native-routing change or guarantee follows. All pre-existing dirty source overlays and manual services remain untouched.
