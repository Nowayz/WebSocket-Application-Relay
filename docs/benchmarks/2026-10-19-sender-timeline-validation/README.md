# Sender timeline validation, 2026-10-19

Source base `715a4d3`. Selected change: reject invalid sender-backlog evidence in mixed-capacity.py after timed traffic. The existing summary accepted NaN and negative maximum pre-send delays, and ignored skipped/repeated/reordered second labels. A one-window NaN fixture produced available=true and exceeded_one_period=false (generator-lag-before-proof.json preserves that Python JSON extension exactly; NaN is not standard JSON). This could label invalid evidence as a healthy sender schedule. No valid native receipt was found to contain these bad values; the problem is demonstrated with controlled fixtures, not claimed as a relay failure or an explanation for previous latency differences.

The helper now requires a positive integer messages-per-second rate, a nonempty timeline, contiguous integer seconds starting at zero, and finite nonnegative numeric maximum delays. Booleans are rejected rather than coerced to integers. Existing strict greater-than-period semantics remain: exactly one period does not count as over one period. Missing fields still fail normally. There is no new latency threshold, deadline, runtime option or output-schema field. All validation runs after timing; no client clock, measurement start, load, route or cleanup logic changes.

Six new test methods cover the period boundary, later backlog, invalid maxima (including NaN/infinities/negative/bool/string/null), second identities/order/gaps, invalid rates, empty input and nonmutation. Against the original helper the suite records 15 failures and 3 errors in 0.002 seconds. All six methods pass after the fix in under 0.001 seconds. The six existing fanout-clock/descriptor-sampling methods also pass.

Nine existing complete population receipts (four codec-limit cases, four current Rust/C++ comparison cases and the selected slot-gauge case) are replayed with both helpers. Each native timeline contains two seconds of 100,000-user observations. Every valid summary is exactly unchanged and equals the archived receipt. Replays do not send traffic or rerun a performance benchmark. They cover valid generated timelines, while the hand-written fixtures cover invalid evidence and exact period behavior. Neither the replay nor these small functional checks establishes a new 100,000-user performance result.

The existing test-mixed-load-gate.py also passes three sequential fresh two-second functional workloads with 32 private users and 16 fanout readers: paced rate 1 passes, unattainable offered rate 1,000,000 retains correctness but exits 1/passed=false, and unpaced rate 0 passes with offered-rate status unavailable. Independent churn/inspectors/extra pairs are disabled. All processes exit normally. This is six configured traffic seconds, not an additional large-population campaign. Its temporary receipts are removed by that existing test; the retained log contains the actual outcomes. Resolved convenience-executable hashes are recorded after the test in live-binaries.json, separately from the older replay receipts' frozen binary identities. These convenience binaries were not rebuilt or promoted by this work. No speed/RSS comparison is drawn from this functional check.

The entire main() body is byte-identical before/after. Only a math import and validation at the start of generator_lag() change. Thus the measured runner path, output exclusivity checks and process management are preserved. The normal invalid-evidence exception follows the existing failure path; it cannot become a successful correctness receipt. Existing services and pre-existing source overlays are untouched.

Run from repository root:

```sh
python3 scripts/test-generator-lag.py
python3 scripts/test-fanout-window-timing.py
python3 scripts/test-mixed-load-gate.py
```

The last command starts three small loopback workloads; the first two are pure/local fixtures. The recorded helper/test sources, bad-evidence proof, before/after logs, nine original receipt/timeline pairs and exact replay summaries are included. Full per-user CSVs remain in the original benchmark reports; no CSV is needed to replay this summary function. Run python3 verify.py in this directory for an offline hash/replay audit with no live traffic.
