#!/usr/bin/env python3
"""Preserve the complete current-C++/AOCL comparison without tracking ELF files."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "docs/benchmarks/2026-10-17-cpp-aocl"
MANIFEST = DEST.with_suffix(".json")
BASE = "a7adb955fc8fa075c9695551ce0c1ca45177ca58"


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def read(name):
    return json.loads((ROOT / name).read_text())


summary = read("artifacts/cpp-aocl-summary.json")
matrix = read("artifacts/cpp-aocl-four-worker.json")
assert summary["passed"] and summary["ordinary_trials"] == len(matrix["cases"]) == 50
assert sha(ROOT / summary["source"]) == summary["source_sha256"]
assert matrix["runner_sha256"] == sha(ROOT / "scripts/compare-cpp.py")
assert not subprocess.check_output(["git", "diff", BASE, "--name-only", "--", "src", "vendor", "Cargo.toml", "Cargo.lock"], cwd=ROOT)

command = ["python3", "scripts/compare-cpp.py", "--relay", "artifacts/baselines/c10m-sendmsg/war-relay",
           "--client", "artifacts/baselines/active-pairs-client/inspection",
           "--client-source-root", "artifacts/baselines/active-pairs-client", "--rebuild-cpp",
           "--workers", "4", "--rooms", "8192", "--pairs", "64", "--client-workers", "8",
           "--server-cpus", "0-3", "--client-cpus", "4-11", "--seconds", "6",
           "--repetitions", "5", "--cases", "private", "query"]
for label in ("zen5", "avx2", "dispatch"):
    command += ["--cpp-preload", f"{label}=artifacts/aocl-libmem-5.3.2-{label}/lib/libaocl-libmem.so"]
command += ["--output", "artifacts/cpp-aocl-four-worker.json"]
(ROOT / "artifacts/cpp-aocl-reproduce.json").write_text(json.dumps({
    "cwd": str(ROOT), "command": command,
    "prerequisites": "Pinned AOCL build directories and frozen relay/client artifacts identified in the manifest; bwrap, Python, Clang and upstream system libraries. Rebuilding with another compiler can change the executable hash. Run alone without profilers/builds/load tests.",
    "source_snapshots": "Archived files preserve repository-relative paths. Restore those paths in the corresponding checkout to use the diagnostic/audit helpers. No ordinary timing trial uses loader debugging."}, indent=2) + "\n")
patch = subprocess.check_output(["git", "diff", BASE, "--binary", "--", ".gitattributes", "scripts/compare-cpp.py",
                                 "docs/STATUS.md", "docs/operations.md", "docs/performance.md"], cwd=ROOT)
(ROOT / "artifacts/cpp-aocl-changes.patch").write_bytes(patch)

files = [path for path in (ROOT / "artifacts").glob("cpp-aocl-*") if path.is_file()]
files += [ROOT / ".gitattributes"]
files += [ROOT / "scripts" / name for name in ("compare-cpp.py", "benchmark.py", "inspection.py", "reference.py")]
files += [ROOT / "artifacts/baselines/cpp-aocl-four" / name for name in ("build.json", "relay.cpp")]
files += [ROOT / "artifacts/baselines/c10m-sendmsg" / name for name in ("build.json", "runtime.patch")]
files += [ROOT / "artifacts/baselines/active-pairs-client/examples/inspection.rs"]
files += list((ROOT / "artifacts/baselines/active-pairs-client/examples/inspection").glob("*.rs"))
files += [ROOT / name for name in ("artifacts/aocl-build.json", "artifacts/aocl-resolvers.json", "src/protocol.rs", "Cargo.toml", "Cargo.lock")]
archived = []
for source in sorted(set(files)):
    relative = source.relative_to(ROOT)
    target = DEST / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)
    assert sha(source) == sha(target)
    archived.append({"path": str(target.relative_to(MANIFEST.parent)), "source_path": str(relative),
                     "sha256": sha(target), "bytes": target.stat().st_size})

binaries = {}
for label, path in (("rust", "artifacts/baselines/c10m-sendmsg/war-relay"),
                    ("cpp", "artifacts/baselines/cpp-aocl-four/app"),
                    ("client", "artifacts/baselines/active-pairs-client/inspection")):
    assert sha(ROOT / path) == matrix[f"{label}_sha256"]
    binaries[label] = {"path": path, "sha256": sha(ROOT / path), "bytes": (ROOT / path).stat().st_size}

manifest = {"schema": 1, "date": "2026-10-17", "parent_revision": BASE,
            "decision": "Retain glibc defaults. Current Rust uses less process RSS but trails rebuilt C++ in median private throughput and CPU cost; C++ AOCL variants establish no useful gain in these measured workloads. Keep explicit isolated C++ preload comparisons.",
            "runtime_changed": False, "binaries_tracked": False, "binaries": binaries,
            "cpp_source_revision": matrix["upstream_commit"], "cpp_build": matrix["cpp_build"],
            "cpp_libraries": matrix["cpp_preloads"],
            "prior_aocl_build_and_validation": {"path": "2026-10-17-aocl.json", "sha256": sha(ROOT / "docs/benchmarks/2026-10-17-aocl.json")},
            "prior_fixed_client_comparison": {"path": "2026-10-16-active-pairs-cpp.json", "sha256": sha(ROOT / "docs/benchmarks/2026-10-16-active-pairs-cpp.json"),
                "association": "Same frozen verifying client SHA and adjacent source snapshot as the prior 64-pair checkpoint; no client rebuild in this study. Source snapshots are attribution, not an independently reproduced bit-identical build."},
            "summary": summary, "reproduction": {"command": command, "runner_sha256": matrix["runner_sha256"]},
            "scope": matrix["scope"], "limits": [
                "Five six-second trials per candidate/workload; all observations including low-throughput repeats retained; no statistical significance claim.",
                "Upstream C++ native optimization flags and Clang 23.1.3 versus the current portable Rust release; original binary protocol versus validated CBOR, with identical application values.",
                "About 8,320 stable users plus query/probe sessions, not a matched 100,000-user C++ capacity or fresh churn test.",
                "RSS sampled at phase gates excludes kernel TCP storage; p99 is the histogram upper bucket including client validation/windowing/scheduling.",
                "Existing idle development services retained; ordinary timing trials have no overlapping builds, profilers or other benchmark loads."],
            "files": archived}
MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n")
for row in archived:
    assert sha(MANIFEST.parent / row["path"]) == row["sha256"]
print(json.dumps({"manifest": str(MANIFEST.relative_to(ROOT)), "verified": True,
                  "files": len(archived), "bytes": sum(row["bytes"] for row in archived),
                  "ordinary_trials": summary["ordinary_trials"]}))
