#!/usr/bin/env python3
"""Reproducible WebTransport freshness/bulk test; impairment is restricted to a named netns.

Only the relay's UDP port is impaired; management traffic remains outside netem.
No host-wide settings are changed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.request
import benchmark as runtime
import benchmark_transport

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def stats(port):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/metrics", timeout=2) as response:
        return {name: int(value) for name, value in (row.split() for row in response.read().decode().splitlines() if row.startswith("war_"))}


def netem(args, port):
    if not args.isolated_namespace:
        if args.delay_ms or args.loss_percent or args.reorder_percent:
            raise ValueError("impairment requires --isolated-namespace")
        return False
    identity = subprocess.check_output(["ip", "netns", "identify"], text=True).strip()
    if identity != args.isolated_namespace:
        raise ValueError(f"current namespace {identity!r} differs from the requested isolated namespace")
    def tc(*command):
        subprocess.run(["sudo", "-n", "tc", *map(str, command)], check=True, stdout=subprocess.DEVNULL)
    tc("qdisc", "replace", "dev", "lo", "root", "handle", "1:", "prio", "bands", "3", "priomap", *(["1"] * 16))
    try:
        command = ["qdisc", "add", "dev", "lo", "parent", "1:1", "handle", "10:", "netem", "delay", f"{args.delay_ms}ms"]
        if args.jitter_ms:
            command.append(f"{args.jitter_ms}ms")
        command += ["loss", f"{args.loss_percent}%"]
        if args.reorder_percent:
            command += ["reorder", f"{args.reorder_percent}%", "50%"]
        command += ["seed", str(args.seed)]
        tc(*command)
        for field in ("src_port", "dst_port"):
            tc("filter", "add", "dev", "lo", "protocol", "ip", "parent", "1:", "prio", "1", "flower", "ip_proto", "udp", field, port, "flowid", "1:1")
    except BaseException:
        # A failed filter setup must also remove the partially installed qdisc.
        subprocess.run(["sudo", "-n", "tc", "qdisc", "del", "dev", "lo", "root"], check=False)
        raise
    return True


def run(args):
    if args.output.exists():
        raise FileExistsError(args.output)
    port, admin = runtime.free_port(), runtime.free_port()
    environment = {key: value for key, value in os.environ.items() if not key.startswith("WAR_")}
    server_command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}", "--workers", "4", "--messages-per-second", "0", "--bytes-per-second", "0", "--ping-interval-secs", "0", "--write-timeout-secs", "30"]
    url = benchmark_transport.prepare(args, server_command, environment, port)
    server_command = ["taskset", "-c", args.server_cpus] + server_command
    result = {"args": {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()}, "server_command": server_command, "server_sha256": digest(args.server), "client_sha256": digest(args.client), "runner_sha256": digest(__file__), "client_source_sha256": digest(ROOT / "examples/state_freshness.rs"), "passed": False, "scope": "Independently checked state generation stamps/payloads and reliable bulk. Freshness probes include the pre-first-update period using initial state at t0. Missing datagrams means not received in the observation window, not an attributed network-loss count. All session/resource gauges must retire."}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile() as logs:
        server = None
        impaired = False
        try:
            server = subprocess.Popen(server_command, stdout=logs, stderr=logs, env=environment)
            deadline = time.monotonic() + 10
            while True:
                if server.poll() is not None:
                    raise RuntimeError("relay exited before readiness")
                try:
                    result["before"] = stats(admin)
                    break
                except OSError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(.02)
            command = [str(args.client.resolve()), "--url", url, "--mode", args.mode, "--seconds", str(args.seconds), "--rate", str(args.rate), "--payload-bytes", str(args.payload_bytes), "--bulk-bytes", str(args.bulk_bytes)]
            if args.client_packet_threshold is not None:
                command += ["--packet-threshold", str(args.client_packet_threshold)]
            command += benchmark_transport.certificate(logs)
            command = ["taskset", "-c", args.client_cpus] + command
            result["client_command"] = command
            impaired = netem(args, port)
            result["server_before"] = runtime.snapshot(server.pid)
            started = time.monotonic()
            client = subprocess.run(command, capture_output=True, text=True, timeout=args.seconds + 90, env=environment)
            result.update(client_exit_code=client.returncode, client_stdout=client.stdout, client_stderr=client.stderr, wall_seconds=time.monotonic()-started, server_after=runtime.snapshot(server.pid))
            if client.returncode:
                raise RuntimeError("freshness client failed")
            result["measurement"] = json.loads(client.stdout)
            if impaired:
                result["netem_statistics"] = json.loads(subprocess.check_output(["tc", "-s", "-j", "qdisc", "show", "dev", "lo"], text=True))
            deadline = time.monotonic() + 10
            while True:
                observed = stats(admin)
                if all(observed[key] == 0 for key in ("war_sessions", "war_connections", "war_queued_bytes", "war_read_buffer_reserved_bytes", "war_variable_bytes", "war_quic_connections", "war_output_queue_capacity_bytes")):
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError("session/resource cleanup did not complete")
                time.sleep(.02)
            result["after"] = observed
            offered = result["measurement"]["offered"]
            expected_input = 1 + (observed["war_datagrams_received"] if args.mode == "datagram" else offered)
            assert observed["war_incoming_messages_total"] == expected_input
            if args.mode == "datagram":
                assert observed["war_datagrams_submitted"] == observed["war_datagrams_received"]
                assert result["measurement"]["received_unique"] <= observed["war_datagrams_submitted"] <= offered
            result["passed"] = True
        except Exception as error:
            result["error"] = repr(error)
        finally:
            if impaired:
                cleanup = subprocess.run(["sudo", "-n", "tc", "qdisc", "del", "dev", "lo", "root"], check=False)
                result["netem_cleanup_exit_code"] = cleanup.returncode
                if cleanup.returncode:
                    result["passed"] = False
                    result.setdefault("error", "netem cleanup failed")
            if server is not None:
                server.terminate()
                try:
                    server.wait(timeout=12)
                except subprocess.TimeoutExpired:
                    server.kill(); server.wait()
                    result["passed"] = False
                    result.setdefault("error", "forced process cleanup")
                result["server_exit_code"] = server.returncode
            if server is not None and server.returncode != 0:
                result["passed"] = False
                result.setdefault("error", "relay did not exit cleanly")
            logs.seek(0)
            result["server_log"] = logs.read().decode(errors="replace")
            args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": result["passed"], "error": result.get("error"), "measurement": result.get("measurement")}))
    return result["passed"]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/war-relay")
    parser.add_argument("--client", type=Path, default=ROOT / "target/release/examples/state_freshness")
    parser.add_argument("--mode", choices=["stream", "datagram"], required=True)
    parser.add_argument("--seconds", type=int, default=10)
    parser.add_argument("--rate", type=int, default=60)
    parser.add_argument("--payload-bytes", type=int, default=1024)
    parser.add_argument("--bulk-bytes", type=int, default=1024*1024)
    parser.add_argument("--client-packet-threshold", type=int, help="QUIC packet-count loss threshold for the client's own sending; 3 is the stock value, the client default matches the relay")
    parser.add_argument("--server-cpus", default="0-3")
    parser.add_argument("--client-cpus", default="8-11")
    parser.add_argument("--isolated-namespace")
    parser.add_argument("--delay-ms", type=float, default=0)
    parser.add_argument("--jitter-ms", type=float, default=0)
    parser.add_argument("--loss-percent", type=float, default=0)
    parser.add_argument("--reorder-percent", type=float, default=0)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--output", type=Path, required=True)
    raise SystemExit(0 if run(parser.parse_args()) else 1)
