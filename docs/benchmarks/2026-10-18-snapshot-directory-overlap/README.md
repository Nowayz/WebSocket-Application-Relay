# Keep copied source outside the Cargo cache — 2026-10-18

Selected build-workflow fix: snapshot, target-cache and frozen-output directories must be mutually separate after resolving aliases. The helper already rejected a cache inside the source tree, but accepted a source tree inside the cache. That layout permits a normal full cache cleanup to erase the copied source: [Cargo documents that a full clean deletes the entire target directory](https://doc.rust-lang.org/cargo/commands/cargo-clean.html). No destructive cleanup was needed or run to test this change.

The original helper successfully built and returned `passed: true` for a real tiny crate nested beneath its target cache, reproducing the missing guard. The selected preflight rejects that layout before refreshing any source modification times, creating an output directory or invoking Cargo. It also rejects the inverse relationship, identical source/cache paths, and a cache alias resolving to a source ancestor. Snapshot/output overlap is checked symmetrically; reused output and cache/output checks remain in place.

The regression fails against the original helper. All six real-Cargo tests pass after the fix in 0.93 seconds. The four overlap layouts assert unchanged source modification times and no output creation. Existing tests still reproduce/recover stale Cargo output, build an example, preserve earlier frozen evidence, reject checkout/source/output symlinks, retain compile-failure receipts and reject source changes during compilation. Syntax and diff checks pass. No relay runtime code, executables or load tests changed.

`build.json` pins the source base and both scripts. The selected patch, exact scripts and original failing/passing logs are retained. Run `python docs/benchmarks/2026-10-18-snapshot-directory-overlap/verify.py` for an offline checksum/log audit, or `python scripts/test-build-snapshot.py` to rerun the subsecond real-Cargo suite.

Running services and the three pre-existing dirty source files remain unchanged.
