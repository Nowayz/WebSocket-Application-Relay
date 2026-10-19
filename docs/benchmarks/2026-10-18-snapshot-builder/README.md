# Fresh Cargo snapshot builds and frozen outputs — 2026-10-18

Selected workflow improvement: `scripts/build-snapshot.py` refreshes a copied snapshot's file timestamps, performs one locked release build, requires Cargo to report actual recompilation of the requested executable, then freezes that executable in a new output directory with its source inventory, binary hash, compiler-artifact message and build log. It supports a native binary or an example such as the benchmark client. There is no new dependency or relay runtime change.

This addresses the stale control caught in the [timer experiment](../2026-10-18-unchanged-timers/README.md): an archived control with old timestamps reused the diagnostic executable in a shared Cargo target directory. The new helper fails if Cargo reports the selected executable as fresh/cached. Cargo's documented [`compiler-artifact` messages](https://doc.rust-lang.org/cargo/reference/external-tools.html#artifact-messages) identify the target, executable and `fresh` status; the helper checks those structured fields instead of guessing an executable from a target-directory path or accepting a generic successful build.

Use a self-contained copied archive, including local path dependencies, with the cache and output outside it:

```sh
mkdir artifacts/new-control-work
git archive HEAD Cargo.toml Cargo.lock src examples vendor |
  tar -x -C artifacts/new-control-work
python scripts/build-snapshot.py \
  --snapshot artifacts/new-control-work \
  --target-dir artifacts/capacity-client-build \
  --output artifacts/candidates/new-control \
  --bin war-relay
```

Use `--example inspection` instead of `--bin war-relay` to freeze the benchmark client in another new output directory. Output contains `build.log`, `build.json` and the executable. `build.json` has `passed: true` only after successful recompilation, unchanged snapshot contents and copying/hashing the frozen output. Failed builds retain a failed receipt and log. Existing output directories and dangling output symlinks are refused before refreshing source modification times or launching Cargo. The helper refuses Git metadata and symlinked source entries, so copied snapshots are used instead of refreshing the live checkout. The output must be separate from the Cargo cache to keep cache cleanup from deleting frozen evidence.

Run all users of a shared target directory serially, including manual Cargo commands. Refreshing all snapshot files can rebuild local vendored dependencies as well as the selected package; that cost stays outside measurements. The helper records snapshot file contents and output identity, not the complete compiler environment or registry dependency contents. Keep recording the intended base/patch and checking experiment-specific diagnostic markers. Choosing the correct control source remains part of each experiment.

Five tests pass in under one second using a tiny dependency-free Rust crate and real Cargo:

- Build a diagnostic, replace its source with control text but restore an old timestamp, and reproduce a successful stale Cargo build that still prints `diagnostic`. The helper recompiles it to print `control`, while the earlier frozen diagnostic remains unchanged.
- Build and select an example target through Cargo's artifact messages.
- Refuse a checkout marker, source symlink, reused output and dangling output symlink without mutating source modification times or earlier evidence.
- Retain a failed receipt and log for invalid Rust, without exporting an executable.
- Detect a build script changing snapshot contents and refuse to freeze the result.

The helper also builds the real native relay and `inspection` example from the same 110-file snapshot. Both receipts report `fresh: false`, preserve identical source inventories, and pin separate executable hashes. The resulting relay passes the independent 776-case / 2,311-exchange corpus. A one-second functional fixture using both new frozen executables verifies private and fanout delivery, FIFO/counters and final resource retirement with clean exits. This is build/workflow acceptance, not a new performance comparison. No long-running load test was needed.

`build.json` pins the source base and new scripts. The source patch, test source/log, real Cargo logs/receipts, corpus receipt and functional fixture receipt are retained. Run `python docs/benchmarks/2026-10-18-snapshot-builder/verify.py` to audit their identities and acceptance offline. To rerun the fast real-Cargo regressions, use `python scripts/test-build-snapshot.py`.

No manual service or convenience executable was replaced. The three pre-existing dirty source files remain byte-for-byte unchanged.
