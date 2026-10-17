#!/usr/bin/env python3
"""Compare frozen relay binaries with one fixed, verifying client workload.

Each candidate runs in a fresh server. Repetition order rotates to balance position.
Full sampled traces are retained beside the compact comparison. No candidates overlap.
"""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import statistics
import subprocess

from stability import ROOT, STRESS, run_case, runtime


def candidate(value):
    label, separator, path = value.partition("=")
    if not separator or not label or not Path(path).is_file():
        raise argparse.ArgumentTypeError("candidate must be label=path-to-built-relay")
    return label, Path(path).resolve()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate", type=candidate, action="append", required=True)
    parser.add_argument("--server-arg", action="append", default=[], metavar="LABEL=ARG",
                        help="Append one server argument for a candidate, e.g. enabled=--idle-timeout-secs=300; repeat as needed.")
    parser.add_argument("--server-preload", action="append", default=[], metavar="LABEL=LIBRARY",
                        help="Preload one shared library into this candidate's server only; record its hash and verify it is mapped.")
    parser.add_argument("--stress", type=Path, default=STRESS)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--seconds", type=int, default=15)
    parser.add_argument("--clients", type=int, default=64)
    parser.add_argument("--mode", choices=["traffic", "fanout"], default="traffic")
    parser.add_argument("--payload-bytes", type=int, default=1024)
    parser.add_argument("--payload-sizes", type=lambda value: [int(item) for item in value.split(",")])
    parser.add_argument("--unread-clients", type=int, default=0)
    parser.add_argument("--reader-delay-ms", type=int, default=0)
    parser.add_argument("--window", type=int, default=16)
    parser.add_argument("--record-failures", action="store_true", help="Keep comparing after a failed case; exit 1 if any case fails.")
    parser.add_argument("--server-cpus", default="0-3")
    parser.add_argument("--client-cpus", default="4-7")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    labels = [label for label, _ in args.candidate]
    if len(set(labels)) != len(labels) or args.repetitions < 1 or not 1 <= args.seconds <= 3600 or not 8 <= args.payload_bytes <= 1048544:
        parser.error("use unique labels, positive repetitions, seconds in 1..=3600, and payload bytes in 8..=1048544")
    server_args = {label: [] for label in labels}
    server_preloads = {label: None for label in labels}
    for item in args.server_preload:
        label, separator, value = item.partition("=")
        if not separator or label not in server_preloads or server_preloads[label] is not None:
            parser.error("preloads require an existing candidate label and at most one library per candidate")
        try:
            server_preloads[label] = runtime.preload_library(value)
        except argparse.ArgumentTypeError as error:
            parser.error(str(error))
    for item in args.server_arg:
        label, separator, value = item.partition("=")
        if not separator or label not in server_args or not value or len(value) > 4096:
            parser.error("server arguments require an existing candidate label and a nonempty argument up to 4096 characters")
        server_args[label].append(value)
    if args.payload_sizes and (len(args.payload_sizes) > 64 or any(not 8 <= size <= 1048544 for size in args.payload_sizes)):
        parser.error("use at most 64 payload sizes in 8..=1048544")
    if args.mode != "fanout" and (args.payload_sizes or args.unread_clients or args.reader_delay_ms):
        parser.error("mixed sizes, unread clients, and reader delay require fanout mode")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    traces = args.output.with_suffix("").with_name(args.output.stem + "-traces")
    traces.mkdir(parents=True, exist_ok=True)
    output = {
        "host": platform.platform(),
        "cpu_topology": subprocess.check_output(["lscpu", "-p=CPU,CORE,SOCKET"], text=True),
        "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
        "working_tree_status": subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True),
        "server_cpus": args.server_cpus, "client_cpus": args.client_cpus,
        "seconds": args.seconds, "clients": args.clients, "repetitions": args.repetitions,
        "payload_bytes": args.payload_bytes,
        "mode": args.mode, "payload_sizes": args.payload_sizes, "unread_clients": args.unread_clients,
        "reader_delay_ms": args.reader_delay_ms, "window": args.window,
        "stress_sha256": hashlib.sha256(args.stress.read_bytes()).hexdigest(),
        "candidates": {label: {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "server_args": server_args[label],
                               "server_preload": runtime.preload_metadata(server_preloads[label])} for label, path in args.candidate},
        "notes": "Sequential fresh four-worker servers. Same fixed verifying client binary, sizes and window for each candidate. CPU sets separate client/server physical cores on the recorded host. Rates disabled; heartbeat enabled. Position rotates per repetition. CPU seconds include setup and teardown. Latency quantiles are fixed-histogram upper bounds; fanout latency begins at window publication. Summary timings include only passed cases. No frequency governor changes.",
        "runs": [], "summary": {},
    }
    for repetition in range(args.repetitions):
        shift = repetition % len(args.candidate)
        order = args.candidate[shift:] + args.candidate[:shift]
        for position, (label, binary) in enumerate(order):
            case = run_case(args.mode, args.seconds, args.clients, server_binary=binary,
                            stress_binary=args.stress, server_cpus=args.server_cpus,
                            client_cpus=args.client_cpus, payload_bytes=args.payload_bytes,
                            payload_sizes=args.payload_sizes, unread_clients=args.unread_clients,
                            reader_delay_ms=args.reader_delay_ms, window=args.window, server_args=server_args[label],
                            server_preload=server_preloads[label])
            trace = traces / f"{repetition}-{position}.json"
            trace.write_text(json.dumps(case, indent=2) + "\n")
            result = next((entry for entry in case["output"] if entry.get("event") == "result"), {})
            cpu = case["final"]["process"]["cpu_seconds"] - case["baseline"]["process"]["cpu_seconds"]
            row = {"repetition": repetition, "position": position, "candidate": label,
                   "trace": str(trace.relative_to(args.output.parent)), "passed": case["passed"],
                   "result": result, "server_cpu_seconds": cpu,
                   "server_cpu_ns_per_delivery": cpu * 1e9 / result["verified_deliveries"] if result.get("verified_deliveries") else None,
                   "peak_rss_kib": max([case["baseline"]["process"]["VmRSS"]] + [sample["process"]["VmRSS"] for sample in case["samples"]]),
                   "final_metrics": case["final"]["metrics"]}
            output["runs"].append(row)
            args.output.write_text(json.dumps(output, indent=2) + "\n")
            print(json.dumps({"event": "comparison_run", "candidate": label, "repetition": repetition,
                              "deliveries_per_second": result.get("deliveries_per_second"),
                              "cpu_ns_per_delivery": row["server_cpu_ns_per_delivery"], "passed": row["passed"]}), flush=True)
            if not case["passed"] and not args.record_failures:
                raise SystemExit(1)
    for label in labels:
        attempts = [row for row in output["runs"] if row["candidate"] == label]
        rows = [row for row in attempts if row["passed"]]
        output["summary"][label] = {
            "passed_runs": len(rows), "failed_runs": len(attempts) - len(rows),
            "median_deliveries_per_second": statistics.median(row["result"]["deliveries_per_second"] for row in rows) if rows else None,
            "min_deliveries_per_second": min(row["result"]["deliveries_per_second"] for row in rows) if rows else None,
            "max_deliveries_per_second": max(row["result"]["deliveries_per_second"] for row in rows) if rows else None,
            "median_server_cpu_ns_per_delivery": statistics.median(row["server_cpu_ns_per_delivery"] for row in rows) if rows else None,
            "median_p99_upper_ms": statistics.median(row["result"]["latency_p99_upper_ms"] for row in rows) if rows else None,
        }
    args.output.write_text(json.dumps(output, indent=2) + "\n")
    print(json.dumps({"event": "comparison_complete", "summary": output["summary"]}), flush=True)
    if not all(row["passed"] for row in output["runs"]):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
