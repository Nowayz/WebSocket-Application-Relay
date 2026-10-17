#!/usr/bin/env python3
"""Measure incomplete-frame staging while an independent healthy client routes."""
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


def run(args):
    port, admin = runtime.free_port(), runtime.free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith("WAR_")}
    server_command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}",
                      "--admin-bind", f"127.0.0.1:{admin}", "--workers", "4",
                      "--messages-per-second", "0", "--bytes-per-second", "0",
                      "--ping-interval-secs", "0", "--idle-timeout-secs", "0", *args.server_arg]
    client_command = [str(args.client.resolve()), "--address", f"127.0.0.1:{port}",
                      "--clients", str(args.clients), "--frame-bytes", str(args.frame_bytes),
                      "--partial-bytes", str(args.partial_bytes), "--hold-secs", str(args.hold_secs)]
    if args.expect_admitted is not None:
        client_command.extend(["--expect-admitted", str(args.expect_admitted)])
    if args.expect_admission_denial:
        client_command.append("--expect-admission-denial")
    client = None
    with tempfile.TemporaryFile() as log:
        server = subprocess.Popen(server_command, env=env, stdout=log, stderr=log)
        try:
            for _ in range(200):
                if server.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read().decode())
                try:
                    runtime.metrics(admin)
                    break
                except OSError:
                    time.sleep(.02)
            else:
                raise TimeoutError("server startup")
            baseline = runtime.snapshot(server.pid)
            client = subprocess.Popen(client_command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
            samples, events, pending, phase = [], [], b"", "startup"
            started = time.monotonic()
            with selectors.DefaultSelector() as selector:
                selector.register(client.stdout, selectors.EVENT_READ)

                def consume(wait=0):
                    nonlocal pending, phase
                    while selector.select(wait):
                        data = os.read(client.stdout.fileno(), 65536)
                        if not data:
                            selector.unregister(client.stdout)
                            break
                        pending += data
                        while b"\n" in pending:
                            line, pending = pending.split(b"\n", 1)
                            value = json.loads(line)
                            events.append(value)
                            phase = value["event"]
                        wait = 0

                while client.poll() is None:
                    if time.monotonic() - started > 40:
                        raise TimeoutError("input probe deadline")
                    consume(.05)
                    before = phase
                    observed, process = runtime.metrics(admin), runtime.snapshot(server.pid)
                    consume()
                    samples.append({"seconds":time.monotonic()-started,
                                    "phase":phase if phase == before else "transition",
                                    "phase_before":before,"phase_after":phase,
                                    "metrics":observed,"process":process})
            stdout, stderr = client.communicate(timeout=2)
            events.extend(json.loads(line) for line in (pending+stdout).splitlines() if line)
            for _ in range(200):
                final = runtime.metrics(admin)
                clean = all(int(final[k]) == 0 for k in ("war_connections","war_sessions","war_queued_bytes","war_variable_bytes")) and int(final["war_channels"]) == 1 and int(final.get("war_read_buffer_reserved_bytes", 0)) == 0
                if clean: break
                time.sleep(.02)
            result = next((e for e in events if e["event"] == "result"), None)
            held = [s for s in samples if s["phase"] == "holding"]
            healthy = bool(held) and result is not None and result["healthy_verified"] > 0
            counts = result is not None and int(final["war_transport_disconnects_total"]) == result["expected_incomplete_tcp_drops"]
            errors = all(int(final[k]) == 0 for k in ("war_protocol_errors_total","war_write_errors_total","war_write_timeouts_total","war_close_failures_total","war_slow_consumers_total","war_global_pressure_total"))
            limit = int(final.get("war_read_buffer_limit_bytes", 0))
            bounded = limit == 0 or all(int(s["metrics"]["war_read_buffer_reserved_bytes"]) <= limit for s in samples) and int(final["war_read_buffer_peak_reserved_bytes"]) <= limit
            denials = result is not None and (not limit or int(final["war_read_buffer_reservations_rejected_total"]) >= len(result["rejected_codes"]) + int(result["new_connection_denial_verified"]))
            record = {"server_command":server_command,"client_command":client_command,
                      "server_sha256":hashlib.sha256(args.server.read_bytes()).hexdigest(),
                      "client_sha256":hashlib.sha256(args.client.read_bytes()).hexdigest(),
                      "baseline":baseline,"samples":samples,"events":events,"final_metrics":final,
                      "stderr":stderr.decode(),"exit_code":client.returncode,"cleaned_up":clean,
                      "healthy_verified":healthy,"transport_counts_match":counts,"error_free":errors,
                      "reservation_bound_verified":bounded,"denial_counts_match":denials,
                      "holding_rss_range_kib":[min(s["process"]["VmRSS"] for s in held),max(s["process"]["VmRSS"] for s in held)] if held else None,
                      "peak_sampled_rss_kib":max([baseline["VmRSS"]]+[s["process"]["VmRSS"] for s in samples]),
                      "passed":client.returncode == 0 and clean and healthy and counts and errors and bounded and denials}
            return record
        finally:
            if client is not None and client.poll() is None:
                client.kill(); client.communicate()
            server.terminate()
            try: server.wait(timeout=12)
            except subprocess.TimeoutExpired: server.kill(); server.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT/"target/release/war-relay")
    parser.add_argument("--client", type=Path, default=ROOT/"target/release/examples/input_budget")
    parser.add_argument("--server-arg", action="append", default=[])
    parser.add_argument("--clients", type=int, default=64)
    parser.add_argument("--frame-bytes", type=int, default=8*1024*1024)
    parser.add_argument("--partial-bytes", type=int, default=2*1024*1024)
    parser.add_argument("--hold-secs", type=int, default=2)
    parser.add_argument("--expect-admitted", type=int)
    parser.add_argument("--expect-admission-denial", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not (1 <= args.clients <= 64 and 65536 <= args.frame_bytes <= 8*1024*1024
            and 1024 <= args.partial_bytes <= 2*1024*1024 and args.partial_bytes < args.frame_bytes
            and 1 <= args.hold_secs <= 10 and (args.expect_admitted is None or 0 <= args.expect_admitted <= args.clients)):
        parser.error("use 1..64 clients, 64 KiB..8 MiB frames, 1 KiB..2 MiB partial bodies smaller than frames, 1..10 seconds, and admitted count within clients")
    record = {"host":platform.platform(),"revision":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
              "notes":"Fresh four-worker plaintext loopback server; heartbeats, input rates and application-idle Close disabled. All sources join before advertising incomplete frames, then send only a bounded partial body. Independent 1 KiB self echoes remain verified during the hold. Admitted incomplete sockets intentionally drop TCP; two complete sessions verify normal Close. Phase markers bracket sampling; RSS excludes the client. This is a decoder-staging experiment, not a total-process memory cap or graceful acceptance of partial-frame departures.",
              "case":run(args)}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(record,indent=2)+"\n")
    print(json.dumps({k:record["case"][k] for k in ("passed","holding_rss_range_kib","peak_sampled_rss_kib","healthy_verified","transport_counts_match")}))
    if not record["case"]["passed"]: raise SystemExit(1)


if __name__ == "__main__":
    main()
