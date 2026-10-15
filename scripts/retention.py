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
SAMPLED_METRICS = ("war_connections", "war_sessions", "war_channels", "war_queued_bytes", "war_variable_bytes",
                   "war_incoming_messages_total", "war_deliveries_total", "war_routing_reused_bytes_total", "war_routing_copied_bytes_total")


def run_case(server_binary, client_binary, cycles, primer_bytes, producers, idle_secs, server_idle_secs):
    port, admin = runtime.free_port(), runtime.free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith("WAR_")}
    server_command = [str(server_binary), "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}",
                      "--workers", "4", "--messages-per-second", "0", "--bytes-per-second", "0",
                      "--ping-interval-secs", "0", "--write-timeout-secs", "60",
                      "--queue-messages", str(max(1024, producers * cycles + 32))]
    command = [str(client_binary), "--address", f"127.0.0.1:{port}", "--cycles", str(cycles),
               "--primer-bytes", str(primer_bytes), "--producers", str(producers), "--idle-secs", str(idle_secs)]
    if server_idle_secs:
        server_command.extend(["--idle-timeout-secs", str(server_idle_secs)])
        command.append("--expect-idle-close")
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
                    observed = runtime.metrics(admin)
                    samples.append({"seconds":time.monotonic()-started, "phase":phase,
                                    "process":runtime.snapshot(server.pid), "metrics":{k:observed[k] for k in SAMPLED_METRICS}})
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
                int(final["war_deliveries_total"]) == result["verified_deliveries"] + producers + 1,  # Welcome frames.
                int(final["war_routing_reused_bytes_total"]) + int(final["war_routing_copied_bytes_total"]) == result["routed_value_bytes"],
                int(final["war_joined_total"]) == producers + 1,
                result["producers"] == producers,
                result["clean_close_handshakes"] == producers + 1,
                result["server_idle_close_verified"] == bool(server_idle_secs),
            ])
            held = [s for s in samples if s["phase"] == "holding"]
            pressure = bool(held) and min(int(s["metrics"]["war_queued_bytes"]) for s in held) > 1024 * 1024
            idle = [s for s in samples if s["phase"] == "idle"]
            idle_clean = idle_secs == 0 or (bool(idle) and all(int(idle[-1]["metrics"][k]) == 0 for k in ["war_queued_bytes","war_variable_bytes"])
                                            and (server_idle_secs > 0 or int(idle[-1]["metrics"]["war_sessions"]) == producers + 1)
                                            and int(idle[-1]["metrics"]["war_connections"]) == producers + 1)
            idle_counter = final.get("war_idle_timeouts_total")
            policy_matches = (not server_idle_secs and idle_counter in (None,"0")) or (server_idle_secs > 0 and idle_counter == str(producers + 1))
            retired = [s for s in samples if s["phase"] == "retired"]
            retired_clean = idle_secs == 0 or (bool(retired) and all(int(retired[-1]["metrics"][k]) == 0 for k in ["war_sessions","war_connections","war_queued_bytes","war_variable_bytes"]))
            error_free = all(int(final[k]) == 0 for k in ["war_protocol_errors_total","war_transport_disconnects_total","war_write_errors_total","war_write_timeouts_total","war_close_failures_total","war_slow_consumers_total","war_global_pressure_total","war_aborted_writes_total","war_rate_limited_total"])
            record = {"cycles":cycles,"primer_bytes":primer_bytes,"producers":producers,"idle_secs":idle_secs,"server_idle_secs":server_idle_secs,"server_command":server_command,"client_command":command,
                      "baseline":baseline,"samples":samples,"events":events,"exit_code":client.returncode,"stderr":stderr.decode(),
                      "final_metrics":final,"final_process":runtime.snapshot(server.pid),"cleaned_up":clean,
                      "counters_match":counters_match,"backpressure_observed":pressure,"error_free":error_free,
                      "idle_with_empty_queues":idle_clean,
                      "idle_policy_matches":policy_matches,"retired_clean":retired_clean,
                      "idle_server_rss_range_kib":[min(s["process"]["VmRSS"] for s in idle),max(s["process"]["VmRSS"] for s in idle)] if idle else None,
                      "retired_server_rss_range_kib":[min(s["process"]["VmRSS"] for s in retired),max(s["process"]["VmRSS"] for s in retired)] if retired else None,
                      "peak_server_rss_kib":max([baseline["VmRSS"]]+[s["process"]["VmRSS"] for s in samples]),
                      "passed":client.returncode == 0 and clean and counters_match and pressure and idle_clean and retired_clean and policy_matches and error_free}
            print(json.dumps({k:record[k] for k in ["cycles","primer_bytes","producers","server_idle_secs","peak_server_rss_kib","idle_server_rss_range_kib","retired_server_rss_range_kib","passed"]}), flush=True)
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
    parser.add_argument("--producers", type=int, nargs="+", default=[1])
    parser.add_argument("--idle-secs", type=int, default=0)
    parser.add_argument("--server-idle-secs", type=int, default=0, help="Require automatic application-idle closes after 1..10 seconds; zero disables.")
    parser.add_argument("--server", type=Path, default=runtime.SERVER)
    parser.add_argument("--client", type=Path, default=ROOT/"target/release/examples/retention")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not all(1 <= producers <= 64 for producers in args.producers) or not all(16 <= cycles <= 512 for cycles in args.cycles):
        parser.error("producers must be in 1..=64 and cycles in 16..=512")
    if not 0 <= args.idle_secs <= 10 or not 65536 <= args.primer_bytes <= 8 * 1024 * 1024:
        parser.error("idle seconds must be in 0..=10 and primer bytes in 64 KiB..=8 MiB")
    if not 0 <= args.server_idle_secs <= 10 or (args.server_idle_secs and not args.idle_secs):
        parser.error("server idle seconds must be in 0..=10, with a nonzero idle observation when enabled")
    if max(args.producers) * max(args.cycles) * args.primer_bytes > 8 * 1024**3:
        parser.error("each case must stay within 8 GiB of primer traffic; reduce producers, cycles, or primer bytes")
    output = {"host":platform.platform(),"revision":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
              "server_sha256":hashlib.sha256(args.server.read_bytes()).hexdigest(),"client_sha256":hashlib.sha256(args.client.read_bytes()).hexdigest(),
              "source_sha256":{str(p.relative_to(ROOT)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(list((ROOT/"src").rglob("*.rs"))+[ROOT/"examples/retention.rs",ROOT/"scripts/retention.py",ROOT/"Cargo.lock"])},
              "notes":"Sequential fresh four-worker Linux servers; default 4 KiB reads and 32 MiB queue bytes, max(1024, producers * cycles + 32) queue messages, 60s write deadline, heartbeats and rates disabled. A 64 KiB requested client receive buffer and eight 1 MiB fillers establish unread transport backpressure. Synchronized producers alternate large unknown-target primers with tiny queued values and verified self barriers. Each producer's full sequence is verified without imposing a cross-producer order. Optional connected-idle sampling follows complete drain; two seconds of retired sampling follow client-initiated or required server-idle Close handshakes. Server-only RSS/CPU and relevant queue/routing metrics sampled about every 50ms, all final counters retained. Each case caps primer traffic at 8 GiB; preparation is bounded independently of traffic. No compaction-policy change or leak/production-capacity claim.","cases":[]}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    for producers in args.producers:
        for cycles in args.cycles:
            output["cases"].append(run_case(args.server,args.client,cycles,args.primer_bytes,producers,args.idle_secs,args.server_idle_secs))
            args.output.write_text(json.dumps(output,indent=2)+"\n")
    if not all(case["passed"] for case in output["cases"]):
        raise SystemExit(1)


if __name__ == "__main__":
    main()
