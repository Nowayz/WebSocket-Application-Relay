#!/usr/bin/env python3
"""Measure unrelated routing during valid messages with many empty fragments.

Linux /proc supplies server-only CPU and RSS. Sources finish every message and
verify its exact encoded value; all sockets verify normal Close. No rate limit
or heartbeat masks the parsing workload. Optional affinity separates the client.
"""
import argparse
import concurrent.futures
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import socket
import struct
import subprocess
import tempfile
import threading
import time
import traceback

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("war_benchmark", ROOT / "scripts/benchmark.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)
ZERO_RESOURCES = ("war_connections", "war_sessions", "war_queued_bytes",
                  "war_variable_bytes", "war_read_buffer_reserved_bytes")
ZERO_ERRORS = ("war_protocol_errors_total", "war_write_errors_total",
               "war_write_timeouts_total", "war_close_failures_total",
               "war_transport_disconnects_total", "war_read_buffer_reservations_rejected_total",
               "war_handshake_errors_total", "war_slow_consumers_total", "war_global_pressure_total")


def frame(opcode, body):
    size = len(body)
    if size < 126:
        length = bytes([0x80 | size])
    elif size < 65536:
        length = b"\xfe" + struct.pack(">H", size)
    else:
        length = b"\xff" + struct.pack(">Q", size)
    # Deterministic test mask: no payload transformation or staging per source.
    return bytes([opcode]) + length + bytes(4) + body


def uint(data, offset=0):
    assert data[offset] >> 5 == 0, "expected unsigned CBOR integer"
    additional = data[offset] & 31
    if additional < 24:
        return additional, offset + 1
    width = {24: 1, 25: 2, 26: 4, 27: 8}[additional]
    end = offset + 1 + width
    assert end <= len(data), "truncated integer"
    return int.from_bytes(data[offset+1:end], "big"), end


class Wire:
    """Independent finite raw WebSocket peer; CBOR values stay encoded."""

    def __init__(self, port, channel):
        self.received_data_bytes = 0
        self.socket = socket.create_connection(("127.0.0.1", port), timeout=10)
        self.socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.socket.sendall(
            b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n"
            b"Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n"
            b"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")
        header = bytearray()
        while not header.endswith(b"\r\n\r\n"):
            header.extend(self.exact(1))
            assert len(header) < 8192, "oversized upgrade response"
        assert header.startswith(b"HTTP/1.1 101 "), header
        assert len(channel) <= 16
        self.socket.sendall(frame(0x82, bytes([0x60 + len(channel)]) + channel))
        opcode, data = self.receive()
        assert opcode == 2, (opcode, data)
        self.id, end = uint(data)
        assert end == len(data) and self.id not in (0, 2**64-1)

    def exact(self, length):
        data = bytearray()
        while len(data) < length:
            part = self.socket.recv(length - len(data))
            assert part, "unexpected WebSocket EOF"
            data.extend(part)
        return bytes(data)

    def receive(self):
        first, second = self.exact(2)
        assert first & 0x80 and not first & 0x70 and not second & 0x80
        length = second & 127
        if length == 126:
            length = int.from_bytes(self.exact(2), "big")
        elif length == 127:
            length = int.from_bytes(self.exact(8), "big")
        assert length < 65536, "oversized test response"
        if first & 15 == 2:
            self.received_data_bytes += length
        return first & 15, self.exact(length)

    def packet(self, value):
        # An explicit uint64 target keeps request lengths independent of ID width.
        return b"\x82\x1b" + self.id.to_bytes(8, "big") + value

    def verify(self, value):
        opcode, data = self.receive()
        assert opcode == 2 and data[0] == 0x82, "unexpected routed message"
        sender, end = uint(data, 1)
        assert sender == self.id and data[end:] == value, "encoded value changed"

    def close(self):
        self.socket.sendall(frame(0x88, b"\x03\xe8"))
        assert self.receive() == (8, b"\x03\xe8"), "normal Close was not acknowledged"
        self.socket.close()


def run(args):
    port, admin = runtime.free_port(), runtime.free_port()
    command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}",
               "--admin-bind", f"127.0.0.1:{admin}", "--workers", str(args.workers),
               "--read-buffer-bytes", str(args.read_buffer_bytes),
               "--messages-per-second", "0", "--bytes-per-second", "0",
               "--ping-interval-secs", "0", "--idle-timeout-secs", "0"]
    if args.server_cpus:
        command = ["taskset", "-c", args.server_cpus] + command
    env = {key: value for key, value in os.environ.items() if not key.startswith("WAR_")}
    # Shared, prebuilt wire input: bounded independently of duration and peers.
    empty = frame(0, b"") * args.fragments
    sources, healthy, health_thread = [], None, None
    stop, busy, ready = threading.Event(), threading.Event(), threading.Event()
    latencies, health_errors = [], []
    result = {"server_command": command,
              "server_sha256": hashlib.sha256(args.server.read_bytes()).hexdigest(),
              "samples": [], "passed": False}
    with tempfile.TemporaryFile() as log:
        server = subprocess.Popen(command, env=env, stdout=log, stderr=log)
        try:
            for _ in range(200):
                if server.poll() is not None:
                    raise RuntimeError("server stopped during startup")
                try:
                    runtime.metrics(admin)
                    break
                except OSError:
                    time.sleep(.02)
            else:
                raise TimeoutError("server startup")
            for index in range(args.sources):
                sources.append(Wire(port, f"fragment-{index}".encode()))
            healthy = Wire(port, b"fair-healthy")

            def probe():
                try:
                    count = 0
                    while not stop.is_set():
                        value = b"\x59\x04\x00" + count.to_bytes(8, "big") + b"h"*1016
                        phase = busy.is_set()
                        started = time.monotonic()
                        healthy.socket.sendall(frame(0x82, healthy.packet(value)))
                        healthy.verify(value)
                        latencies.append({"busy": phase, "ms": (time.monotonic()-started)*1000})
                        count += 1
                        ready.set()
                        stop.wait(.001)
                    healthy.close()
                except Exception as error:
                    health_errors.append(repr(error))
                    ready.set()

            health_thread = threading.Thread(target=probe)
            health_thread.start()
            assert ready.wait(5), "healthy probe did not start"
            time.sleep(.2)
            assert not health_errors, health_errors
            result["baseline"] = runtime.snapshot(server.pid)

            def source_traffic(source):
                durations = []
                for wave in range(args.cycles):
                    value = b"\x48" + wave.to_bytes(8, "big")
                    packet = source.packet(value)
                    started = time.monotonic()
                    source.socket.sendall(frame(0x02, packet[:1]))
                    source.socket.sendall(empty)
                    source.socket.sendall(frame(0x80, packet[1:]))
                    source.verify(value)
                    durations.append(time.monotonic() - started)
                source.close()
                return durations

            busy.set()
            started = time.monotonic()
            with concurrent.futures.ThreadPoolExecutor(max_workers=args.sources) as pool:
                jobs = [pool.submit(source_traffic, source) for source in sources]
                while not all(job.done() for job in jobs):
                    assert time.monotonic() - started < 120, "fragment workload deadline"
                    tick = time.monotonic()
                    observed = runtime.metrics(admin)
                    result["samples"].append({"seconds": time.monotonic()-started,
                                              "admin_ms": (time.monotonic()-tick)*1000,
                                              "process": runtime.snapshot(server.pid),
                                              "metrics": observed})
                    time.sleep(.01)
                result["source_durations"] = [job.result() for job in jobs]
            result["elapsed_seconds"] = time.monotonic() - started
            busy.clear()
            result["after_workload"] = runtime.snapshot(server.pid)
            time.sleep(.1)
            stop.set()
            health_thread.join(12)
            assert not health_thread.is_alive() and not health_errors, health_errors
            for _ in range(200):
                final = runtime.metrics(admin)
                if all(int(final[key]) == 0 for key in ZERO_RESOURCES):
                    break
                time.sleep(.02)
            result["final_metrics"] = final
            assert all(int(final[key]) == 0 for key in ZERO_RESOURCES + ZERO_ERRORS), final
            assert int(final["war_channels"]) == 1
            assert int(final["war_joined_total"]) == args.sources + 1
            assert int(final["war_incoming_messages_total"]) == len(latencies) + args.sources*args.cycles
            assert int(final["war_incoming_bytes_total"]) == len(latencies)*1037 + args.sources*args.cycles*19
            assert int(final["war_deliveries_total"]) == len(latencies) + args.sources*args.cycles + args.sources + 1
            assert int(final["war_delivered_bytes_total"]) == healthy.received_data_bytes + sum(s.received_data_bytes for s in sources)
            assert int(final["war_routing_copied_bytes_total"]) == 0
            busy_ms = sorted(item["ms"] for item in latencies if item["busy"])
            assert busy_ms, "no healthy probes overlapped the workload"
            result.update({"latencies": latencies, "busy_probes": len(busy_ms),
                           "busy_p99_ms": busy_ms[min(len(busy_ms)-1, len(busy_ms)*99//100)],
                           "busy_max_ms": max(busy_ms),
                           "admin_max_ms": max(s["admin_ms"] for s in result["samples"]),
                           "verified_fragmented_messages": args.sources*args.cycles,
                           "verified_empty_continuations": args.sources*args.cycles*args.fragments,
                           "clean_close_handshakes": args.sources+1,
                           "server_cpu_seconds": result["after_workload"]["cpu_seconds"]-result["baseline"]["cpu_seconds"],
                           "passed": True})
        except Exception:
            result["error"] = traceback.format_exc()
        finally:
            stop.set()
            for source in sources:
                source.socket.close()
            if healthy is not None:
                healthy.socket.close()
            if health_thread is not None:
                health_thread.join(12)
            server.terminate()
            try:
                server.wait(timeout=12)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()
            log.seek(0)
            result["server_log"] = log.read().decode()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT/"target/release/war-relay")
    parser.add_argument("--workers", type=int, choices=[1, 4], default=4)
    parser.add_argument("--sources", type=int, default=4)
    parser.add_argument("--fragments", type=int, default=500000)
    parser.add_argument("--cycles", type=int, default=8)
    parser.add_argument("--read-buffer-bytes", type=int, default=4096)
    parser.add_argument("--server-cpus")
    parser.add_argument("--client-cpus")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not (1 <= args.sources <= 16 and 1 <= args.fragments <= 2000000 and 1 <= args.cycles <= 8
            and 512 <= args.read_buffer_bytes <= 1024*1024):
        parser.error("use 1..16 sources, 1..2000000 empty continuations, 1..8 cycles, 512 B..1 MiB reads")
    if args.client_cpus:
        cpus = set()
        for part in args.client_cpus.split(","):
            endpoints = part.split("-")
            cpus.update(range(int(endpoints[0]), int(endpoints[-1])+1))
        os.sched_setaffinity(0, cpus)
    record = {"host": platform.platform(),
              "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "args": {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
              "notes": "Finite valid CBOR self routes with many empty masked continuations; exact encoded values and every normal Close verified. Independent 1 KiB self echoes every 1 ms, and admin samples approximately every 10 ms. Client preparation precedes timing. Rates, heartbeats and application idle disabled; ordinary staging/message limits retained. Affinity, read size, worker count and every observed latency are recorded. A scheduler opportunity is not a hard latency guarantee; no production capacity claim.",
              "case": run(args)}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(record, indent=2)+"\n")
    print(json.dumps({key: record["case"].get(key) for key in
                      ("passed", "elapsed_seconds", "busy_probes", "busy_p99_ms", "busy_max_ms", "admin_max_ms", "server_cpu_seconds")}))
    if not record["case"]["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
