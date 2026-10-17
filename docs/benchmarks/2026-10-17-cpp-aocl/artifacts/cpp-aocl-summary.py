#!/usr/bin/env python3
"""Audit the frozen direct comparison and derive per-run-normalized summaries."""
import hashlib
import json
from pathlib import Path
import statistics

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "artifacts/cpp-aocl-four-worker.json"
OUT = ROOT / "artifacts/cpp-aocl-summary.json"


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def distribution(values):
    return {"median": statistics.median(values), "min": min(values), "max": max(values),
            "values": values}


data = json.loads(SOURCE.read_text())
kinds = ("cpp", "cpp-zen5", "cpp-avx2", "cpp-dispatch", "rust")
assert data["passed"] and len(data["cases"]) == 50
assert data["runner_sha256"] == sha(ROOT / "scripts/compare-cpp.py")
assert data["cpp_sha256"] == sha(ROOT / "artifacts/baselines/cpp-aocl-four/app")
assert data["rust_sha256"] == sha(ROOT / "artifacts/baselines/c10m-sendmsg/war-relay")
assert data["client_sha256"] == sha(ROOT / "artifacts/baselines/active-pairs-client/inspection")
for path, expected in data["cpp_build"]["upstream_sources"].items():
    assert sha(ROOT / path) == expected, path
for path, expected in data["client_sources_sha256"].items():
    assert sha(Path(data["client_source_root"]) / path) == expected, path
for library in data["cpp_preloads"].values():
    assert sha(library["path"]) == library["sha256"]

totals = {"healthy_deliveries": 0, "timed_queries": 0, "normal_closes": 0}
for row in data["cases"]:
    path = ROOT / row["trace"]
    assert sha(path) == row["trace_sha256"]
    trace = json.loads(path.read_text())
    assert row["passed"] and trace["passed"] and not trace["diagnostic_only"]
    assert trace["client_exit_code"] == 0 and not trace["server_forced_kill"]
    cpp = row["kind"].startswith("cpp")
    assert trace["server_exit_code"] == (-15 if cpp else 0)
    assert trace["client_aocl_mapping_absent"]
    assert trace["server_sha256"] == data[f"{row['kind']}_sha256"]
    assert trace["client_sha256"] == data["client_sha256"]
    assert trace["runner_sha256"] == data["runner_sha256"]
    assert trace["client_sources_sha256"] == data["client_sources_sha256"]
    assert trace["isolation"]["host_netns"] != trace["isolation"]["test_netns"]
    assert [event["event"] for event in trace["events"]] == ["ready", "measured", "done"]
    assert row["mixed"] == trace["mixed"]
    assert row["server_threads"] == (6 if cpp else 5)
    final = trace["events"][-1]
    assert final["details"]["passed"]
    assert final["details"]["counts"]["joins"] == row["normal_closes"]
    assert final["details"]["counts"]["normal_closes"] == row["normal_closes"]
    assert row["mixed"]["inspection"]["verified"] == (1200 if row["case"] == "query" else 0)
    expected_preload = data["cpp_preloads"].get(row["kind"])
    assert trace["server_preload"] == expected_preload
    if cpp:
        assert trace["listening_sockets"] == 4
        if expected_preload:
            assert trace["loaded_libraries"][expected_preload["path"]] == expected_preload["sha256"]
        else:
            assert not any("libaocl-libmem" in name for name in trace["loaded_libraries"])
    else:
        metrics = final["metrics"]
        for key in ("war_connections", "war_sessions", "war_queued_bytes", "war_read_buffer_reserved_bytes",
                    "war_variable_bytes", "war_output_queue_capacity_bytes", "war_routing_copied_bytes_total"):
            assert metrics[key] == 0, key
        assert metrics["war_routing_reused_bytes_total"] == row["mixed"]["healthy"]["verified"] * 1027
    totals["healthy_deliveries"] += row["mixed"]["healthy"]["verified"]
    totals["timed_queries"] += row["mixed"]["inspection"]["verified"]
    totals["normal_closes"] += row["normal_closes"]

groups = {}
comparisons = {}
for case in ("private", "query"):
    for kind in kinds:
        rows = [row for row in data["cases"] if row["case"] == case and row["kind"] == kind]
        assert sorted(row["repetition"] for row in rows) == [1, 2, 3, 4, 5]
        groups[f"{case}/{kind}"] = {
            "deliveries_per_second": distribution([row["mixed"]["healthy"]["per_second"] for row in rows]),
            "cpu_ns_per_delivery": distribution([row["mixed"]["server_cpu_seconds"] / row["mixed"]["healthy"]["verified"] * 1e9 for row in rows]),
            "measured_rss_mib": distribution([row["sampled_server_rss_kib"][1] / 1024 for row in rows]),
            "healthy_p99_upper_ms": distribution([row["mixed"]["healthy"]["p99_upper_ms"] for row in rows]),
            "query_p99_upper_ms": distribution([row["mixed"]["inspection"]["p99_upper_ms"] for row in rows]),
            "server_cpu_per_wall_second": distribution([row["mixed"]["server_cpu_per_wall_second"] for row in rows]),
        }
    for kind in kinds[1:]:
        comparisons[f"{case}/{kind}_versus_cpp_glibc_percent"] = {
            metric: 100 * (groups[f"{case}/{kind}"][metric]["median"] / groups[f"{case}/cpp"][metric]["median"] - 1)
            for metric in ("deliveries_per_second", "cpu_ns_per_delivery", "measured_rss_mib")
        }

bindings = json.loads((ROOT / "artifacts/cpp-aocl-bindings.json").read_text())
assert bindings["passed"] and bindings["ignored_library_rejected"] and len(bindings["runs"]) == 4
for row in bindings["runs"]:
    assert row["passed"] and row["client_aocl_mapping_absent"]
    memcpy = [line for line in row["bindings"] if "symbol `memcpy'" in line]
    assert len(memcpy) == 1
    assert ("libaocl-libmem" in memcpy[0]) == (row["name"] != "glibc")
smoke = json.loads((ROOT / "artifacts/cpp-aocl-smoke.json").read_text())
assert smoke["passed"] and len(smoke["cases"]) == 10

summary = {"passed": True, "source": str(SOURCE.relative_to(ROOT)), "source_sha256": sha(SOURCE),
           "ordinary_trials": 50, "smoke_trials": 10, "binding_diagnostics": 4,
           "ignored_preload_rejected": True, "totals": totals, "groups": groups,
           "versus_cpp_glibc_percent": comparisons,
           "method": "Five fresh trials per case/candidate. CPU cost is the median of each trial's server CPU seconds divided by verified private deliveries; query-case CPU includes query work. RSS is the measured phase gate, not peak or kernel memory. Percentiles are histogram upper buckets. All recorded runs, including lower-throughput runs, are retained."}
OUT.write_text(json.dumps(summary, indent=2) + "\n")
print(json.dumps({key: value for key, value in summary.items() if key not in ("groups", "method")}, indent=2))
for key, group in groups.items():
    print(key, json.dumps({metric: value["median"] for metric, value in group.items()}))
