#!/usr/bin/env python3
"""Verify Linux accept recovery and shutdown with a child-only descriptor limit.

No system limits are changed. Raw masked WebSocket frames independently check
native CBOR and legacy replies. Build both binaries before running.
"""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import resource
import signal
import socket
import subprocess
import tempfile
import time
import traceback

import benchmark as runtime

ROOT = runtime.ROOT
FD_LIMIT = 64


def exact(stream, count):
    result = bytearray()
    while len(result) < count:
        part = stream.recv(count - len(result))
        if not part:
            raise EOFError("transport ended before the complete reply")
        result.extend(part)
    return bytes(result)


class Client:
    def __init__(self, stream, legacy, name=b"live"):
        self.stream, self.legacy = stream, legacy
        stream.settimeout(5)
        protocol = b"" if legacy else b"Sec-WebSocket-Protocol: war.cbor.v1\r\n"
        stream.sendall(b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: Upgrade\r\n"
                       b"Upgrade: websocket\r\nSec-WebSocket-Version: 13\r\n"
                       b"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n" + protocol + b"\r\n")
        header = bytearray()
        while not header.endswith(b"\r\n\r\n"):
            header.extend(exact(stream, 1))
            assert len(header) < 8192
        assert header.startswith(b"HTTP/1.1 101 ")
        assert len(name) < 24
        self.send(1 if legacy else 2, name if legacy else bytes([0x60 + len(name)]) + name)
        opcode, self.welcome = self.read()
        assert opcode == 2
        if legacy:
            assert len(self.welcome) == 8
            user_id = int.from_bytes(self.welcome, "little")
        else:
            first = self.welcome[0]
            assert first >> 5 == 0
            size = {24: 1, 25: 2, 26: 4, 27: 8}.get(first & 31, 0)
            assert len(self.welcome) == 1 + size
            user_id = int.from_bytes(self.welcome[1:], "big") if size else first
        assert 0 < user_id < (1 << 64) - 1
        self.echo()

    def send(self, opcode, body):
        assert len(body) < 126
        mask = b"\x11\x37\x59\x73"
        self.stream.sendall(bytes([0x80 | opcode, 0x80 | len(body)]) + mask
                            + bytes(value ^ mask[i % 4] for i, value in enumerate(body)))

    def read(self):
        first, second = exact(self.stream, 2)
        assert first & 0x80 and not first & 0x70 and not second & 0x80
        length = second & 127
        if length == 126:
            length = int.from_bytes(exact(self.stream, 2), "big")
        elif length == 127:
            length = int.from_bytes(exact(self.stream, 8), "big")
        assert length < 8192
        return first & 15, exact(self.stream, length)

    def echo(self):
        value = b"\x18\x2a"
        packet = self.welcome + value if self.legacy else b"\x82" + self.welcome + value
        self.send(2, packet)
        assert self.read() == (2, packet)

    def close(self):
        value = b"\x03\xe8accept-pressure"
        self.send(8, value)
        assert self.read() == (8, value)
        assert self.stream.recv(1) == b""
        self.stream.close()


class Management:
    """One retained HTTP connection remains usable when no more FDs can open."""
    def __init__(self, port):
        self.stream = socket.create_connection(("127.0.0.1", port), timeout=3)
        self.stream.settimeout(3)
        self.file = self.stream.makefile("rb")

    def metrics(self):
        self.stream.sendall(b"GET /metrics HTTP/1.1\r\nHost: localhost\r\n\r\n")
        assert self.file.readline().startswith(b"HTTP/1.1 200 ")
        headers = {}
        while (line := self.file.readline()) != b"\r\n":
            assert line
            key, value = line.decode().split(":", 1)
            headers[key.lower()] = value.strip()
        length = int(headers["content-length"])
        body = self.file.read(length)
        assert len(body) == length
        return {key: int(value) for key, value in
                (row.split() for row in body.decode().splitlines() if row.startswith("war_"))}

    def close(self):
        self.file.close()
        self.stream.close()


def wait_live(process, action):
    deadline = time.monotonic() + 5
    while True:
        assert process.poll() is None, f"listener process exited {process.returncode}"
        try:
            return action()
        except ConnectionRefusedError:
            assert time.monotonic() < deadline
            time.sleep(.01)


def fd_count(process):
    assert process.poll() is None, f"listener process exited {process.returncode}"
    return len(list(Path(f"/proc/{process.pid}/fd").iterdir()))


def fill(process, port, held):
    for _ in range(FD_LIMIT * 2):
        held.append(socket.create_connection(("127.0.0.1", port), timeout=3))
        time.sleep(.002)
        if fd_count(process) == FD_LIMIT:
            break
    assert fd_count(process) == FD_LIMIT, "did not reach the child descriptor limit"
    # Keep this already queued flow through recovery. A fresh connect must not
    # be needed to wake the edge-triggered native listener.
    return socket.create_connection(("127.0.0.1", port), timeout=3)


def run_case(args, name):
    legacy = name == "bridge"
    port, admin, bridge_port = runtime.free_port(), runtime.free_port(), runtime.free_port()
    assert len({port, admin, bridge_port}) == 3
    workers = 1 if legacy else int(name.rsplit("-", 1)[1])
    native_command = [str(args.server.resolve()), "--workers", str(workers),
                      "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}",
                      "--handshake-timeout-secs", "60", "--ping-interval-secs", "0"]
    bridge_command = [str(args.bridge.resolve()), "--bind", f"127.0.0.1:{bridge_port}",
                      "--upstream", f"ws://127.0.0.1:{port}"] if legacy else None
    env = {k: v for k, v in os.environ.items() if not k.startswith("WAR_")}
    env["RUST_LOG"] = "war_relay=info"
    record = {"case": name, "rlimit_nofile": FD_LIMIT, "native_command": native_command,
              "bridge_command": bridge_command, "passed": False}
    processes, held, pending, healthy, management = [], [], None, None, None
    with tempfile.TemporaryFile() as logs:
        try:
            def limited():
                resource.setrlimit(resource.RLIMIT_NOFILE, (FD_LIMIT, FD_LIMIT))
            native = subprocess.Popen(native_command, env=env, stdout=logs, stderr=logs,
                                      preexec_fn=None if legacy else limited)
            processes.append(native)
            management = wait_live(native, lambda: Management(admin))
            management.metrics()
            if legacy:
                target = subprocess.Popen(bridge_command, env=env, stdout=logs, stderr=logs,
                                          preexec_fn=limited)
                processes.append(target)
            else:
                target = native
            public = bridge_port if legacy else port
            stream = wait_live(target, lambda: socket.create_connection(("127.0.0.1", public), timeout=3))
            healthy = Client(stream, legacy)
            record["initial_metrics"] = management.metrics()
            def accept_errors():
                if not legacy:
                    return management.metrics().get("war_accept_errors_total", 0)
                return os.pread(logs.fileno(), 1024 * 1024, 0).count(b"legacy accept failed")

            def wait_error(previous):
                deadline = time.monotonic() + 5
                while accept_errors() <= previous:
                    assert target.poll() is None, f"accept pressure terminated the process: {target.returncode}"
                    assert time.monotonic() < deadline, "no accept error observed at the descriptor limit"
                    time.sleep(.01)
                assert b"os error 24" in os.pread(logs.fileno(), 1024 * 1024, 0)

            errors_before = accept_errors()
            pending = fill(target, public, held)
            wait_error(errors_before)
            assert target.poll() is None, f"accept pressure terminated the process: {target.returncode}"
            healthy.echo()
            first = runtime.snapshot(target.pid)
            started = time.monotonic()
            errors_before = management.metrics().get("war_accept_errors_total", 0)
            time.sleep(.6)
            healthy.echo()
            last = runtime.snapshot(target.pid)
            elapsed = time.monotonic() - started
            metrics = management.metrics()
            record["pressure"] = {"fd_count": fd_count(target), "elapsed_seconds": elapsed,
                                  "cpu_seconds": last["cpu_seconds"] - first["cpu_seconds"],
                                  "metrics": metrics}
            if not legacy:
                errors = metrics["war_accept_errors_total"]
                assert errors > 0
                assert errors - errors_before <= workers * (math.ceil(elapsed) + 1), "accept retry spun"
            for stream in held:
                stream.close()
            held.clear()
            started = time.monotonic()
            recovered = Client(pending, legacy, b"recovered")
            pending = None
            recovered.close()
            healthy.echo()
            record["queued_flow_recovery_seconds"] = time.monotonic() - started
            deadline = time.monotonic() + 5
            while True:
                metrics = management.metrics()
                if (metrics["war_connections"] == metrics["war_sessions"] == 1
                        and metrics["war_channels"] == 2
                        and metrics["war_queued_bytes"] == metrics["war_variable_bytes"] == 0):
                    break
                assert time.monotonic() < deadline, "retired pressure sockets retained resources"
                time.sleep(.01)
            assert metrics["war_routing_copied_bytes_total"] == 0
            record["recovered_metrics"] = metrics
            # A second full population proves cancellation interrupts backoff.
            errors_before = accept_errors()
            pending = fill(target, public, held)
            wait_error(errors_before)
            healthy.echo()
            started = time.monotonic()
            target.send_signal(signal.SIGTERM)
            if not legacy:
                opcode, body = healthy.read()
                assert opcode == 8 and int.from_bytes(body[:2], "big") == 1001
                record["shutdown_close"] = 1001
            assert target.wait(timeout=3) == 0
            record["shutdown_seconds"] = time.monotonic() - started
            record["passed"] = True
        except Exception as error:
            record["error"] = repr(error)
            record["traceback"] = traceback.format_exc()
        finally:
            for stream in held:
                stream.close()
            if pending is not None:
                pending.close()
            if healthy is not None:
                healthy.stream.close()
            if management is not None:
                management.close()
            exits = []
            for process in reversed(processes):
                if process.poll() is None:
                    process.send_signal(signal.SIGTERM)
                try:
                    exits.append(process.wait(timeout=5))
                except subprocess.TimeoutExpired:
                    process.kill()
                    exits.append(process.wait(timeout=5))
            record["process_exit_codes"] = exits
            record["passed"] = record["passed"] and all(code == 0 for code in exits)
        logs.seek(0)
        record["server_log"] = logs.read().decode(errors="replace")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/war-relay")
    parser.add_argument("--bridge", type=Path, default=ROOT / "target/release/war-legacy-bridge")
    parser.add_argument("--cases", nargs="+", choices=["native-1", "native-4", "bridge"],
                        default=["native-1", "native-4", "bridge"])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not Path("/proc/self/fd").is_dir():
        parser.error("the descriptor-pressure probe requires Linux /proc")
    result = {"scope": __doc__, "source": Path(__file__).read_text(),
              "server_sha256": hashlib.sha256(args.server.read_bytes()).hexdigest(),
              "bridge_sha256": hashlib.sha256(args.bridge.read_bytes()).hexdigest(),
              "cases": [], "passed": False}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    for name in args.cases:
        case = run_case(args, name)
        result["cases"].append(case)
        args.output.write_text(json.dumps(result, indent=2) + "\n")
        print(json.dumps({k: case[k] for k in ["case", "passed", "process_exit_codes"]}), flush=True)
    result["passed"] = all(case["passed"] for case in result["cases"])
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    raise SystemExit(0 if result["passed"] else 1)


if __name__ == "__main__":
    main()
