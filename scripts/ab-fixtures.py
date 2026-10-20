#!/usr/bin/env python3
"""Run the measurement fixtures against two release builds, alternating.

Both trees must already contain release binaries. The fixtures themselves
are taken from the candidate tree; only the server and client binaries
differ. Every receipt is written under <output>/<side>/<workload>-<n>.json,
and a summary table of medians is printed and written to <output>/summary.md.
"""
import argparse
import json
import statistics
import subprocess
import sys
from pathlib import Path

PIN = ["--server-cpus", "0-1", "--client-cpus", "2-3"]
SERVER_PIN = ["--server-cpus", "0-1"]


def workloads(tree):
    server = ["--server", f"{tree}/target/release/war-relay"]
    capacity = ["--client", f"{tree}/target/release/examples/capacity"]
    inspection = ["--client", f"{tree}/target/release/examples/inspection"]
    return {
        "private-rings": ["scripts/capacity.py", *server, *capacity, "--users", "2048", "--seconds", "5",
                          "--messages-per-second", "20", "--churn-workers", "2", "--ping-interval-secs", "1",
                          "--tcp-send-buffer-bytes", "16384", "--tcp-receive-buffer-bytes", "65536",
                          "--workers", "2", "--client-workers", "2", *PIN],
        "room-churn-fanout": ["scripts/inspection.py", *server, *inspection, "--rooms", "0", "--pairs", "16",
                              "--mode", "none", "--inspectors", "0", "--churn-workers", "4",
                              "--fanout-readers", "32", "--fanout-churn-every", "2", "--fanout-window", "1",
                              "--workers", "2", "--client-workers", "2", "--seconds", "4", *PIN],
        "global-broadcast": ["scripts/inspection.py", *server, *inspection, "--rooms", "0", "--pairs", "0",
                             "--mode", "none", "--inspectors", "0", "--churn-workers", "0",
                             "--fanout-readers", "2048", "--fanout-global-broadcast", "--fanout-window", "2",
                             "--fanout-windows-per-second", "1", "--workers", "2", "--client-workers", "2",
                             "--seconds", "4", *PIN],
        "observers": ["scripts/inspection.py", *server, *inspection, "--rooms", "0", "--pairs", "16",
                      "--mode", "none", "--inspectors", "0", "--churn-workers", "2", "--fanout-readers", "256",
                      "--fanout-global-observers", "--fanout-window", "1", "--workers", "2",
                      "--client-workers", "2", "--seconds", "4", *PIN],
        "observer-barrier": ["scripts/observer-barrier.py", *server, "--workers", "2", "--audience", "2048",
                             "--queue-messages", "1", *SERVER_PIN],
        "observer-messages": ["scripts/observer-message-fairness.py", *server, "--workers", "2",
                              "--audience", "2048", "--queue-messages", "1", "--kind", "channel", *SERVER_PIN],
        "shutdown": ["scripts/shutdown-capacity.py", *server, "--workers", "2", "--users", "2048",
                     "--prime-broadcast", *SERVER_PIN],
    }


def numbers(value, prefix=""):
    """Flatten every numeric leaf of a receipt into dotted keys."""
    if isinstance(value, bool):
        return {}
    if isinstance(value, (int, float)):
        return {prefix.rstrip("."): float(value)}
    if isinstance(value, dict):
        out = {}
        for key, item in value.items():
            out.update(numbers(item, f"{prefix}{key}."))
        return out
    if isinstance(value, list) and value and all(isinstance(v, dict) for v in value):
        out = {}
        for index, item in enumerate(value):
            out.update(numbers(item, f"{prefix}{index}."))
        return out
    return {}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--base", type=Path, required=True)
    parser.add_argument("--candidate", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--only", help="Comma-separated workload names")
    args = parser.parse_args()
    sides = {"base": args.base.resolve(), "candidate": args.candidate.resolve()}
    selected = set(args.only.split(",")) if args.only else None
    receipts = {}
    for repeat in range(args.repeats):
        order = list(sides.items()) if repeat % 2 == 0 else list(reversed(list(sides.items())))
        for name in workloads("x"):
            if selected and name not in selected:
                continue
            for side, tree in order:
                command = [sys.executable, "-I"] + workloads(tree)[name]
                command[2] = str(args.candidate.resolve() / command[2])
                out = args.output / side / f"{name}-{repeat}.json"
                out.parent.mkdir(parents=True, exist_ok=True)
                command += ["--output", str(out)]
                print(f"[{repeat}] {name} {side}", flush=True)
                result = subprocess.run(command, cwd=args.candidate.resolve(), capture_output=True, text=True)
                if result.returncode != 0:
                    print(result.stdout[-3000:], result.stderr[-3000:], flush=True)
                    receipts.setdefault(name, {}).setdefault(side, []).append(None)
                    continue
                receipts.setdefault(name, {}).setdefault(side, []).append(json.loads(out.read_text()))
    lines = ["| workload | metric | base median | candidate median | change |", "|---|---|---|---|---|"]
    for name, by_side in receipts.items():
        flat = {side: [numbers(r) for r in runs if r] for side, runs in by_side.items()}
        failures = {side: sum(r is None for r in runs) for side, runs in by_side.items()}
        if any(failures.values()):
            lines.append(f"| {name} | failed runs | {failures.get('base', 0)} | {failures.get('candidate', 0)} | |")
        keys = set.intersection(*(set(k for run in runs for k in run) for runs in flat.values() if runs)) if all(flat.values()) else set()
        for key in sorted(keys):
            if not any(token in key for token in ("_ms", "per_second", "cpu_seconds", "seconds", "lifecycle")):
                continue
            if any(token in key for token in ("timestamp", "monotonic", "sha", "pid", "elapsed_seconds", "scheduled")):
                continue
            base = statistics.median(run[key] for run in flat["base"] if key in run)
            cand = statistics.median(run[key] for run in flat["candidate"] if key in run)
            if base == 0 and cand == 0:
                continue
            change = "" if base == 0 else f"{(cand - base) / base * 100:+.1f}%"
            lines.append(f"| {name} | {key} | {base:.4g} | {cand:.4g} | {change} |")
    summary = "\n".join(lines)
    print(summary)
    (args.output / "summary.md").write_text(summary + "\n")


if __name__ == "__main__":
    main()
