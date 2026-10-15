#!/usr/bin/env python3
"""Sample real server RSS/queue ownership beside unknown-target slab primers.

Build first: cargo build --release --bins --example retention
Runs fresh servers sequentially. The client checks every delivery and Close.
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


def run_case(server_binary, client_binary, cycles, primer_bytes):
    port, admin = runtime.free_port(), runtime.free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith("WAR_")}
    server_command = [str(server_binary), "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}",
                      "--workers", "4", "--messages-per-second", "0", "--bytes-per-second", "0",
                      "--ping-interval-secs", "0", "--write-timeout-secs", "60", "--queue-messages", "1024"]
    command = [str(client_binary), "--address", f"127.0.0.1:{port}", "--cycles", str(cycles), "--primer-bytes", str(primer_bytes)]
    client = None
    with tempfile.TemporaryFile() as logs:
        server = subprocess.Popen(server_command, stdout=logs, stderr=logs, env=env)
        try:
            for _ in range(200):
                if server.poll() is not None:
                    logs.seek(0)
                    raise RuntimeError(logs.read().decode())
                try:
                    runtime.metrics(admin)
                    break
                except OSError:
                    time.sleep(0.02)
            else:
                raise TimeoutError("server readiness deadline")
            started = time.monotonic()
            baseline = runtime.snapshot(server.pid)
            client = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
            samples, events, pending, phase = [], [], b"", "startup"
            with selectors.DefaultSelector() as selector:
                selector.register(client.stdout, selectors.EVENT_READ)
                while client.poll() is None:
                    if time.monotonic() - started > 50:
                        raise TimeoutError("retention client exceeded outer deadline")
                    if selector.select(timeout=0.05):
                        pending += os.read(client.stdout.fileno(), 65536)
                        while b"\n" in pending:
                            line, pending = pending.split(b"\n", 1)
                            entry = json.loads(line)
                            events.append(entry)
                            phase = entry["event"]
                    samples.append({"seconds":time.monotonic()-started, "phase":phase,
                                    "process":runtime.snapshot(server.pid), "metrics":runtime.metrics(admin)})
            stdout, stderr = client.communicate(timeout=2)
            events.extend(json.loads(line) for line in (pending + stdout).splitlines() if line)
            for _ in range(200):
                final = runtime.metrics(admin)
                clean = all(int(final[k]) == 0 for k in ["war_sessions","war_connections","war_queued_bytes","war_variable_bytes"]) and int(final["war_channels"]) == 1
                if clean:
                    break
                time.sleep(0.02)
            result = next((entry for entry in events if entry["event"] == "result"), None)
            counters_match = result is not None and all([
                int(final["war_incoming_messages_total"]) == result["incoming_messages"],
                int(final["war_deliveries_total"]) == result["verified_deliveries"] + 2,  # Welcome frames.
                int(final["war_routing_reused_bytes_total"]) + int(final["war_routing_copied_bytes_total"]) == result["routed_value_bytes"],
                int(final["war_joined_total"]) == 2,
            ])
            held = [s for s in samples if s["phase"] == "holding"]
            pressure = bool(held) and min(int(s["metrics"]["war_queued_bytes"]) for s in held) > 1024 * 1024
            error_free = all(int(final[k]) == 0 for k in ["war_protocol_errors_total","war_transport_disconnects_total","war_write_errors_total","war_write_timeouts_total","war_close_failures_total","war_slow_consumers_total","war_global_pressure_total"])
            record = {"cycles":cycles,"primer_bytes":primer_bytes,"server_command":server_command,"client_command":command,
                      "baseline":baseline,"samples":samples,"events":events,"exit_code":client.returncode,"stderr":stderr.decode(),
                      "final_metrics":final,"final_process":runtime.snapshot(server.pid),"cleaned_up":clean,
                      "counters_match":counters_match,"backpressure_observed":pressure,"error_free":error_free,
                      "peak_server_rss_kib":max([baseline["VmRSS"]]+[s["process"]["VmRSS"] for s in samples]),
                      "passed":client.returncode == 0 and clean and counters_match and pressure and error_free}
            print(json.dumps({k:record[k] for k in ["cycles","primer_bytes","peak_server_rss_kib","passed"]}), flush=True)
            return record
        finally:
            if client is not None and client.poll() is None:
                client.kill()
                client.communicate()
            server.terminate()
            try:
                server.wait(timeout=12)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cycles", type=int, nargs="+", default=[64,256])
    parser.add_argument("--primer-bytes", type=int, default=4*1024*1024)
    parser.add_argument("--server", type=Path, default=runtime.SERVER)
    parser.add_argument("--client", type=Path, default=ROOT/"target/release/examples/retention")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = {"host":platform.platform(),"revision":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
              "server_sha256":hashlib.sha256(args.server.read_bytes()).hexdigest(),"client_sha256":hashlib.sha256(args.client.read_bytes()).hexdigest(),
              "source_sha256":{str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(list((ROOT/"src").rglob("*.rs"))+[ROOT/"examples/retention.rs",ROOT/"scripts/retention.py",ROOT/"Cargo.lock"])},
              "notes":"Sequential fresh four-worker Linux servers; default 4 KiB reads, 32 MiB queue bytes, 1024 queue messages, 60s write deadline, heartbeats and rates disabled. A 64 KiB requested client receive buffer and eight 1 MiB fillers establish unread transport backpressure. Large unknown-target primers alternate with tiny queued values and verified self barriers. Server-only RSS/CPU and queue metrics sampled about every 50ms. No compaction-policy change or leak/production-capacity claim.","cases":[]}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    for cycles in args.cycles:
        output["cases"].append(run_case(args.server,args.client,cycles,args.primer_bytes))
        args.output.write_text(json.dumps(output,indent=2)+"\n")
    if not all(case["passed"] for case in output["cases"]):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
