# Modern C++ relay comparison archive

See [the report](../../../cpp/BENCHMARKS.md) for the results and their limits.

The accepted study contains 36 native CBOR saturation trials, six competing
observer trials, eight AOCL trials, and four 100,000-user trials with 48 million
verified timed probes. All runtime processes are fresh and sequential. Per-user
CSV files and large JSON receipts are gzip-compressed; `manifest.json` retains
both stored-file hashes and original uncompressed hashes.

`source/cpp` is the accepted C++ source snapshot, including its pinned setup,
transport patch, build, tests and comparison/analysis tools. `cpp-build.json`
binds runtime source hashes to the executable and static library. The independent
native replay reproduced both hashes exactly. The compared C++ executable is the
one in that build record. Five Rust source archives are reconstructed from their
saved base revisions and patches and checked against their build-source hashes;
these include the three original then-untracked stress-client build inputs.
No runtime binaries are committed in this archive.

The normal matrix used the exact original runner saved under
`raw/cpp-modern-final-comparison-runner.py`; its working-tree counterpart was
subsequently formatted and adds a runner hash to future traces. Other accepted
runners record their source hash in their JSON. The test fixtures used for wire
verification are also saved under `source/scripts`.

The earlier tuning trials inherit the raw key `rust` for a **C++ control**:
for counter and batch experiments it means C++ v0; for guard-release it means
C++ v2. Use the executable hashes, not that inherited label. Available original
control snapshots are under `experiments/`. The intermediate batch candidate
was overwritten before a separate source freeze, so its short tuning result is
explanatory evidence, not a claim of an independently replayable source build.
The final retained batching implementation is fully source-bound and retested.
Earlier preliminary 100k CSVs remain only in the working `artifacts` directory;
the complete committed census is the accepted four-trial study.

Verification from the repository root:

```sh
python3 cpp/verify-archive.py
```

For a fresh native build, run `cpp/setup.sh` and `cpp/build.sh`; LLVM/Clang,
llvm-ar, lld, a C compiler, make and Linux pthread/epoll support are required.
The CMake route and library embedding example are in `cpp/README.md`. To rerun
the original comparisons, rebuild the recorded Rust sources/clients with their
recorded compiler/profile and place them at the frozen paths used by the saved
runner commands. Do not substitute a new client build while attributing its
results to an old binary hash. The AOCL trial additionally requires the dispatcher
build recorded in `aocl-build.json`; it was not adopted.

Population memory excludes kernel TCP and client memory. The C++ operational
feature gaps remain explicit in the report. No existing manual relay service was
replaced, and these measurements are not a production deployment or acceptance.
