#!/usr/bin/env python3
"""Exercise CBOR rejection and fragmented input with independent healthy traffic.

Expected forms are constructed directly from RFC 8949 and WAR's documented
definite-length/depth/item limits. The raw client does not use the Rust parser.
Every worker owns its witness, channel, counters and sockets. This finite corpus
is a regression campaign, not exhaustive fuzzing or a performance benchmark.
"""
import argparse
import concurrent.futures
import hashlib
import importlib.util
import itertools
import json
import os
from pathlib import Path
import platform
import socket
import subprocess
import tempfile
import threading
import time
import traceback
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("war_wire", ROOT / "scripts/fragment-fairness.py")
wire = importlib.util.module_from_spec(spec)
spec.loader.exec_module(wire)
MAX_ID = 2**64 - 1
LIMIT = 1024*1024
TOKEN = "corpus-fixture-admin"
GUARD = b"\x18\x2a"
GET_GUARD = b"\x82\x00\x82\x05\x45guard"
GUARD_REPLY = b"\x82\x00\x82\x18\xc8\x82\xf5" + GUARD
EMPTY = wire.frame(0, b"") * 257


class Peer(wire.Wire):
    def __init__(self, port, *, source_address=None):
        self.socket = socket.create_connection(("127.0.0.1", port), timeout=5, source_address=source_address)
        self.socket.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.id = None
        self.data_frames = self.received_data_bytes = 0
        self.socket.sendall(
            b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n"
            b"Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n"
            b"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")
        header = bytearray()
        while not header.endswith(b"\r\n\r\n"):
            header.extend(self.exact(1))
            assert len(header) < 8192, "oversized upgrade response"
        assert header.startswith(b"HTTP/1.1 101 "), header

    def exact(self, length):
        data = bytearray()
        while len(data) < length:
            part = self.socket.recv(length-len(data))
            if not part:
                raise EOFError("WebSocket transport ended")
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
        assert length <= LIMIT + 16, "oversized test response"
        body = self.exact(length)
        if first & 15 == 2:
            self.data_frames += 1
            self.received_data_bytes += length
        return first & 15, body

    def welcome(self, reply=None):
        opcode, body = reply if reply is not None else self.receive()
        assert opcode == 2, (opcode, body)
        self.id, end = wire.uint(body)
        assert end == len(body) and self.id not in (0, MAX_ID)

    def join(self, channel):
        self.socket.sendall(wire.frame(0x82, bytes([0x40+len(channel)])+channel))
        self.welcome()

    def policy_close(self, expected, *, terminal=False, reply=None):
        opcode, body = reply if reply is not None else self.receive()
        assert opcode == 8 and len(body) >= 2, (opcode, body)
        code = int.from_bytes(body[:2], "big")
        assert code == expected, (code, expected, body)
        if not terminal:
            self.socket.sendall(wire.frame(0x88, body))
            assert self.socket.recv(1) == b"", "server did not finish policy Close"
        self.socket.close()
        return code


def corpus():
    seeds = [
        b"\x00", b"\x18\x00", b"\x1b"+bytes([255])*8,
        b"\x3b"+bytes([255])*8, b"\x58\x01\x80", b"\x78\x02pi",
        b"\x65"+"€π".encode(), b"\x98\x01\xf6", b"\xa2\x00\x01\x00\x02",
        b"\xd8\x18\x43\x00\x01\x02", b"\xf9\x7e\x01", b"\xf9\x80\x00",
        b"\xfa\x7f\x80\x00\x00", b"\xfb\x7f\xf8"+bytes(5)+b"\x01",
        b"\xf8\x20", b"\xf8\xff", b"\x81"*64+b"\x00", b"\xc0"*64+b"\x00",
        b"\x9a\x00\x00\xff\xff"+b"\xf6"*65535,
    ]
    cases = []
    def add(name, stage, payload, code=1002, terminal=False, scope="private"):
        cases.append({"name": name, "stage": stage, "payload": payload,
                      "code": code, "terminal": terminal, "scope": scope, "peer_close": False})
    for index, value in enumerate(seeds):
        add(f"valid-{index}", "value", value, code=0)
    invalid = {}
    for index, value in enumerate(seeds[:16]):
        for end in range(len(value)):
            invalid.setdefault(value[:end], f"truncated-{index}-{end}")
        invalid.setdefault(value+b"\x00", f"trailing-{index}")
    for major in range(8):
        for additional in range(28, 32):
            invalid.setdefault(bytes([major*32+additional]), f"unsupported-or-reserved-{major}-{additional}")
    for value in range(32):
        for prefix in [b"", b"\x81", b"\xa1\x00", b"\xc0"]:
            invalid.setdefault(prefix+b"\xf8"+bytes([value]), f"reserved-simple-{prefix.hex()}-{value}")
    for text in [b"\x80", b"\xc0\xaf", b"\xed\xa0\x80", b"\xf4\x90\x80\x80", b"\xe2\x82"]:
        invalid[bytes([0x60+len(text)])+text] = "invalid-utf8-"+text.hex()
    for marker in [b"\x81", b"\xc0"]:
        invalid[marker*65+b"\x00"] = "excessive-depth-"+marker.hex()
    for major in [0x5b, 0x7b, 0x9b, 0xbb]:
        invalid[bytes([major])+bytes([255])*8] = f"impossible-length-{major}"
    invalid[b"\x9a\x00\x01\x00\x00"+b"\xf6"*65536] = "excessive-items"
    for value, name in invalid.items():
        for scope in ["private", "other-target", "variable"]:
            add(name+"-"+scope, "value", value, scope=scope)
    for name, payload, code in [
        ("join-not-string", b"\x00", 1002), ("join-break", b"\xff", 1002),
        ("join-truncated", b"\x65bad", 1002), ("join-trailing", b"\x60\x00", 1002),
        ("join-invalid-utf8", b"\x61\xff", 1002), ("join-too-long", b"\x51"+b"x"*17, 1002),
    ]:
        add(name, "join", payload, code)
    malformed_frames = [
        ("unmasked", b"\x82\x01\xf6", 1002, True),
        ("reserved-bit", wire.frame(0xc2, b"\x00"), 1002, True),
        ("reserved-opcode", wire.frame(0x83, b"\x00"), 1002, True),
        ("unexpected-continuation", wire.frame(0x80, b"\x00"), 1002, True),
        ("fragmented-ping", wire.frame(0x09, b""), 1002, True),
        ("large-ping", wire.frame(0x89, bytes(126)), 1002, True),
        ("invalid-close-code", wire.frame(0x88, b"\x03\xed"), 1002, True),
        ("short-close", wire.frame(0x88, b"x"), 1002, True),
        ("invalid-text-utf8", wire.frame(0x81, b"\xff"), 1002, True),
        ("unsupported-text", wire.frame(0x81, b"text"), 1003, False),
        ("oversized-header-only", b"\x82\xff"+(LIMIT+1).to_bytes(8,"big")+bytes(4), 1009, True),
    ]
    for name, payload, code, terminal in malformed_frames:
        for stage in ["before-frame", "after-frame"]:
            add(name+"-"+stage, stage, payload, code, terminal)
            # tungstenite normalizes this peer Close to a 1002 reply and
            # returns Message::Close, rather than a terminal decoder error.
            cases[-1]["peer_close"] = name == "invalid-close-code"
    return cases


def send_message(peer, payload, mode):
    if mode != "fragmented":
        peer.socket.sendall(wire.frame(0x82, payload))
        return
    # Force cooperative steps while retaining an unfinished message, then vary
    # nonempty fragment sizes across numeric arguments and UTF-8 codepoints.
    peer.socket.sendall(wire.frame(0x02, payload[:1])+EMPTY+wire.frame(0x89, b"corpus"))
    assert peer.receive() == (10, b"corpus"), "interleaved Pong changed"
    position = 1
    frames = bytearray()
    for width in itertools.cycle([1, 2, 5, 13, 64, 257, 1024]):
        end = min(len(payload), position+width)
        frames.extend(wire.frame(0x80 if end == len(payload) else 0, payload[position:end]))
        position = end
        if position == len(payload):
            break
    peer.socket.sendall(frames)


class Worker:
    def __init__(self, port, index, legacy_expectation):
        self.port, self.channel = port, f"corpus-{index}".encode()
        self.baseline = legacy_expectation
        self.peers, self.rows = [], []
        self.counts = dict(joins=0, incoming_messages=0, incoming_bytes=0,
                           join_rejected=0, protocol_errors=0, normal_closes=0,
                           policy_handshakes=0, terminal_close_receipts=0, normalized_peer_close_replies=0,
                           expected_prejoin_drops=0, disconnect_notifications=0)
        self.witness = self.open(join=True)
        self.app(self.witness, b"\x82\x00\x82\x04\x82\x45guard"+GUARD)
        assert self.witness.receive() == (2, b"\x82\x00\x82\x04\xf5")

    def open(self, join=False):
        peer = Peer(self.port)
        self.peers.append(peer)
        if join:
            peer.join(self.channel)
            self.counts["joins"] += 1
        return peer

    def account(self, payload):
        self.counts["incoming_messages"] += 1
        self.counts["incoming_bytes"] += len(payload)

    def app(self, peer, payload, mode="whole"):
        self.account(payload)
        send_message(peer, payload, mode)

    def barrier(self, departed, expect_departure):
        value = b"\x48"+len(self.rows).to_bytes(8, "big")
        self.app(self.witness, self.witness.packet(value))
        seen = 0
        while True:
            opcode, body = self.witness.receive()
            assert opcode == 2 and body[0] == 0x82
            sender, end = wire.uint(body, 1)
            if sender == MAX_ID:
                actual, final = wire.uint(body, end)
                assert final == len(body) and actual not in (0, MAX_ID)
                assert expect_departure and seen == 0
                assert departed is None or departed == actual
                seen += 1
                continue
            assert sender == self.witness.id and body[end:] == value, "rejected data reached witness"
            assert seen == int(expect_departure), "missing/duplicate disconnect notification"
            self.counts["disconnect_notifications"] += seen
            return

    def execute(self, case, mode):
        peer = self.open()
        stage, scope, code = case["stage"], case["scope"], case["code"]
        joined = stage in ("value", "after-frame")
        payload, first_reply = case["payload"], None
        if stage == "value":
            if scope == "variable":
                payload = b"\x82\x00\x82\x04\x82\x45guard"+payload
            else:
                target = self.witness.id if scope == "private" else 0
                if scope == "other-target":
                    # Validation must precede routing, including to a target
                    # other than this witness. Do not assume random ID widths.
                    target = next(i for i in range(1, 100) if i != self.witness.id)
                payload = b"\x82\x1b"+target.to_bytes(8,"big")+payload
        if joined:
            self.counts["joins"] += 1
            if mode == "pipeline":
                data = wire.frame(0x82, payload) if stage == "value" else payload
                peer.socket.sendall(wire.frame(0x82, bytes([0x40+len(self.channel)])+self.channel)+data)
                first_reply = peer.receive()
                if first_reply[0] == 2:
                    peer.welcome(first_reply)
                    first_reply = None
            else:
                peer.join(self.channel)
        if stage == "value":
            self.account(payload)
            if mode != "pipeline":
                send_message(peer, payload, mode)
        elif stage == "join":
            send_message(peer, payload, mode)
        elif mode != "pipeline" or not joined:
            if mode == "fragmented":
                for byte in payload:
                    peer.socket.sendall(bytes([byte]))
            else:
                peer.socket.sendall(payload)
        dropped = self.baseline and stage == "before-frame" and case["terminal"] and not case["peer_close"]
        if dropped:
            try:
                peer.receive()
            except (EOFError, ConnectionResetError):
                self.counts["expected_prejoin_drops"] += 1
            else:
                raise AssertionError("expected old-release prejoin reset")
            peer.socket.close()
        elif code:
            peer.policy_close(code, terminal=case["terminal"], reply=first_reply)
            category = "normalized_peer_close_replies" if case["peer_close"] else (
                "terminal_close_receipts" if case["terminal"] else "policy_handshakes")
            self.counts[category] += 1
        else:
            assert first_reply is None and peer.id is not None
            opcode, body = self.witness.receive()
            assert opcode == 2 and body[0] == 0x82
            sender, end = wire.uint(body, 1)
            assert sender == peer.id and body[end:] == case["payload"], "valid encoded value changed"
            peer.close()
            self.counts["normal_closes"] += 1
        if code and not joined and not case["peer_close"]:
            self.counts["join_rejected"] += 1
        if code == 1002 and joined and not case["peer_close"]:
            self.counts["protocol_errors"] += 1
        self.barrier(peer.id, joined)
        if scope == "variable":
            self.app(self.witness, GET_GUARD)
            assert self.witness.receive() == (2, GUARD_REPLY), "invalid input mutated a variable"
        self.rows.append({"name": case["name"], "mode": mode, "stage": stage,
                          "scope": scope, "bytes": len(payload),
                          "sha256": hashlib.sha256(payload).hexdigest(),
                          "expected_close": code or None, "expected_old_drop": dropped,
                          "welcome_observed": peer.id is not None,
                          "normalized_peer_close": case["peer_close"], "passed": True})

    def run(self, work):
        try:
            for case, mode in work:
                try:
                    self.execute(case, mode)
                except Exception as error:
                    raise RuntimeError(f"{case['name']} / {mode}: {error}") from error
            self.witness.close()
            self.counts["normal_closes"] += 1
            return {"cases": self.rows, "expected": self.counts,
                    "received_data_frames": sum(p.data_frames for p in self.peers),
                    "received_data_bytes": sum(p.received_data_bytes for p in self.peers)}
        finally:
            for peer in self.peers:
                peer.socket.close()


def admin_request(port, path, method="GET"):
    request = urllib.request.Request(f"http://127.0.0.1:{port}/{path}", method=method,
                                     headers={"Authorization": "Bearer "+TOKEN})
    return urllib.request.urlopen(request, timeout=2)


def metrics(port):
    with admin_request(port, "metrics") as response:
        return {key: int(value) for key, value in (line.split() for line in response.read().decode().splitlines() if line.startswith("war_"))}


def run(args):
    port, admin = wire.runtime.free_port(), wire.runtime.free_port()
    command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}",
               "--workers", str(args.workers), "--messages-per-second", "0", "--bytes-per-second", "0",
               "--ping-interval-secs", "0", "--idle-timeout-secs", "0", "--join-timeout-secs", "2",
               "--write-timeout-secs", "1", "--max-message-bytes", str(LIMIT),
               "--total-read-buffer-bytes", str(64*LIMIT)]
    env = {key: value for key, value in os.environ.items() if not key.startswith("WAR_")}
    env["WAR_ADMIN_TOKEN"] = TOKEN
    server_env = wire.runtime.preload_environment(env, args.server_preload)
    result = {"server_command": command, "server_sha256": hashlib.sha256(args.server.read_bytes()).hexdigest(),
              "passed": False, "samples": []}
    stop, ready = threading.Event(), threading.Event()
    healthy = None
    thread = None
    healthy_stats, healthy_errors = [], []
    with tempfile.TemporaryFile() as log:
        server = subprocess.Popen(command, env=server_env, stdout=log, stderr=log)
        try:
            for _ in range(200):
                if server.poll() is not None:
                    raise RuntimeError("server exited during startup")
                try:
                    metrics(admin)
                    break
                except OSError:
                    time.sleep(.02)
            else:
                raise TimeoutError("server startup")
            result["server_preload"] = wire.runtime.preload_metadata(args.server_preload, server.pid)
            healthy = Peer(port)
            healthy.join(b"corpus-healthy")
            def probe():
                try:
                    while not stop.is_set():
                        value = b"\x59\x04\x00"+len(healthy_stats).to_bytes(8,"big")+b"h"*1016
                        started = time.monotonic()
                        healthy.socket.sendall(wire.frame(0x82, healthy.packet(value)))
                        healthy.verify(value)
                        healthy_stats.append((time.monotonic()-started)*1000)
                        ready.set()
                        stop.wait(.002)
                    healthy.close()
                except Exception:
                    healthy_errors.append(traceback.format_exc())
                    ready.set()
            thread = threading.Thread(target=probe)
            thread.start()
            assert ready.wait(5) and not healthy_errors, healthy_errors
            cases = corpus()
            if args.smoke:
                cases = [c for c in cases if c["stage"] != "value"] + [c for c in cases if c["stage"] == "value"][:12]
            work = [(case, mode) for case in cases for mode in args.modes
                    if mode != "pipeline" or case["stage"] in ("value", "after-frame")]
            result["corpus_cases"] = len(cases)
            result["scheduled_exchanges"] = len(work)
            started = time.monotonic()
            with concurrent.futures.ThreadPoolExecutor(max_workers=args.clients) as pool:
                jobs = [pool.submit(Worker(port, index, args.expect_prejoin_drop).run, work[index::args.clients]) for index in range(args.clients)]
                while not all(job.done() for job in jobs):
                    assert time.monotonic()-started < 180, "corpus deadline"
                    result["samples"].append({"seconds": time.monotonic()-started,
                                              "metrics": metrics(admin), "process": wire.runtime.snapshot(server.pid)})
                    time.sleep(.1)
                workers = [job.result() for job in jobs]
            stop.set()
            thread.join(7)
            assert not thread.is_alive() and not healthy_errors, healthy_errors
            for _ in range(200):
                final = metrics(admin)
                if all(final[key] == 0 for key in ("war_connections", "war_sessions", "war_queued_bytes", "war_variable_bytes", "war_read_buffer_reserved_bytes")):
                    break
                time.sleep(.02)
            result["final_metrics"] = final
            expected = {key: sum(worker["expected"][key] for worker in workers) for key in workers[0]["expected"]}
            expected_metrics = {
                "war_joined_total": expected["joins"]+1,
                "war_disconnected_total": expected["joins"]+1,
                "war_incoming_messages_total": expected["incoming_messages"]+len(healthy_stats),
                "war_incoming_bytes_total": expected["incoming_bytes"]+len(healthy_stats)*1037,
                "war_protocol_errors_total": expected["protocol_errors"],
                "war_join_rejected_total": 0 if args.expect_prejoin_drop else expected["join_rejected"],
                "war_deliveries_total": sum(w["received_data_frames"] for w in workers)+healthy.data_frames,
                "war_delivered_bytes_total": sum(w["received_data_bytes"] for w in workers)+healthy.received_data_bytes,
            }
            # Preserve useful evidence even when a counter assertion fails.
            result.update({"workers": workers, "worker_expected": expected, "expected_metrics": expected_metrics,
                           "verified_close_counts": {"normal": expected["normal_closes"]+1,
                                                     "policy_handshakes": expected["policy_handshakes"],
                                                     "terminal_decoder_receipts": expected["terminal_close_receipts"],
                                                     "normalized_peer_close_replies": expected["normalized_peer_close_replies"]},
                           "healthy_verified": len(healthy_stats), "healthy_max_rtt_ms": max(healthy_stats)})
            for key, value in expected_metrics.items():
                assert final.get(key, 0) == value, (key, final.get(key, 0), value)
            assert all(final[key] == 0 for key in ("war_connections", "war_sessions", "war_queued_bytes", "war_variable_bytes", "war_read_buffer_reserved_bytes", "war_close_failures_total", "war_transport_disconnects_total", "war_write_errors_total", "war_write_timeouts_total", "war_handshake_errors_total", "war_read_buffer_reservations_rejected_total", "war_routing_copied_bytes_total")), final
            assert final["war_channels"] == 1
            result.update({"elapsed_seconds": time.monotonic()-started, "passed": True})
        except Exception:
            result["error"] = traceback.format_exc()
        finally:
            stop.set()
            if healthy is not None:
                healthy.socket.close()
            if thread is not None:
                thread.join(7)
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
    parser.add_argument("--server-preload", type=wire.runtime.preload_library,
                        help="Preload one shared library into the server only; verify its mapping and record its hash")
    parser.add_argument("--workers", type=int, choices=[1,4], default=4)
    parser.add_argument("--clients", type=int, choices=range(1,9), default=4)
    parser.add_argument("--modes", nargs="+", choices=["whole","fragmented","pipeline"], default=["whole","fragmented","pipeline"])
    parser.add_argument("--expect-prejoin-drop", action="store_true", help="Reproduce the old release's terminal prejoin drops; not valid rejection behavior")
    parser.add_argument("--smoke", action="store_true")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    record = {"host": platform.platform(), "revision": subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
              "script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "wire_helper_sha256": hashlib.sha256((ROOT/"scripts/fragment-fairness.py").read_bytes()).hexdigest(),
              "args": {key:str(value) if isinstance(value,Path) else value for key,value in vars(args).items()},
              "scope": "Finite independent raw WebSocket/CBOR corpus. Definite WAR forms are distinguished from malformed or excluded indefinite forms. Fragmented messages include 257 empty continuations, interleaved Ping/Pong and mixed nonempty chunks. Raw bad frames are written byte by byte in fragmented mode; TCP may coalesce writes. Pipelined join/data may reject before a welcome is sent. Complete-message policy rejections verify acknowledgement and EOF; terminal decoder failures only verify Close receipt. Each worker verifies its exact disconnect events and uses a FIFO self barrier to prove invalid values were not delivered; variable sentinels must survive invalid updates. Independent healthy traffic continues. Old-reset mode verifies expected defects rather than acceptance. Not exhaustive fuzzing or a latency/capacity claim.",
              "case": run(args)}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(record,indent=2)+"\n")
    print(json.dumps({key:record["case"].get(key) for key in ("passed","corpus_cases","scheduled_exchanges","healthy_verified","healthy_max_rtt_ms","elapsed_seconds","verified_close_counts","error")}))
    if not record["case"]["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
