#!/usr/bin/env python3
"""Audit and save matched CPU diagnostics independently of ordinary timings."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import statistics
import subprocess

ROOT = Path(__file__).resolve().parents[1]
DEST = ROOT / "docs/benchmarks/2026-10-17-cpp-cpu"
MANIFEST = DEST.with_suffix(".json")
BASE = "b51a5e65f9eccd026307077d24f18dafce20e7ed"


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


data = json.loads((ROOT / "artifacts/cpp-cpu-diagnostic.json").read_text())
assert data["passed"] and len(data["runs"]) == 12
expected = {"cpp": "0f07d539c145f0be960af3b27649126d3bafd7ec2015e6c9e2444cde5fe587b4",
            "rust": "c27ff051a53e8edbef3070d9e328aa262a86fbb61898f882db6427ada7cdc988"}
profiles = []
verified = closes = 0
for row in data["runs"]:
    path = ROOT / row["trace"]
    assert sha(path) == row["trace_sha256"]
    trace = json.loads(path.read_text())
    assert trace["passed"] and trace["diagnostic_only"] and trace["client_exit_code"] == 0
    assert trace["server_sha256"] == expected[row["kind"]]
    assert trace["client_sha256"] == "c69a691b6e2cb12ea8ffd566eb7998ace9123c6c34815ee577be87da79f48569"
    assert trace["runner_sha256"] == sha(ROOT / "scripts/compare-cpp.py")
    assert trace["server_exit_code"] == (-15 if row["kind"] == "cpp" else 0)
    assert not trace["server_forced_kill"] and trace["client_aocl_mapping_absent"]
    assert trace["server_preload"] is None
    assert [event["event"] for event in trace["events"]] == ["ready", "measured", "done"]
    assert abs(sum(row["cpu_seconds"].values()) - trace["mixed"]["server_cpu_seconds"]) < 1e-8
    final = trace["events"][-1]
    assert final["details"]["passed"]
    assert final["details"]["counts"]["joins"] == final["details"]["counts"]["normal_closes"]
    if row["kind"] == "rust":
        for key in ("war_connections", "war_sessions", "war_queued_bytes", "war_read_buffer_reserved_bytes", "war_routing_copied_bytes_total"):
            assert final["metrics"][key] == 0, key
        assert final["metrics"]["war_routing_reused_bytes_total"] == row["verified"] * 1027
    else:
        assert trace["listening_sockets"] == 4
    verified += row["verified"]
    closes += final["details"]["counts"]["normal_closes"]
    if row["profiled"]:
        raw = ROOT / row["raw_profile"]
        assert sha(raw) == row["raw_sha256"]
        report = (ROOT / row["report"]).read_text()
        assert int(re.search(r"Total Lost Samples: (\d+)", report).group(1)) == 0
        log = ROOT / "artifacts/reference" / f"cpp-cpu-{row['kind']}-1.perf.log"
        samples = int(re.search(r"\((\d+) samples\)", log.read_text()).group(1))
        target = ROOT / "artifacts" / log.name
        shutil.copy2(log, target)
        profiles.append({"kind": row["kind"], "samples": samples, "lost_samples": 0,
                         "raw_path": row["raw_profile"], "raw_sha256": row["raw_sha256"],
                         "raw_bytes": raw.stat().st_size, "raw_tracked": False,
                         "cpu_ns_per_delivery": row["cpu_ns_per_delivery"]})

accounting = {}
for kind in ("cpp", "rust"):
    rows = [row for row in data["runs"] if row["kind"] == kind and not row["profiled"]]
    assert sorted(row["repetition"] for row in rows) == [2, 3, 4, 5, 6]
    accounting[kind] = {}
    for part in ("user", "system", "total"):
        values = [sum(row["cpu_ns_per_delivery"].values()) if part == "total" else row["cpu_ns_per_delivery"][part] for row in rows]
        accounting[kind][part] = {"median_ns_per_delivery": statistics.median(values),
                                 "min": min(values), "max": max(values), "values": values}
accounting["rust_minus_cpp_median_ns"] = {part: accounting["rust"][part]["median_ns_per_delivery"] - accounting["cpp"][part]["median_ns_per_delivery"] for part in ("user", "system", "total")}

assert not subprocess.check_output(["git", "diff", BASE, "--name-only", "--", "src", "vendor", "Cargo.toml", "Cargo.lock", "scripts"], cwd=ROOT)
patch = subprocess.check_output(["git", "diff", BASE, "--binary", "--", ".gitattributes", "docs/STATUS.md", "docs/performance.md"], cwd=ROOT)
(ROOT / "artifacts/cpp-cpu-notes.patch").write_bytes(patch)
files = [path for path in (ROOT / "artifacts").glob("cpp-cpu-*") if path.is_file()]
files += [ROOT / ".gitattributes", ROOT / "scripts/compare-cpp.py", ROOT / "scripts/inspection.py", ROOT / "scripts/benchmark.py", ROOT / "scripts/reference.py"]
archived = []
for source in sorted(set(files)):
    target = DEST / source.relative_to(ROOT)
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, target)
    archived.append({"path": str(target.relative_to(MANIFEST.parent)), "source_path": str(source.relative_to(ROOT)),
                     "sha256": sha(target), "bytes": target.stat().st_size})
manifest = {"schema": 1, "date": "2026-10-17", "parent_revision": BASE, "diagnostic_only": True,
            "runtime_changed": False, "passed": True, "runs": data["runs"], "profiles": profiles,
            "unprofiled_cpu_accounting": accounting, "verified_private_deliveries": verified, "normal_closes": closes,
            "ordinary_comparison": "2026-10-17-cpp-aocl.json",
            "interpretation": "Additional user CPU is consistent with the annotated global counters, registry/read ownership and output/budget bookkeeping. Named CBOR helper self samples are small. Sampling and accounting identify costs but are not an ablation proving a recoverable speedup or exact cache-coherence cost.",
            "method": "Same frozen native-protocol binaries/client, four workers, 64 active pairs/8192 idle rooms, 6 seconds, cores 0-3 server and 4-11 client. Two 499 Hz cycles profiles (8192-byte DWARF stacks), then five unprofiled accounting repeats per implementation. /proc user/system ticks at the ready/measured gates sum exactly to the controller's CPU delta. Each fresh namespace runs sequentially. Root is limited to disposable diagnostic processes; no sysctl changes. Initial collector did repetitions 1-3; its extension changes only the outer repetition range and resumes at 4-6. Both versions retained. Profiles and root accounting are separate from the earlier ordinary medians. Marginal medians of user/system CPU need not add to the median total.",
            "limits": ["Root diagnostic processes versus the unprivileged ordinary benchmark; short plaintext loopback; no TLS/100000-user/churn study.",
                       "Tick accounting and cycle samples have different units; per-symbol cycle fractions are not percentages of time or an exact breakdown of the ordinary 16.29 percent CPU gap.",
                       "Sampling skid/inlining prevent exact attribution to one adjacent atomic instruction; no direct cache-miss or lock-wait counter measurement.",
                       "No server optimization or new compiler-target ablation in this diagnostic."],
            "files": archived}
MANIFEST.write_text(json.dumps(manifest, indent=2) + "\n")
for entry in archived:
    assert sha(MANIFEST.parent / entry["path"]) == entry["sha256"]
print(json.dumps({"passed": True, "files": len(archived), "bytes": sum(row["bytes"] for row in archived),
                  "verified_private_deliveries": verified, "normal_closes": closes,
                  "unprofiled_cpu_accounting": accounting}, indent=2))
