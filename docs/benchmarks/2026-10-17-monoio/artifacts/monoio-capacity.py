#!/usr/bin/env python3
"""Sequential ordinary 100,000-user overload comparison."""
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "artifacts/monoio-capacity.json"
variants = {"mio": ROOT / "artifacts/candidates/input-batch/war-relay",
            "initial": ROOT / "artifacts/candidates/monoio-readiness/war-relay",
            "persistent": ROOT / "artifacts/candidates/monoio-persistent/war-relay"}
client = ROOT / "artifacts/baselines/capacity-final/capacity"
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
record = {"passed": False, "diagnostic_only": False, "runs": [],
          "variants": {key: {"path": str(path.relative_to(ROOT)), "sha256": sha(path)} for key, path in variants.items()},
          "client": {"path": str(client.relative_to(ROOT)), "sha256": sha(client)},
          "scope": "Three rotating sequential pairs of fresh four-worker servers. 100000 users, room size16, 1KiB bodies, 10 offered messages/user/s for five seconds, 16KiB send/64KiB receive socket requests. Server cores0-3, fixed eight-worker client cores8-15 within the other L3 domain. Ready/measured/done gates, all routed bodies, normal Close/EOF and exact counters/resource retirement verified by capacity.py. No builds/profiles/other load tests overlap timings."}
for repetition in range(3):
    for kind in list(variants)[repetition % len(variants):] + list(variants)[:repetition % len(variants)]:
        path = OUT.with_name(f"{OUT.stem}-{kind}-{repetition + 1}.json")
        command = [sys.executable, str(ROOT / "scripts/capacity.py"), "--server", str(variants[kind]),
                   "--client", str(client), "--users", "100000", "--seconds", "5", "--messages-per-second", "10",
                   "--client-cpus", "8-15", "--tcp-send-buffer-bytes", "16384", "--tcp-receive-buffer-bytes", "65536", "--output", str(path)]
        with path.with_suffix(".log").open("w") as log:
            completed = subprocess.run(command, cwd=ROOT, stdout=log, stderr=log, timeout=180)
        data = json.loads(path.read_text())
        assert completed.returncode == 0 and data["passed"], data.get("error")
        measurement = data["measurements"][0]
        event = data["events"][2]["details"]
        row = {"kind": kind, "repetition": repetition + 1, "passed": True,
               "ready_rss_kib": data["events"][1]["server"]["VmRSS"],
               "ready_fd_count": data["events"][1]["server"]["fd_count"],
               "trace": str(path.relative_to(ROOT)), "trace_sha256": sha(path),
               "deliveries_per_second": measurement["stable_deliveries_per_second"],
               "cpu_ns_per_delivery": measurement["server_cpu_seconds"] * 1e9 / event["round_trip"]["samples"],
               "verified": event["round_trip"]["samples"], "p99_ms": event["round_trip"]["p99_upper_ms"],
               "user_cpu_seconds": measurement["server_user_cpu_seconds"],
               "system_cpu_seconds": measurement["server_system_cpu_seconds"]}
        record["runs"].append(row)
        OUT.write_text(json.dumps(record, indent=2) + "\n")
        print(json.dumps(row), flush=True)
record["summary"] = {}
for kind in variants:
    rows = [row for row in record["runs"] if row["kind"] == kind]
    record["summary"][kind] = {metric: {"median": statistics.median(row[metric] for row in rows),
        "min": min(row[metric] for row in rows), "max": max(row[metric] for row in rows)}
        for metric in ("deliveries_per_second", "cpu_ns_per_delivery", "p99_ms")}
record["passed"] = True
OUT.write_text(json.dumps(record, indent=2) + "\n")
print(json.dumps({"passed": True, "summary": record["summary"]}), flush=True)
