#!/usr/bin/env python3
"""Reproducible freshness/bulk test; impairment is restricted to a named netns.

Only the external relay port is impaired. A TLS front end's internal TCP hop and
management traffic remain outside netem. No host-wide settings are changed.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import socket
import subprocess
import tempfile
import time
import urllib.request
import benchmark as runtime

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def stats(port):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/metrics", timeout=2) as response:
        return {name: int(value) for name, value in (row.split() for row in response.read().decode().splitlines() if row.startswith("war_"))}


def netem(args, external_port, protocol):
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
            tc("filter", "add", "dev", "lo", "protocol", "ip", "parent", "1:", "prio", "1", "flower", "ip_proto", protocol, field, external_port, "flowid", "1:1")
    except BaseException:
        # A failed filter setup must also remove the partially installed qdisc.
        subprocess.run(["sudo", "-n", "tc", "qdisc", "del", "dev", "lo", "root"], check=False)
        raise
    return True


def run(args):
    if args.output.exists():
        raise FileExistsError(args.output)
    port, admin, front_port = runtime.free_port(), runtime.free_port(), runtime.free_port()
    environment = {key: value for key, value in os.environ.items() if not key.startswith("WAR_")}
    environment["RUST_LOG"] = "war_relay=info"
    wt = args.mode in ("stream", "datagram")
    server_command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}", "--workers", "4", "--messages-per-second", "0", "--bytes-per-second", "0", "--ping-interval-secs", "0", "--write-timeout-secs", "30"]
    server_command += ["--dev-cert"] if wt else ["--legacy-websocket"]
    server_command = ["taskset", "-c", args.server_cpus] + server_command
    result = {"args": {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()}, "server_command": server_command, "server_sha256": digest(args.server), "client_sha256": digest(args.client), "runner_sha256": digest(__file__), "client_source_sha256": digest(ROOT / "examples/state_freshness.rs"), "passed": False, "scope": "Independently checked state generation stamps/payloads and reliable bulk. Freshness probes include the pre-first-update period using initial state at t0. Missing datagrams means not received in the observation window, not an attributed network-loss count. All session/resource gauges must retire."}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile() as logs, tempfile.TemporaryFile() as front_logs:
        server = front = None
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
            external_port = port
            url = f"https://127.0.0.1:{port}/war" if wt else f"ws://127.0.0.1:{port}"
            command = [str(args.client.resolve()), "--url", url, "--mode", args.mode, "--seconds", str(args.seconds), "--rate", str(args.rate), "--payload-bytes", str(args.payload_bytes), "--bulk-bytes", str(args.bulk_bytes)]
            if args.client_packet_threshold is not None:
                command += ["--packet-threshold", str(args.client_packet_threshold)]
            if wt:
                text = os.pread(logs.fileno(), os.fstat(logs.fileno()).st_size, 0).decode()
                text = re.sub(r"\x1b\[[0-?]*[ -/]*[@-~]", "", text)
                pin = re.search(r"certificate_sha256=([0-9a-fA-F:]{95})", text)
                if not pin:
                    raise RuntimeError("certificate pin missing")
                command += ["--certificate", pin[1]]
            if args.mode == "websocket-tls":
                if not all((args.tls_cert, args.tls_key, args.tls_ca)):
                    raise ValueError("TLS comparison requires certificate/key/CA paths")
                external_port = front_port
                front_command = ["taskset", "-c", args.server_cpus, str(args.tls_front.resolve()), "--bind", f"127.0.0.1:{front_port}", "--upstream", f"127.0.0.1:{port}", "--cert", str(args.tls_cert.resolve()), "--key", str(args.tls_key.resolve())]
                front = subprocess.Popen(front_command, stdout=front_logs, stderr=front_logs, env=environment)
                deadline = time.monotonic() + 5
                while True:
                    if front.poll() is not None:
                        raise RuntimeError("TLS front end exited")
                    try:
                        with socket.create_connection(("127.0.0.1", front_port), timeout=.2):
                            break
                    except OSError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(.02)
                command[command.index("--url") + 1] = f"wss://127.0.0.1:{front_port}"
                command += ["--ca-certificate", str(args.tls_ca.resolve())]
                result["tls_front_command"] = front_command
                result["tls_front_sha256"] = digest(args.tls_front)
                result["tls_trusted_certificate_sha256"] = digest(args.tls_ca)
            command = ["taskset", "-c", args.client_cpus] + command
            result["client_command"] = command
            impaired = netem(args, external_port, "udp" if wt else "tcp")
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
            expected_input = 1 + (observed["war_datagrams_received"] if wt and args.mode == "datagram" else offered)
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
            for name, process in (("tls_front", front), ("server", server)):
                if process is not None:
                    process.terminate()
                    try:
                        process.wait(timeout=12)
                    except subprocess.TimeoutExpired:
                        process.kill(); process.wait()
                        result["passed"] = False
                        result.setdefault("error", "forced process cleanup")
                    result[name + "_exit_code"] = process.returncode
            if server is not None and server.returncode != 0:
                result["passed"] = False
                result.setdefault("error", "relay did not exit cleanly")
            logs.seek(0); front_logs.seek(0)
            result["server_log"] = logs.read().decode(errors="replace")
            result["tls_front_log"] = front_logs.read().decode(errors="replace")
            args.output.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"passed": result["passed"], "error": result.get("error"), "measurement": result.get("measurement")}))
    return result["passed"]


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/war-relay")
    parser.add_argument("--client", type=Path, default=ROOT / "target/release/examples/state_freshness")
    parser.add_argument("--mode", choices=["websocket", "websocket-tls", "stream", "datagram"], required=True)
    parser.add_argument("--tls-front", type=Path, default=ROOT / "target/release/examples/tls_front")
    parser.add_argument("--tls-cert", type=Path)
    parser.add_argument("--tls-key", type=Path)
    parser.add_argument("--tls-ca", type=Path)
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
