#!/usr/bin/env python3
"""Ordinary rotating 100,000-user comparisons; no diagnostics or concurrent jobs."""
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / "artifacts/baselines/c10m-sendmsg/war-relay"
CLIENT = ROOT / "artifacts/baselines/capacity-final/capacity"
labels = ["glibc", "zen5", "avx2"]
record = {"server_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(),
          "client_sha256": hashlib.sha256(CLIENT.read_bytes()).hexdigest(),
          "runs": [], "summary": {}, "passed": False}
destination = ROOT / "artifacts/aocl-capacity.json"
for repetition in range(3):
    order = labels[repetition:] + labels[:repetition]
    for position, label in enumerate(order):
        output = ROOT / f"artifacts/aocl-capacity-{repetition}-{position}-{label}.json"
        command = [sys.executable, str(ROOT/"scripts/capacity.py"), "--server", str(BINARY),
                   "--client", str(CLIENT), "--users", "100000", "--room-size", "16", "--seconds", "5",
                   "--messages-per-second", "10", "--tcp-send-buffer-bytes", "16384",
                   "--tcp-receive-buffer-bytes", "65536", "--connection-headroom", "10000", "--output", str(output)]
        if label != "glibc":
            command += ["--server-preload", str(ROOT/f"artifacts/aocl-libmem-5.3.2-{label}/lib/libaocl-libmem.so")]
        with output.with_suffix(".log").open("w") as log:
            run = subprocess.run(command, cwd=ROOT, stdout=log, stderr=log, timeout=300)
        result = json.loads(output.read_text())
        assert run.returncode == 0 and result["passed"], (label, result.get("error"))
        measurement = result["measurement"]
        row = {"repetition": repetition, "position": position, "candidate": label, "trace": str(output.relative_to(ROOT)),
               "command": command, "passed": True, "server_preload": result.get("server_preload"),
               "measurement": measurement,
               "server_cpu_ns_per_delivery": measurement["server_cpu_seconds"]*1e9/measurement["round_trip"]["samples"]}
        record["runs"].append(row)
        destination.write_text(json.dumps(record, indent=2)+"\n")
        print(json.dumps({"candidate": label, "repetition": repetition, "passed": True,
                          "dps": measurement["stable_deliveries_per_second"], "cpu_ns": row["server_cpu_ns_per_delivery"]}), flush=True)
for label in labels:
    rows = [row for row in record["runs"] if row["candidate"] == label]
    rates = [row["measurement"]["stable_deliveries_per_second"] for row in rows]
    record["summary"][label] = {"passed_runs": len(rows), "median_deliveries_per_second": statistics.median(rates),
        "min_deliveries_per_second": min(rates), "max_deliveries_per_second": max(rates),
        "median_server_cpu_ns_per_delivery": statistics.median(row["server_cpu_ns_per_delivery"] for row in rows),
        "median_p99_upper_ms": statistics.median(row["measurement"]["round_trip"]["p99_upper_ms"] for row in rows),
        "median_server_user_cpu_seconds": statistics.median(row["measurement"]["server_user_cpu_seconds"] for row in rows),
        "median_server_system_cpu_seconds": statistics.median(row["measurement"]["server_system_cpu_seconds"] for row in rows)}
record["passed"] = True
destination.write_text(json.dumps(record, indent=2)+"\n")
print(json.dumps({"complete": True, "summary": record["summary"]}), flush=True)
