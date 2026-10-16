#!/usr/bin/env python3
"""Measure verified global routing before/after a transient population in separate rooms.

Build the registry_peak example first. Server/client phases are explicitly gated;
native CPU snapshots exclude filling/draining the population from routing cost.
The optional perf profile is diagnostic and must not be used as a timing sample.
"""
import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import select
import signal
import subprocess
import tempfile
import time

import benchmark as runtime

ROOT = runtime.ROOT


def run(args):
    port, admin = runtime.free_port(), runtime.free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith("WAR_")}
    server_command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}",
                      "--admin-bind", f"127.0.0.1:{admin}", "--workers", "4",
                      "--messages-per-second", "0", "--bytes-per-second", "0",
                      "--ping-interval-secs", "0"]
    command = [str(args.client.resolve()), "--url", f"ws://127.0.0.1:{port}",
               "--peak-clients", str(args.peak_clients), "--waves", str(args.waves),
               "--seconds", str(args.seconds), "--payload-bytes", str(args.payload_bytes)]
    command += ["--scope", args.scope]
    if args.server_cpus:
        server_command = ["taskset", "-c", args.server_cpus] + server_command
    if args.client_cpus:
        command = ["taskset", "-c", args.client_cpus] + command
    digest = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()
    result = {"timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "host": platform.platform(), "server_command": server_command, "command": command,
              "server_sha256": digest(args.server), "client_sha256": digest(args.client),
              "client_source_sha256": digest(ROOT / "examples/registry_peak.rs"),
              "runner_sha256": digest(__file__), "profile_diagnostic": args.profile,
              "args": {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()},
              "scope": "Two retained clients, 64 prebuilt cyclic values, 16 message windows; every encoded sender/value/order checked. Global: broadcast with transient peers in separate rooms. Room: ordinary room broadcast, transient peers in the same room. Private: private delivery with all peers in the global room, exercising its listener index with default zero listener flags. Both shared-room modes verify every departing ID and exact FIFO prefix at each remaining peer, pacing 16 departures per window by reader progress so default queue limits remain intact. All peers verify exact normal Close and EOF. Gated event-boundary metrics/CPU/RSS snapshots, no continuous RSS peak claim. Loopback, no TLS; heartbeat and rates disabled. CPU deltas include one management sample at each boundary, at host tick resolution. Profiled cases are diagnostic, not timing samples.",
              "events": [], "traffic": [], "passed": False}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile() as logs, tempfile.TemporaryFile() as client_errors:
        server = subprocess.Popen(server_command, stdout=logs, stderr=logs, env=env)
        client = perf = None
        perf_log = None
        try:
            started = time.monotonic()
            while True:
                if server.poll() is not None:
                    raise RuntimeError("server stopped during readiness")
                try:
                    runtime.metrics(admin)
                    break
                except OSError:
                    if time.monotonic() - started > 5:
                        raise
                    time.sleep(.02)
            client = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=client_errors, text=True, env=env)
            ready = {}
            limit = (args.waves + 1) * args.seconds + args.peak_clients * args.waves * .02 + 40
            while True:
                if time.monotonic() - started > limit:
                    raise TimeoutError("registry workload exceeded its outer deadline")
                if not select.select([client.stdout], [], [], .25)[0]:
                    continue
                line = client.stdout.readline()
                if not line:
                    raise RuntimeError("client stopped before its final gated event")
                event = json.loads(line)
                event.update(metrics={k: int(float(v)) for k, v in runtime.metrics(admin).items()},
                             process=runtime.snapshot(server.pid), seconds=time.monotonic() - started)
                result["events"].append(event)
                metrics, wave = event["metrics"], event["wave"]
                if event["event"] == "traffic_ready":
                    assert metrics["war_connections"] == metrics["war_sessions"] == 2
                    assert metrics["war_channels"] == (2 if args.scope == "room" else 1)
                    ready[wave] = event
                    if args.profile and wave == 1:
                        profile = args.output.with_suffix(".perf.data")
                        perf_log = tempfile.TemporaryFile()
                        perf = subprocess.Popen(["perf", "record", "-e", "cycles:u", "-F", "499",
                                                 "--call-graph", "dwarf,8192", "-p", str(server.pid),
                                                 "-o", str(profile)], stdout=perf_log, stderr=perf_log)
                        time.sleep(.2)
                        if perf.poll() is not None:
                            raise RuntimeError("perf could not attach")
                        result["profile_path"] = str(profile)
                elif event["event"] == "traffic_done":
                    if perf is not None:
                        perf.send_signal(signal.SIGINT)
                        result["perf_exit_code"] = perf.wait(timeout=10)
                        perf_log.seek(0)
                        result["perf_log"] = perf_log.read().decode(errors="replace")
                        perf_log.close()
                        perf = perf_log = None
                    before, output = ready[wave], event["details"]
                    cpu = event["process"]["cpu_seconds"] - before["process"]["cpu_seconds"]
                    expected = {"war_incoming_messages_total": output["sent"],
                                "war_incoming_bytes_total": output["sent_encoded_bytes"],
                                "war_deliveries_total": output["verified"],
                                "war_delivered_bytes_total": output["delivered_encoded_bytes"],
                                "war_routing_reused_bytes_total": output["value_bytes"],
                                "war_routing_copied_bytes_total": 0}
                    for key, value in expected.items():
                        assert metrics[key] - before["metrics"][key] == value, key
                    row = {"wave": wave, **output, "server_cpu_seconds": cpu,
                           "server_ns_per_delivery": cpu * 1e9 / output["verified"]}
                    result["traffic"].append(row)
                    print(json.dumps(row), flush=True)
                elif event["event"] == "population_ready":
                    assert metrics["war_connections"] == metrics["war_sessions"] == args.peak_clients + 2
                    assert metrics["war_channels"] == {"room": 2, "private": 1, "global": args.peak_clients + 1}[args.scope]
                elif event["event"] == "done":
                    for key in ["war_connections", "war_sessions", "war_queued_bytes", "war_variable_bytes", "war_read_buffer_reserved_bytes"]:
                        assert metrics[key] == 0, key
                    assert metrics["war_channels"] == 1
                    details = event["details"]
                    joins = args.peak_clients * args.waves + 2
                    assert details["normal_closes"] == details["joins"] == joins
                    assert metrics["war_joined_total"] == metrics["war_disconnected_total"] == metrics["war_accepted_total"] == joins
                    assert metrics["war_deliveries_total"] == joins + sum(t["verified"] for t in result["traffic"]) + details.get("departure_deliveries", 0) + 1
                    assert metrics["war_delivered_bytes_total"] == details["welcome_bytes"] + details["disconnect_bytes"] + details.get("departure_bytes", 0) + sum(t["delivered_encoded_bytes"] for t in result["traffic"])
                    for key in ["war_handshake_errors_total", "war_origin_rejected_total", "war_join_rejected_total", "war_rejected_total", "war_protocol_errors_total", "war_rate_limited_total", "war_write_timeouts_total", "war_write_errors_total", "war_close_failures_total", "war_aborted_writes_total", "war_transport_disconnects_total", "war_slow_consumers_total", "war_global_pressure_total", "war_read_buffer_reservations_rejected_total"]:
                        assert metrics.get(key, 0) == 0, key
                client.stdin.write("continue\n")
                client.stdin.flush()
                if event["event"] == "done":
                    assert client.wait(timeout=5) == 0
                    result["passed"] = True
                    break
        except Exception as error:
            result["error"] = repr(error)
        finally:
            if perf is not None:
                perf.send_signal(signal.SIGINT)
                perf.wait(timeout=10)
            if perf_log is not None:
                perf_log.seek(0)
                result["perf_log"] = perf_log.read().decode(errors="replace")
                perf_log.close()
            if client is not None:
                if client.poll() is None:
                    client.kill()
                    client.wait()
                result["client_exit_code"] = client.returncode
                client.stdin.close()
                client.stdout.close()
            server.terminate()
            try:
                result["server_exit_code"] = server.wait(timeout=12)
            except subprocess.TimeoutExpired:
                server.kill()
                result["server_exit_code"] = server.wait()
            logs.seek(0)
            client_errors.seek(0)
            result.update(server_log=logs.read().decode(errors="replace"), client_log=client_errors.read().decode(errors="replace"))
            if result["server_exit_code"] != 0:
                result["passed"] = False
                result.setdefault("error", f"server exited with status {result['server_exit_code']}")
            args.output.write_text(json.dumps(result, indent=2) + "\n")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=runtime.SERVER)
    parser.add_argument("--client", type=Path, default=ROOT / "target/release/examples/registry_peak")
    parser.add_argument("--peak-clients", type=int, default=8192)
    parser.add_argument("--waves", type=int, default=1)
    parser.add_argument("--seconds", type=int, default=3)
    parser.add_argument("--payload-bytes", type=int, default=1024)
    parser.add_argument("--server-cpus", default="0-3")
    parser.add_argument("--client-cpus", default="4-7")
    parser.add_argument("--profile", action="store_true")
    parser.add_argument("--scope", choices=["global", "room", "private"], default="global")
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    outcome = run(arguments)
    print(json.dumps({"passed": outcome["passed"], "error": outcome.get("error")}), flush=True)
    raise SystemExit(0 if outcome["passed"] else 1)
