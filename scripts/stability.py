#!/usr/bin/env python3
"""Run sequential, sampled real-socket stability checks in fresh relay processes.

Build first: cargo build --release --bins --example stress
The runner records failures as evidence as well as successful workloads.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import selectors
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("war_benchmark", ROOT / "scripts/benchmark.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)
STRESS = ROOT / "target/release/examples/stress"


def sample(pid, admin, started):
    tick = time.monotonic()
    observed = runtime.metrics(admin)
    return {"seconds": time.monotonic() - started, "admin_latency_ms": (time.monotonic() - tick) * 1000,
            "process": runtime.snapshot(pid), "metrics": observed}


def run_case(mode, seconds, clients, *, server_binary=None, stress_binary=None,
             server_cpus=None, client_cpus=None, payload_bytes=None):
    port, admin = runtime.free_port(), runtime.free_port()
    env = {key: value for key, value in os.environ.items() if not key.startswith("WAR_")}
    with tempfile.TemporaryFile() as logs:
        server_command = [
            str(Path(server_binary or runtime.SERVER).resolve()), "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}",
            "--workers", "4", "--messages-per-second", "0", "--bytes-per-second", "0",
            "--max-message-bytes", "1048576", "--queue-bytes", "1048586",
        ]
        if server_cpus:
            server_command = ["taskset", "-c", server_cpus] + server_command
        server = subprocess.Popen(server_command, stdout=logs, stderr=logs, env=env)
        process = None
        try:
            started = time.monotonic()
            while True:
                if server.poll() is not None:
                    logs.seek(0)
                    raise RuntimeError(logs.read().decode())
                try:
                    baseline = sample(server.pid, admin, started)
                    break
                except OSError:
                    if time.monotonic() - started > 5:
                        raise
                    time.sleep(0.02)
            command = [str(Path(stress_binary or STRESS).resolve()), "--url", f"ws://127.0.0.1:{port}", "--mode", mode,
                       "--clients", str(clients), "--duration-secs", str(seconds),
                       "--payload-bytes", str(payload_bytes if payload_bytes is not None else (65536 if mode == "slow" else 1024))]
            if client_cpus:
                command = ["taskset", "-c", client_cpus] + command
            process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env)
            with selectors.DefaultSelector() as selector:
                selector.register(process.stdout, selectors.EVENT_READ)
                if not selector.select(timeout=10):
                    raise TimeoutError("stress workload did not become ready")
                ready = process.stdout.readline()
            if not ready:
                raise RuntimeError(process.stderr.read())
            print(json.dumps({"event": "case_started", "mode": mode, "seconds": seconds, "clients": clients}), flush=True)
            observations, errors = [], []
            while process.poll() is None:
                if time.monotonic() - started > seconds + 40:
                    raise TimeoutError("stress workload exceeded its outer deadline")
                try:
                    observations.append(sample(server.pid, admin, started))
                except OSError as error:
                    errors.append({"seconds": time.monotonic() - started, "error": str(error)})
                time.sleep(0.25)
            stdout, stderr = process.communicate(timeout=2)
            output = [json.loads(line) for line in (ready + stdout).splitlines() if line]
            cleanup_start = time.monotonic()
            while True:
                final = sample(server.pid, admin, started)
                metrics = final["metrics"]
                clean = all(int(metrics[key]) == 0 for key in ["war_connections", "war_sessions", "war_queued_bytes", "war_variable_bytes"]) and int(metrics["war_channels"]) == 1
                if clean or time.monotonic() - cleanup_start > 12:
                    break
                time.sleep(0.05)
            result_output = next((entry for entry in output if entry.get("event") == "result"), None)
            counts_match = result_output is not None and result_output["verified_deliveries"] > 0 and int(metrics["war_incoming_messages_total"]) == result_output["verified_deliveries"] and int(metrics["war_joined_total"]) == result_output["joins"]
            pressure_valid = mode != "slow" or (int(metrics["war_slow_consumers_total"]) == 1 and result_output is not None and result_output["disconnect_notifications"] == 1)
            result = {"mode": mode, "duration_seconds": seconds, "clients": clients,
                      "command": command, "server_command": server_command,
                      "server_cpus": server_cpus, "client_cpus": client_cpus,
                      "baseline": baseline, "samples": observations,
                      "admin_errors": errors, "output": output, "stderr": stderr,
                      "exit_code": process.returncode, "cleanup_seconds": time.monotonic() - cleanup_start,
                      "cleaned_up": clean, "final": final,
                      "counts_match": counts_match, "pressure_valid": pressure_valid,
                      "passed": process.returncode == 0 and clean and not errors and counts_match and pressure_valid}
            print(json.dumps({"event": "case_finished", "mode": mode, "passed": result["passed"], "result": output[-1] if output else None,
                              "peak_server_rss_kib": max([baseline["process"]["VmRSS"]] + [s["process"]["VmRSS"] for s in observations]),
                              "cleanup_seconds": result["cleanup_seconds"]}), flush=True)
            return result
        finally:
            if process is not None and process.poll() is None:
                process.kill()
                process.communicate()
            server.terminate()
            try:
                server.wait(timeout=12)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--traffic-secs", type=int, default=30)
    parser.add_argument("--churn-secs", type=int, default=20)
    parser.add_argument("--slow-secs", type=int, default=12)
    parser.add_argument("--clients", type=int, default=64)
    parser.add_argument("--modes", nargs="+", choices=["traffic", "churn", "slow"], default=["traffic", "churn", "slow"])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--server", type=Path, default=runtime.SERVER)
    parser.add_argument("--stress", type=Path, default=STRESS)
    parser.add_argument("--server-cpus")
    parser.add_argument("--client-cpus")
    parser.add_argument("--payload-bytes", type=int)
    args = parser.parse_args()
    output = {"host": platform.platform(), "server_sha256": hashlib.sha256(args.server.read_bytes()).hexdigest(),
              "stress_sha256": hashlib.sha256(args.stress.read_bytes()).hexdigest(),
              "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "working_tree_status": subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True),
              "source_sha256": {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(list((ROOT / "src").rglob("*.rs")) + [ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "examples/stress.rs"])},
              "notes": "Sequential fresh four-worker servers; 4 KiB default read buffers; heartbeats enabled, rates disabled for stress, 1 MiB message limit and output budget of 1 MiB + 10 bytes. Server-only RSS/CPU and HTTP metrics sampled every 250ms. Loopback, no TLS. Latency quantiles are fixed-histogram bucket upper bounds. Not a long-term or production capacity guarantee.",
              "cases": []}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for mode in args.modes:
        seconds = {"traffic": args.traffic_secs, "churn": args.churn_secs, "slow": args.slow_secs}[mode]
        output["cases"].append(run_case(mode, seconds, args.clients, server_binary=args.server,
                                       stress_binary=args.stress, server_cpus=args.server_cpus,
                                       client_cpus=args.client_cpus, payload_bytes=args.payload_bytes))
        args.output.write_text(json.dumps(output, indent=2) + "\n")
    if not all(case["passed"] for case in output["cases"]):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
