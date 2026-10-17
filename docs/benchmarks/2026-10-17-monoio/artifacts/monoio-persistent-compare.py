#!/usr/bin/env python3
"""Ordinary private/query/churn comparisons with frozen binaries and verifier."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
parser = argparse.ArgumentParser()
parser.add_argument("--cases", nargs="+", choices=("private", "query", "churn"), default=["private"])
parser.add_argument("--repetitions", type=int, default=5)
parser.add_argument("--output", type=Path, default=ROOT / "artifacts/monoio-persistent-private.json")
args = parser.parse_args()
variants = {"mio": ROOT / "artifacts/candidates/input-batch/war-relay",
            "initial": ROOT / "artifacts/candidates/monoio-readiness/war-relay",
            "persistent": ROOT / "artifacts/candidates/monoio-persistent/war-relay"}
client = ROOT / "artifacts/baselines/active-pairs-client/inspection"
record = {"passed": False, "diagnostic_only": False,
          "scope": "Sequential fresh native servers, 64 private pairs/8192 idle rooms, same frozen verifier, four workers on CPUs 0-3 and client CPUs 4-7. Six-second gated traffic; query adds 200 CBOR channel queries/s, churn adds eight verified reconnect workers. Full payload, FIFO, Close/EOF, counter and cleanup checks. No builds/profiles/other load tests overlap ordinary timing.",
          "variants": {key: {"path": str(path.relative_to(ROOT)), "sha256": sha(path)} for key, path in variants.items()},
          "client": {"path": str(client.relative_to(ROOT)), "sha256": sha(client)},
          "helpers": {name: sha(ROOT / "scripts" / name) for name in ("inspection.py", "benchmark.py")},
          "runs": []}
for repetition in range(1, args.repetitions + 1):
    shift = (repetition - 1) % len(args.cases)
    for case in args.cases[shift:] + args.cases[:shift]:
        for kind in list(variants)[(repetition-1) % len(variants):] + list(variants)[:(repetition-1) % len(variants)]:
            path = args.output.with_name(f"{args.output.stem}-{case}-{kind}-{repetition}.json")
            command = [sys.executable, str(ROOT / "scripts/inspection.py"), "--server", str(variants[kind]),
                       "--client", str(client), "--client-source-root", str(client.parent), "--pairs", "64",
                       "--rooms", "8192", "--workers", "4", "--client-workers", "8", "--seconds", "6",
                       "--server-cpus", "0-3", "--client-cpus", "4-7", "--mode", "cbor" if case == "query" else "none",
                       "--churn-workers", "8" if case == "churn" else "0", "--output", str(path)]
            completed = subprocess.run(command, cwd=ROOT, capture_output=True, text=True, timeout=100)
            trace = json.loads(path.read_text())
            assert completed.returncode == 0 and trace["passed"], completed.stdout + completed.stderr + str(trace.get("error"))
            assert trace["server_sha256"] == record["variants"][kind]["sha256"]
            assert trace["client_sha256"] == record["client"]["sha256"] and not trace["profile_diagnostic"]
            assert trace["server_exit_code"] == trace["client_exit_code"] == 0 and not trace["server_forced_kill"]
            final = trace["events"][-1]
            for key in ("war_connections", "war_sessions", "war_queued_bytes", "war_read_buffer_reserved_bytes", "war_variable_bytes", "war_routing_copied_bytes_total"):
                assert final["metrics"][key] == 0, key
            mixed = trace["mixed"]
            row = {"kind": kind, "case": case, "repetition": repetition, "trace": str(path.relative_to(ROOT)),
                   "trace_sha256": sha(path), "passed": True,
                   "deliveries_per_second": mixed["healthy"]["per_second"],
                   "cpu_ns_per_delivery": mixed["server_cpu_seconds"] / mixed["healthy"]["verified"] * 1e9,
                   "p99_ms": mixed["healthy"]["p99_upper_ms"],
                   "rss_mib": trace["events"][1]["process"]["VmRSS"] / 1024,
                   "verified": mixed["healthy"]["verified"], "churn_per_second": mixed["churn"]["per_second"],
                   "churn_verified": mixed["churn"]["verified"], "queries_verified": mixed["inspection"]["verified"],
                   "normal_closes": final["details"]["counts"]["normal_closes"]}
            record["runs"].append(row)
            args.output.write_text(json.dumps(record, indent=2) + "\n")
            print(json.dumps(row), flush=True)
record["summary"] = {}
for case in args.cases:
    for kind in variants:
        rows = [row for row in record["runs"] if row["case"] == case and row["kind"] == kind]
        record["summary"][f"{case}/{kind}"] = {
            metric: {"median": statistics.median(row[metric] for row in rows),
                     "min": min(row[metric] for row in rows), "max": max(row[metric] for row in rows)}
            for metric in ("deliveries_per_second", "cpu_ns_per_delivery", "p99_ms", "rss_mib", "churn_per_second")}
record["passed"] = True
args.output.write_text(json.dumps(record, indent=2) + "\n")
print(json.dumps({"passed": True, "summary": record["summary"]}), flush=True)
