#!/usr/bin/env python3
"""Compare read-buffer sizes with identical real-socket workloads in fresh servers.

Run from any directory after cargo build --release --bins --example load.
Results contain server-only RSS/CPU plus load-generator validation and latency.
Candidates run sequentially; order reverses on alternate repetitions.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import tempfile
import time
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
SERVER = ROOT / "target/release/war-relay"
LOAD = ROOT / "target/release/examples/load"


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


def snapshot(pid):
    values = {}
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith(("VmRSS:", "VmSize:", "RssAnon:", "Threads:")):
            key, value = line.split(":", 1)
            values[key] = int(value.split()[0])
    fields = Path(f"/proc/{pid}/stat").read_text().split()
    values["cpu_seconds"] = (int(fields[13]) + int(fields[14])) / os.sysconf("SC_CLK_TCK")
    # Quiescent-gate diagnostics expose flow/worker placement without sampling
    # inside a timed request. Threads may retire between directory and stat.
    threads = {}
    for task in Path(f"/proc/{pid}/task").iterdir():
        try:
            fields = (task / "stat").read_text().rsplit(")", 1)[1].split()
            threads[task.name] = (int(fields[11]) + int(fields[12])) / os.sysconf("SC_CLK_TCK")
        except FileNotFoundError:
            continue
    values["thread_cpu_seconds"] = threads
    return values


def metrics(port):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/metrics", timeout=1) as response:
        return dict(line.split() for line in response.read().decode().splitlines() if line.startswith("war_"))


def trial(buffer, workload, clients, nagle=False):
    port, admin = free_port(), free_port()
    env = {key: value for key, value in os.environ.items() if not key.startswith("WAR_")}
    with tempfile.TemporaryFile() as logs:
        server = subprocess.Popen([
            str(SERVER), "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}",
            "--workers", "4", "--messages-per-second", "0", "--bytes-per-second", "0",
            "--ping-interval-secs", "0", "--read-buffer-bytes", str(buffer),
        ], stdout=logs, stderr=logs, env=env)
        try:
            for _ in range(200):
                if server.poll() is not None:
                    logs.seek(0)
                    raise RuntimeError(logs.read().decode())
                try:
                    metrics(admin)
                    break
                except OSError:
                    time.sleep(0.02)
            else:
                raise RuntimeError("server readiness timeout")
            baseline = snapshot(server.pid)
            command = [str(LOAD), "--url", f"ws://127.0.0.1:{port}"]
            if nagle:
                command.append("--nagle")
            if workload == "idle":
                process = subprocess.Popen(command + ["--mode", "idle", "--clients", str(clients), "--hold-secs", "2"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                try:
                    line = process.stdout.readline()
                    if not line:
                        raise RuntimeError(process.stderr.read())
                    result = json.loads(line)
                    observed = metrics(admin)
                    assert int(observed["war_sessions"]) == clients, observed
                    measured = snapshot(server.pid)
                    process.communicate(timeout=10)
                    assert process.returncode == 0
                finally:
                    if process.poll() is None:
                        process.kill()
                        process.communicate()
            else:
                args = {
                    "private-256": ["--mode", "private", "--clients", "128", "--messages", "1000", "--payload-bytes", "256"],
                    "private-65536": ["--mode", "private", "--clients", "32", "--messages", "200", "--payload-bytes", "65536"],
                    "broadcast-256": ["--mode", "broadcast", "--clients", "64", "--messages", "500", "--payload-bytes", "256"],
                }[workload]
                completed = subprocess.run(command + args, check=True, capture_output=True, text=True, timeout=60)
                result = json.loads(completed.stdout)
                measured = snapshot(server.pid)
                observed = metrics(admin)
            return {"read_buffer_bytes": buffer, "workload": workload, "baseline": baseline, "measured": measured,
                    "server_cpu_seconds": measured["cpu_seconds"] - baseline["cpu_seconds"],
                    "server_rss_delta_kib": measured["VmRSS"] - baseline["VmRSS"], "metrics": observed, "result": result}
        finally:
            server.terminate()
            try:
                server.wait(timeout=12)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--idle-clients", type=int, default=2000)
    parser.add_argument("--buffers", type=int, nargs="+", default=[131072, 4096])
    parser.add_argument("--nagle", action="store_true")
    parser.add_argument("--workloads", nargs="+", choices=["idle", "private-256", "private-65536", "broadcast-256"], default=["idle", "private-256", "private-65536", "broadcast-256"])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    samples = []
    for repetition in range(args.repetitions):
        order = args.buffers if repetition % 2 == 0 else list(reversed(args.buffers))
        for workload in args.workloads:
            for buffer in order:
                sample = trial(buffer, workload, args.idle_clients, args.nagle)
                samples.append(sample)
                print(json.dumps({"repetition": repetition, "workload": workload, "buffer": buffer, "rss_kib": sample["measured"]["VmRSS"], "deliveries_per_second": sample["result"].get("deliveries_per_second")}), flush=True)
    output = {"host": platform.platform(), "cpu": subprocess.check_output(["lscpu"], text=True),
              "rustc": subprocess.check_output(["rustc", "--version"], text=True).strip(),
              "server_sha256": hashlib.sha256(SERVER.read_bytes()).hexdigest(), "server_workers": 4,
              "load_sha256": hashlib.sha256(LOAD.read_bytes()).hexdigest(),
              "client_tcp_nodelay": not args.nagle,
              "notes": "Loopback, no TLS, rates/heartbeat disabled only for benchmark. Fresh process per sample. RSS KiB excludes load generator and kernel socket memory. CPU time includes setup and teardown, elapsed/latency excludes setup. No affinity pinning.",
              "samples": samples}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(output, indent=2) + "\n")


if __name__ == "__main__":
    main()
