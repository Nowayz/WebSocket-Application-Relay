#!/usr/bin/env python3
"""Matched CPU accounting/profiles; no runtime changes or ordinary benchmark data."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
CACHE = ROOT / "artifacts/reference"
sys.path.insert(0, str(ROOT / "scripts"))
spec = importlib.util.spec_from_file_location("comparison", ROOT / "scripts/compare-cpp.py")
comparison = importlib.util.module_from_spec(spec)
spec.loader.exec_module(comparison)


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def inside(args):
    original = comparison.runtime.snapshot
    count = 0
    profiler = None
    perf_log = None
    profile_command = None
    output = CACHE / f"cpp-cpu-{args.kind}-{args.repetition}.json"
    raw = output.with_suffix(".perf.data")
    perf_status = None

    def snapshot(pid):
        nonlocal count, profiler, perf_log, perf_status, profile_command
        result = original(pid)
        fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
        ticks = os.sysconf("SC_CLK_TCK")
        result["user_cpu_seconds"] = int(fields[11]) / ticks
        result["system_cpu_seconds"] = int(fields[12]) / ticks
        executable = os.readlink(f"/proc/{pid}/exe")
        if executable.endswith(("/app", "/war-relay")):
            count += 1
            if args.profile and count == 2:
                assert not raw.exists(), raw
                perf_log = output.with_suffix(".perf.log").open("w")
                profile_command = ["perf", "record", "-e", "cycles", "-F", "499",
                                   "--call-graph", "dwarf,8192", "-p", str(pid), "-o", str(raw)]
                profiler = subprocess.Popen(profile_command, stdout=perf_log, stderr=perf_log)
                time.sleep(.2)
                assert profiler.poll() is None, "perf could not attach"
            if args.profile and count == 3:
                profiler.send_signal(signal.SIGINT)
                perf_status = profiler.wait(timeout=12)
                perf_log.close()
                profiler = perf_log = None
                assert perf_status in (0, -signal.SIGINT), perf_status
        return result

    comparison.runtime.snapshot = snapshot
    trial = argparse.Namespace(kind=args.kind, case="private", control=None, cpp_preload=[],
        cpp_executable=ROOT / "artifacts/baselines/cpp-aocl-four/app", cpp_system_libraries=True,
        cpp_loop_count=4, host_netns=args.host_netns,
        relay=ROOT / "artifacts/baselines/c10m-sendmsg/war-relay",
        client=ROOT / "artifacts/baselines/active-pairs-client/inspection",
        client_source_root=ROOT / "artifacts/baselines/active-pairs-client", workers=4,
        rooms=8192, pairs=64, client_workers=8, churn_workers=0, inspectors=2,
        inspection_interval_ms=10, seconds=6, server_cpus="0-3", client_cpus="4-11",
        output=output, loader_bindings=False, audit_ids=False)
    try:
        record = comparison.trial(trial)
    finally:
        if profiler is not None:
            profiler.send_signal(signal.SIGINT)
            profiler.wait(timeout=12)
            perf_log.close()
    assert record["passed"], record.get("error")
    assert count == 4, count
    ready, measured = record["events"][:2]
    times = {key: measured["server"][key + "_cpu_seconds"] - ready["server"][key + "_cpu_seconds"]
             for key in ("user", "system")}
    verified = record["mixed"]["healthy"]["verified"]
    record.update(diagnostic_only=True, cpu_diagnostic={"profiled": args.profile,
        "repetition": args.repetition, "cpu_seconds": times,
        "cpu_ns_per_delivery": {key: value / verified * 1e9 for key, value in times.items()},
        "profile_command": profile_command, "perf_exit_code": perf_status,
        "collector": "Root collector/server/client in disposable network/PID namespaces; /proc process CPU deltas at the same ready/measured gates. Profiler attaches only between gates. No sysctl or executable changes."})
    if args.profile:
        record["cpu_diagnostic"].update(raw_profile=str(raw.relative_to(ROOT)), raw_sha256=digest(raw))
    output.write_text(json.dumps(record, indent=2) + "\n")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--inside", action="store_true")
    parser.add_argument("--kind", choices=("cpp", "rust"))
    parser.add_argument("--repetition", type=int)
    parser.add_argument("--host-netns")
    parser.add_argument("--profile", action="store_true")
    parser.add_argument("--first-repetition", type=int, default=1)
    parser.add_argument("--last-repetition", type=int, default=6)
    args = parser.parse_args()
    if args.inside:
        inside(args)
        return
    saved = ROOT / "artifacts/cpp-cpu-diagnostic.json"
    record = json.loads(saved.read_text()) if args.first_repetition > 1 else {"diagnostic_only": True, "runs": []}
    record["passed"] = False
    assert all(row["repetition"] < args.first_repetition for row in record["runs"])
    for repetition in range(args.first_repetition, args.last_repetition + 1):
        for kind in (("cpp", "rust") if repetition % 2 else ("rust", "cpp")):
            command = ["sudo", "-n", "bwrap", "--unshare-net", "--unshare-pid", "--die-with-parent",
                       "--ro-bind", "/", "/", "--bind", str(CACHE), str(CACHE), "--proc", "/proc",
                       "--dev", "/dev", "--", sys.executable, str(Path(__file__).resolve()), "--inside",
                       "--kind", kind, "--repetition", str(repetition), "--host-netns", os.readlink("/proc/self/ns/net")]
            if repetition == 1:
                command.append("--profile")
            log = ROOT / f"artifacts/cpp-cpu-{kind}-{repetition}.log"
            with log.open("w") as handle:
                completed = subprocess.run(command, cwd=ROOT, stdout=handle, stderr=handle, timeout=45)
            assert completed.returncode == 0, log.read_text()[-3000:]
            source = CACHE / f"cpp-cpu-{kind}-{repetition}.json"
            target = ROOT / "artifacts" / source.name
            shutil.copy2(source, target)
            data = json.loads(target.read_text())
            assert data["passed"] and data["client_exit_code"] == 0 and not data["server_forced_kill"]
            row = {"kind": kind, "trace": str(target.relative_to(ROOT)), "trace_sha256": digest(target),
                   "verified": data["mixed"]["healthy"]["verified"], **data["cpu_diagnostic"]}
            if repetition == 1:
                report = subprocess.run(["sudo", "-n", "perf", "report", "--stdio", "-i", str(ROOT / row["raw_profile"]),
                    "--no-children", "--sort", "dso,symbol", "--call-graph", "none", "--percent-limit", "0.1"],
                    capture_output=True, text=True, check=True)
                text = ROOT / f"artifacts/cpp-cpu-{kind}-flat.txt"
                text.write_text(report.stdout)
                row["report"] = str(text.relative_to(ROOT))
            record["runs"].append(row)
            (ROOT / "artifacts/cpp-cpu-diagnostic.json").write_text(json.dumps(record, indent=2) + "\n")
            print(json.dumps(row), flush=True)
    record["passed"] = True
    (ROOT / "artifacts/cpp-cpu-diagnostic.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main()
