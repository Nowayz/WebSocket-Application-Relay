"""Ordinary, sequential comparisons of frozen readiness/completion runtimes."""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
parser = argparse.ArgumentParser()
parser.add_argument("--cases", nargs="+", choices=["private", "capacity"], default=["private", "capacity"])
parser.add_argument("--variants", nargs="+", choices=["mio", "readiness", "completion", "bounded", "timer"], default=["mio", "readiness", "completion"])
parser.add_argument("--repetitions", type=int, default=3)
parser.add_argument("--label", default="monoio-completion")
args = parser.parse_args()
variants = {name: ROOT / path for name, path in {
    "mio": "artifacts/candidates/input-batch/war-relay",
    "timer": "artifacts/candidates/timer-inplace/war-relay",
    "readiness": "artifacts/candidates/monoio-final/war-relay",
    "completion": "artifacts/candidates/monoio-completion/war-relay",
    "bounded": "artifacts/candidates/monoio-completion-bounded/war-relay",
}.items() if name in args.variants}
clients = {"private": ROOT / "artifacts/baselines/active-pairs-client/inspection",
           "capacity": ROOT / "artifacts/baselines/capacity-final/capacity"}
record = {"passed": False, "diagnostic_only": False, "runs": [],
          "scope": "Fresh sequential four-worker servers on cores0-3, identical frozen clients and bodies. Private:64 pairs/8192 idle rooms, client cores4-7, six seconds. Capacity:100000 users/16-member rooms, 1KiB bodies, ten offered messages/user/s for five seconds plus drain, client cores8-15,16KiB/64KiB TCP. Three rotating backend orders. No build/profile/load overlap. Exact payload/FIFO/counters/Close/EOF/resource checks. Completion changes quiescent receive ownership: at most 28 admitted bytes per idle user; original worker bounds stay enforced for Mio/readiness. Normal hard staging quota unchanged.",
          "variants": {name: {"path": str(path.relative_to(ROOT)), "sha256": sha(path)} for name, path in variants.items()},
          "clients": {name: {"path": str(path.relative_to(ROOT)), "sha256": sha(path)} for name, path in clients.items()},
          "helpers": {name: sha(ROOT / name) for name in ("scripts/inspection.py", "scripts/capacity.py", "scripts/capacity_pressure.py", "scripts/benchmark.py")}}
for case in args.cases:
    output = ROOT / f"artifacts/{args.label}-{case}.json"
    cohort = {**record, "runs": []}
    for repetition in range(args.repetitions):
        names = list(variants)
        shift = repetition % len(names)
        for kind in names[shift:] + names[:shift]:
            trace = output.with_name(f"{output.stem}-{kind}-{repetition + 1}.json")
            command = [sys.executable, str(ROOT / f"scripts/{'inspection' if case == 'private' else 'capacity'}.py"),
                       "--server", str(variants[kind]), "--client", str(clients[case]),
                       "--read-storage", "completion" if kind in ("completion", "bounded") else "worker", "--output", str(trace)]
            if case == "private":
                command += ["--client-source-root", str(clients[case].parent), "--pairs", "64", "--rooms", "8192",
                            "--workers", "4", "--client-workers", "8", "--seconds", "6", "--mode", "none",
                            "--churn-workers", "0", "--server-cpus", "0-3", "--client-cpus", "4-7"]
            else:
                command += ["--users", "100000", "--seconds", "5", "--messages-per-second", "10", "--client-cpus", "8-15",
                            "--tcp-send-buffer-bytes", "16384", "--tcp-receive-buffer-bytes", "65536"]
            print(json.dumps({"started": case, "variant": kind, "repetition": repetition + 1}), flush=True)
            with trace.with_suffix(".log").open("w") as logs:
                completed = subprocess.run(command, cwd=ROOT, stdout=logs, stderr=logs, timeout=240)
            data = json.loads(trace.read_text())
            assert completed.returncode == 0 and data["passed"], (case, kind, data.get("error"))
            assert data["server_sha256"] == cohort["variants"][kind]["sha256"]
            assert data["client_sha256"] == cohort["clients"][case]["sha256"]
            assert data["server_exit_code"] == data["client_exit_code"] == 0
            assert not data.get("server_forced_kill") and not data.get("forced_kill_pids")
            if case == "private":
                measurement = data["mixed"]
                count = measurement["healthy"]["verified"]
                rate = measurement["healthy"]["per_second"]
                cpu = measurement["server_cpu_seconds"]
                p99 = measurement["healthy"]["p99_upper_ms"]
                rss = data["events"][1]["process"]["VmRSS"]
                fds = None
            else:
                measurement = data["measurements"][0]
                count = data["events"][2]["details"]["round_trip"]["samples"]
                rate = measurement["stable_deliveries_per_second"]
                cpu = measurement["server_cpu_seconds"]
                p99 = data["events"][2]["details"]["round_trip"]["p99_upper_ms"]
                rss = data["events"][1]["server"]["VmRSS"]
                fds = data["events"][1]["server"]["fd_count"]
            row = {"variant": kind, "repetition": repetition + 1, "passed": True,
                   "trace": str(trace.relative_to(ROOT)), "trace_sha256": sha(trace), "verified": count,
                   "deliveries_per_second": rate, "cpu_ns_per_delivery": cpu / count * 1e9,
                   "p99_ms": p99, "ready_rss_mib": rss / 1024, "ready_fd_count": fds,
                   "ready_staging_bytes": data["events"][1]["metrics"]["war_read_buffer_reserved_bytes"]}
            cohort["runs"].append(row)
            output.write_text(json.dumps(cohort, indent=2) + "\n")
            print(json.dumps(row), flush=True)
    cohort["summary"] = {}
    for kind in variants:
        rows = [row for row in cohort["runs"] if row["variant"] == kind]
        cohort["summary"][kind] = {metric: {"median": statistics.median(row[metric] for row in rows),
            "min": min(row[metric] for row in rows), "max": max(row[metric] for row in rows)}
            for metric in ("deliveries_per_second", "cpu_ns_per_delivery", "p99_ms", "ready_rss_mib", "ready_staging_bytes")}
    cohort["passed"] = True
    output.write_text(json.dumps(cohort, indent=2) + "\n")
    print(json.dumps({"passed": case, "summary": cohort["summary"]}), flush=True)
