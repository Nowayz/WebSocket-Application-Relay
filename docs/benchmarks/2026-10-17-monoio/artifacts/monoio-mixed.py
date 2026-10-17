#!/usr/bin/env python3
"""65-second 100000-user mixed acceptance, two selected runs and one control."""
import hashlib
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "artifacts/monoio-mixed.json"
sha = lambda path: hashlib.sha256(Path(path).read_bytes()).hexdigest()
candidate = ROOT / "artifacts/candidates/monoio-readiness/war-relay"
control = ROOT / "artifacts/candidates/monoio-persistent/war-relay"
client = ROOT / "artifacts/baselines/capacity-final/capacity"
stress = ROOT / "artifacts/baselines/capacity-final/stress"
record = {"passed": False, "runs": [], "scope": "Two sequential fresh 65-second Monoio mixed acceptance cases: initial readiness and persistent watchers. 100000 stable users at one 1KiB message/user/second, 16 churn workers, 15-second heartbeat, eight deliberately unread pressure recipients, full final fresh routing through every stable user and Close/EOF/resource retirement. Same fixed clients, four server cores 0-3, eight client cores 8-15 in the other L3 domain, 16KiB/64KiB TCP profile. Not an overload-throughput benchmark; no concurrent build/profile/load test."}
for label, binary in (("readiness", candidate), ("persistent", control)):
    path = OUT.with_name(f"{OUT.stem}-{label}.json")
    command = [sys.executable, str(ROOT / "scripts/capacity.py"), "--server", str(binary),
               "--client", str(client), "--users", "100000", "--seconds", "65",
               "--churn-workers", "16", "--ping-interval-secs", "15", "--connection-headroom", "10000",
               "--pressure-client", str(stress), "--tcp-send-buffer-bytes", "16384", "--tcp-receive-buffer-bytes", "65536",
               "--client-cpus", "8-15", "--output", str(path)]
    print(json.dumps({"started": label}), flush=True)
    with path.with_suffix(".log").open("w") as logs:
        completed = subprocess.run(command, cwd=ROOT, stdout=logs, stderr=logs, timeout=240)
    data = json.loads(path.read_text())
    assert completed.returncode == 0 and data["passed"], data.get("error")
    assert data["server_sha256"] == sha(binary)
    assert data["server_exit_code"] == data["client_exit_code"] == 0 and not data.get("forced_kill_pids")
    measured = data["events"][2]["details"]
    assert measured["round_trip"]["samples"] == 6_500_000
    row = {"kind": label, "trace": str(path.relative_to(ROOT)), "trace_sha256": sha(path),
           "passed": True, "server_sha256": sha(binary), "measurement": data["measurements"][0],
           "round_trip": measured["round_trip"], "churn": measured.get("churn"), "pressure": data["pressure"]}
    record["runs"].append(row)
    OUT.write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps({"passed": label, "round_trip": measured["round_trip"]}), flush=True)
record["passed"] = True
OUT.write_text(json.dumps(record, indent=2) + "\n")
