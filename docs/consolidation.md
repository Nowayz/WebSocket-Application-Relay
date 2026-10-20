# Rust and native CBOR consolidation

The maintained product now consists of the `war-relay` Rust server and the
`clients/js` native CBOR client. Binary/Text clients, the compatibility bridge,
C/C++ servers, native comparison libraries/build systems and their dedicated
scripts have been removed. The original implementations and full comparison
records remain accessible at Git commit `587c1cd`; the
[performance history](performance-history.md) retains the useful findings.

The JavaScript package includes public TypeScript declarations, uses explicit
JavaScript codec entries and omits optional native extractors. Its standalone
browser module is built with a WebAssembly bundler. The production Rust server
uses the synchronous WebSocket codec directly; the async wrapper is a development
dependency for load clients and tests. Shared payload ownership, routing order,
admission policy and resource budgets retain their existing contracts.

## Local acceptance on 2026-10-19

- Formatting and Clippy with warnings denied passed.
- All 205 Rust tests passed. A real duplicate-registration fault checks TCP
  closure and release of physical admission/queue credit without a C shim.
- All 66 JavaScript tests passed, with no skipped tests. Six fixture resource
  gauges returned to zero before shutdown.
- TypeScript usage checks passed. The browser build and four-file package
  dry run passed; nothing was published.
- All seven browser checks passed against a newly built native server:
  exact uint64 IDs, structured values, Unicode/bytes, private routing,
  a 128 KiB value, concurrent commands/variables, rejected unsafe numeric IDs,
  departures and clean Close handshakes. Six resource gauges returned to zero.
- JavaScript build-freshness, mixed/inspection runner cleanup/output,
  profiler-failure, fanout-window timing and generator-lag regressions passed.
- The native accept-pressure check passed with one and four workers.
- The independent raw WebSocket/CBOR corpus passed in whole-message, fragmented
  and pipelined modes, including malformed inputs and Close/EOF checks.
- Short capacity and inspection screens passed. The mixed offered-load check
  accepted its satisfiable/unpaced cases and rejected its deliberately
  unsatisfiable case while preserving correctness checks.
- The maintained-product audit found one Rust server target and no retired
  native/protocol files. Vendor provenance verified all 62 recorded files.

This is functional consolidation evidence. It is not a new 100,000-user timing
campaign or a measured throughput improvement. The registration test replaces
one useful retired fault-injection case; it does not intercept every failing
socket-option syscall. See [stability checks](stability.md) for those limits.

## Follow-up milestone

The next milestone was to narrow disconnect-listener revocation to the relevant
observer. At consolidation, pending global departure work made that barrier
synchronously submit other observers' output; the prior 100,000-observer screen
showed roughly 338–350 ms stalls. The
[follow-up report](observer-barriers.md) records its implementation and checks.
Preserve old-notice-before-ack ordering, prevent replay after re-subscription,
leave unrelated recipients deferred, and handle a ready full one-slot output
queue before acknowledging the command.

Compare short control/candidate runs with the same native clients and frozen
binaries. Check the whole user population, worst event latency, completed offered
load, CPU and retained memory. Adopt the change only if those results justify it
while retaining per-recipient FIFO and bounded work/storage. The ongoing goal is
consistently responsive operation at 100,000 concurrent users with an understandable
Rust/CBOR implementation; this cleanup does not complete that performance work.
The [current roadmap](roadmap.md) carries this goal forward after the observer
barrier and global-broadcast scheduling changes.
