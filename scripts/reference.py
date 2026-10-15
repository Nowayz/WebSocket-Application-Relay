#!/usr/bin/env python3
"""Compare the preserved C++ executable with the Rust legacy bridge on the wire.

Linux/x86-64 only. Old libraries live in artifacts/reference; all listening and
client sockets live in a private bubblewrap network namespace. No pip packages,
system library installation, C++ source changes, or public listeners are needed.
"""

import argparse
import base64
from collections import defaultdict
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import select
import shutil
import socket
import struct
import subprocess
import sys
import tarfile
import time
import urllib.request


ROOT = Path(__file__).resolve().parent.parent
CACHE = ROOT / "artifacts/reference"
UPSTREAM = "881c8daaf2cf05adea8f522b28cf372e8f82d4e9"
APP_SHA = "ce95e4dea01a4db0e5c32942d5520b5f4a8ab1c87affdcd8120bc8aefa332547"
# SHA256 entries from Debian bullseye/main/binary-amd64/Packages.xz.
PACKAGES = [
    ("o/openssl/libssl1.1_1.1.1w-0+deb11u1_amd64.deb",
     "aadf8b4b197335645b230c2839b4517aa444fd2e8f434e5438c48a18857988f7"),
    ("t/tbb/libtbb2_2020.3-1_amd64.deb",
     "87ac7778820cbd7a2c7485f7fccd2ac4437b39cbb390e49b3aeec38a826f5af9"),
]
MAX_ID = (1 << 64) - 1
ZERO_REPLY = bytes(8) + b"\xc8"
TEST_PASSWORD = b"metalgear"  # Public upstream fixture, set only on the isolated Rust process.


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def prepare():
    (CACHE / "packages").mkdir(parents=True, exist_ok=True)
    (CACHE / "root").mkdir(exist_ok=True)
    records = []
    for name, digest in PACKAGES:
        path = CACHE / "packages" / Path(name).name
        url = "https://archive.debian.org/debian/pool/main/" + name
        if not path.exists():
            with urllib.request.urlopen(url, timeout=60) as response:
                temporary = path.with_suffix(".download")
                temporary.write_bytes(response.read())
                temporary.replace(path)
        if sha(path) != digest:
            raise RuntimeError(f"package checksum mismatch: {path}")
        members = subprocess.check_output(["ar", "t", str(path)], text=True).splitlines()
        member = next(x for x in members if x.startswith("data.tar."))
        data = subprocess.check_output(["ar", "p", str(path), member])
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:*") as archive:
            archive.extractall(CACHE / "root", filter="data")
        records.append({"url": url, "sha256": digest})
    if sha(ROOT / "app") != APP_SHA:
        raise RuntimeError("reference executable differs from the pinned upstream app")
    shutil.copyfile(ROOT / "app", CACHE / "app")
    (CACHE / "app").chmod(0o755)
    return records


def u64(value):
    return struct.pack("<Q", value)


def command(op, data=b""):
    return bytes(8) + bytes([op]) + data


class Client:
    """Small independent masked WebSocket test client; application bytes stay raw."""

    def __init__(self, port, channel, text=False, reject=False):
        self.socket = socket.create_connection(("127.0.0.1", port), timeout=3)
        self.socket.settimeout(3)
        self.closed = False
        self.socket.sendall(
            b"GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n"
            b"Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n"
            b"Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\r\n")
        header = bytearray()
        while not header.endswith(b"\r\n\r\n"):
            header.extend(self.exact(1))
            assert len(header) < 8192, "oversized upgrade reply"
        assert header.startswith(b"HTTP/1.1 101 "), header
        self.send(1 if text else 2, channel)
        opcode, payload = self.receive()
        if reject:
            assert opcode == 8, (opcode, payload)
            self.rejection = payload
            return
        assert opcode == 2 and len(payload) == 8, (opcode, payload)
        self.id = struct.unpack("<Q", payload)[0]
        assert self.id not in (0, MAX_ID), self.id

    def exact(self, length):
        data = bytearray()
        while len(data) < length:
            part = self.socket.recv(length - len(data))
            if not part:
                raise EOFError("unexpected WebSocket EOF")
            data.extend(part)
        return bytes(data)

    def send(self, opcode, payload):
        mask = os.urandom(4)
        size = len(payload)
        header = bytes([0x80 | opcode])
        if size < 126:
            header += bytes([0x80 | size])
        elif size < 65536:
            header += b"\xfe" + struct.pack(">H", size)
        else:
            header += b"\xff" + struct.pack(">Q", size)
        body = bytes(value ^ mask[i & 3] for i, value in enumerate(payload))
        self.socket.sendall(header + mask + body)

    def receive(self):
        fragments = bytearray()
        first = None
        while True:
            a, b = self.exact(2)
            assert not b & 0x80 and not a & 0x70, "invalid uncompressed server frame"
            size = b & 0x7f
            if size == 126:
                size = struct.unpack(">H", self.exact(2))[0]
            elif size == 127:
                size = struct.unpack(">Q", self.exact(8))[0]
            assert size <= 2 * 1024 * 1024, "oversized test reply"
            payload = self.exact(size)
            opcode = a & 15
            if opcode == 9:
                self.send(10, payload)
                continue
            if opcode == 8:
                self.closed = True
                self.send(8, payload)
                return opcode, payload
            if opcode == 10:
                continue
            if opcode:
                assert first is None and opcode in (1, 2), opcode
                first = opcode
            else:
                assert first is not None, "unexpected continuation"
            fragments.extend(payload)
            if a & 0x80:
                return first, bytes(fragments)


class Suite:
    def __init__(self, port, implementation, reference_started):
        self.port, self.implementation = port, implementation
        self.reference_started = reference_started
        self.clients = {}
        self.names = {}
        self.common, self.differences = [], []

    def join(self, label, channel, text=False):
        client = Client(self.port, channel, text)
        assert client.id not in self.names, "duplicate live welcome ID"
        self.clients[label] = client
        self.names[client.id] = label
        return client

    def wire(self, sender, target, body, text=False):
        prefix = base64.b64encode(u64(target)) if text else u64(target)
        output = base64.b64encode(u64(sender.id)) if text else u64(sender.id)
        return (1 if text else 2, prefix + body), (1 if text else 2, output + body)

    def collect(self, expected):
        received = defaultdict(list)
        deadline = time.monotonic() + 3
        while True:
            watched = {c.socket: label for label, c in self.clients.items() if not c.closed}
            complete = all(received[label] == frames for label, frames in expected.items())
            wait = min(.075 if complete else 3, max(0, deadline - time.monotonic()))
            ready, _, _ = select.select(list(watched), [], [], wait)
            if not ready:
                break
            for stream in ready:
                label = watched[stream]
                received[label].append(self.clients[label].receive())
            if time.monotonic() >= deadline:
                break
        actual = {k: v for k, v in received.items() if v}
        wanted = {k: v for k, v in expected.items() if v}
        assert actual == wanted, {"actual": self.normalized(actual), "expected": self.normalized(wanted)}
        return actual

    def normalized(self, frames):
        def frame(opcode, data):
            if opcode == 8:
                return {"close_code": int.from_bytes(data[:2], "big"), "reason_hex": data[2:].hex()}
            sender = struct.unpack("<Q", base64.b64decode(data[:12]) if opcode == 1 else data[:8])[0]
            body = data[12:] if opcode == 1 else data[8:]
            value = ({"departed": self.names.get(struct.unpack("<Q", body)[0], "0")}
                     if sender == MAX_ID and len(body) == 8 else
                     {"hex": body.hex()} if len(body) <= 64 else
                     {"length": len(body), "sha256": hashlib.sha256(body).hexdigest()})
            return {"type": "text" if opcode == 1 else "binary",
                    "sender": self.names.get(sender, "broadcast" if sender == MAX_ID else "relay"),
                    "value": value}
        return {label: [frame(*item) for item in frames[label]] for label in sorted(frames)}

    def exchange(self, name, label, wire, deliveries=None, difference=False, barrier=True):
        sender = self.clients[label]
        barrier = barrier and wire[0] != 8
        expected = defaultdict(list)
        for destination, frame in deliveries or []:
            expected[destination].append(frame)
        sender.send(*wire)
        if barrier:
            sender.send(2, command(5, b"__barrier"))
            expected[label].append((2, ZERO_REPLY))
        try:
            actual = self.collect(expected)
        except Exception as error:
            raise AssertionError(f"{self.implementation}: {name}: {error}") from error
        if barrier:
            actual[label].pop()  # FIFO command reply proves the preceding input ran.
            if not actual[label]:
                del actual[label]
        record = {"case": name, "frames": self.normalized(actual)}
        (self.differences if difference else self.common).append(record)

    def route(self, name, source, target, destinations, body=b"payload", text=False):
        wire, output = self.wire(self.clients[source], target, body, text)
        self.exchange(name, source, wire, [(label, output) for label in destinations])

    def query(self, label):
        client = self.clients[label]
        client.send(2, command(2))
        opcode, data = client.receive()
        assert opcode == 2 and data[:8] == bytes(8), "channel reply header"
        count = int.from_bytes(data[8:12], "little")
        names, position = [], 12
        for _ in range(count):
            size = data[position]
            names.append(data[position + 1:position + 1 + size].hex())
            position += 1 + size
        assert len(data) == position + count * 4, "channel reply layout"
        return dict(sorted((name, int.from_bytes(data[position + i * 4:position + i * 4 + 4], "little"))
                           for i, name in enumerate(names)))

    def close(self):
        failed = sys.exc_info()[0] is not None
        errors, acknowledged = [], 0
        close = struct.pack(">H", 4002) + b"reference finished"
        try:
            for client in self.clients.values():
                if not client.closed:
                    client.send(8, close)
            for label, client in self.clients.items():
                if not client.closed:
                    while True:
                        opcode, payload = client.receive()
                        if opcode == 8:
                            assert payload == close, (label, payload)
                            acknowledged += 1
                            break
        except (OSError, EOFError, AssertionError) as error:
            errors.append(str(error))
        finally:
            for client in self.clients.values():
                client.socket.close()
        if errors and not failed:
            raise AssertionError(f"{self.implementation} cleanup: {errors}")
        return acknowledged

    def run(self):
        try:
            original = self.implementation == "cpp"
            a = self.join("a", b"diff-a", text=True)
            b = self.join("b", b"diff-a")
            c = self.join("c", b"diff-b")
            self.join("g", b"re_globl")
            self.join("h", b"re_globl")
            self.join("empty", b"")
            self.join("sixteen", bytes(range(16)))
            self.join("bytes", b"\xff\x00\xfe")
            self.join("utf8", "π".encode() * 8, text=True)
            self.common.append({"case": "join-boundaries", "unique_nonreserved_ids": 9})
            for text, channel in ((False, b"x" * 17), (True, "π".encode() * 9)):
                rejected = Client(self.port, channel, text, reject=True)
                rejected.socket.close()
                reason = b"Channel Length Exceeded" + (b"\0" if original else b"")
                assert rejected.rejection == struct.pack(">H", 1002) + reason
                self.common.append({"case": f"oversized-{'text' if text else 'binary'}-channel", "close_code": 1002})
                self.differences.append({"case": f"oversized-{'text' if text else 'binary'}-close-reason",
                                         "reason_hex": reason.hex()})
            self.exchange("authenticate", "a", (2, command(0, TEST_PASSWORD)))
            channels = self.query("a")
            assert channels == {name.hex(): count for name, count in
                                [(b"diff-a", 2), (b"diff-b", 1), (b"re_globl", 2),
                                 (b"", 1), (bytes(range(16)), 1), (b"\xff\x00\xfe", 1),
                                 ("π".encode() * 8, 1)]}
            self.common.append({"case": "channel-list-layout", "channels_hex": channels})
            for text in (False, True):
                mode = "text" if text else "binary"
                body = "π\0hello".encode() if text else b"\0\xff\x01payload"
                self.route(f"{mode}-channel", "a", MAX_ID, ["b"], body, text)
                self.route(f"{mode}-cross-channel", "a", c.id, ["c"], body, text)
                self.route(f"{mode}-self", "a", a.id, ["a"], body, text)
                self.route(f"{mode}-unknown", "a", 0xfeeddeadbeef1234, [], body, text)
            for label, flags in (("g", 1), ("h", 2)):
                self.exchange(f"authenticate-{label}", label, (2, command(0, TEST_PASSWORD)))
                self.exchange(f"listener-{flags}", label, (2, command(1, bytes([flags]))))
            for text in (False, True):
                mode = "text" if text else "binary"
                self.route(f"{mode}-channel-listener", "a", MAX_ID, ["b", "g"], text=text)
                self.route(f"{mode}-private-listener", "a", c.id, ["c", "h"], text=text)
                self.route(f"{mode}-listener-target-dedup", "a", self.clients["h"].id, ["h"], text=text)
                self.route(f"{mode}-global", "g", MAX_ID,
                           [label for label in self.clients if label != "g"], text=text)
            for size in (117, 118, 65527, 65528):
                body = bytes(range(256)) * (size // 256) + bytes(range(size % 256))
                self.route(f"frame-body-{size}", "a", b.id, ["b", "h"], body)
            first_wire, first_output = self.wire(a, b.id, b"first")
            a.send(*first_wire)
            second_wire, second_output = self.wire(a, b.id, b"second")
            self.exchange("ordered-pair", "a", second_wire,
                          [("b", first_output), ("b", second_output),
                           ("h", first_output), ("h", second_output)])
            self.exchange("text-relay-target-ignored", "a", (1, base64.b64encode(bytes(8)) + b"ignored"))
            self.exchange("malformed-listen-ignored", "a", (2, command(1)))
            key, value = b"k\x00\xff", b"value\x00\xfe"
            self.exchange("variable-write", "a", (2, command(4, bytes([len(key)]) + key + value)))
            self.exchange("variable-shared-read", "b", (2, command(5, key)), [("b", (2, ZERO_REPLY + value))])
            self.exchange("variable-isolation", "c", (2, command(5, key)), [("c", (2, ZERO_REPLY))])
            self.exchange("variable-empty-write", "a", (2, command(4, b"\x01e")))
            self.exchange("variable-empty-read", "b", (2, command(5, b"e")), [("b", (2, ZERO_REPLY))])
            self.exchange("variable-missing-read", "b", (2, command(5, b"missing")), [("b", (2, ZERO_REPLY))])
            self.exchange("disconnect-listener", "g", (2, command(1, b"\x07")))
            fixed = 0x0807060504030201
            self.exchange("fixed-id", "a", (2, command(3, u64(fixed))))
            self.names[fixed] = "a"
            a.id = fixed
            self.route("fixed-id-private", "c", fixed, ["a", "g", "h"])
            self.route("private-sender-is-listener", "g", c.id, ["c", "g", "h"])
            self.exchange("authenticate-takeover", "b", (2, command(0, TEST_PASSWORD)))
            departed = 0 if original else fixed
            close = struct.pack(">H", 4001) + b"UserID Taken" + (b"\0" if original else b"")
            event = (2, u64(MAX_ID) + u64(departed))
            self.exchange("takeover-departed-id-and-close-reason", "b", (2, command(3, u64(fixed))),
                          [("a", (8, close)), ("b", event), ("g", event)], difference=True)
            b.id = fixed
            self.names[fixed] = "b"
            self.route("takeover-new-owner", "c", fixed, ["b", "g", "h"])
            h = self.clients["h"]
            event = (2, u64(MAX_ID) + u64(h.id))
            close = struct.pack(">H", 4002) + b"client finished"
            self.exchange("global-departure-dedup", "h", (8, close),
                          [("h", (8, close))] + [("g", event)] * (2 if original else 1), difference=True)
            assert time.monotonic() - self.reference_started < 25, "missed C++ pre-GC window; rerun the comparison"
            channels = self.query("b")
            expected = 2 if original else 1
            assert channels[b"diff-a".hex()] == expected and channels[b"re_globl".hex()] == expected, channels
            self.differences.append({"case": "departed-membership-retirement", "channels_hex": channels})
            d = self.join("retired", b"retired")
            self.exchange("retirement-variable-write", "retired", (2, command(4, b"\x01kretained")))
            event = (2, u64(MAX_ID) + u64(d.id))
            self.exchange("ordinary-departure", "retired", (8, close), [("retired", (8, close)), ("g", event)])
            self.join("rejoined", b"retired")
            reply = ZERO_REPLY + (b"retained" if original else b"")
            self.exchange("last-member-variable-retirement", "rejoined", (2, command(5, b"k")),
                          [("rejoined", (2, reply))], difference=True)
            assert time.monotonic() - self.reference_started < 25, "missed C++ pre-GC window; rerun the comparison"
            u = self.join("unauthenticated", b"unauth")
            reason = struct.pack(">H", 1008) + b"Authentication Required"
            event = (2, u64(MAX_ID) + u64(u.id))
            self.exchange("unauthenticated-query-policy", "unauthenticated", (2, command(2)),
                          [] if original else [("unauthenticated", (8, reason)), ("g", event)],
                          difference=True, barrier=original)
            close = struct.pack(">H", 4001) + b"UserID Taken\0"
            self.exchange("same-id-assignment", "g", (2, command(3, u64(self.clients["g"].id))),
                          [("g", (8, close))] if original else [], difference=True, barrier=not original)
        finally:
            acknowledged = self.close()
        return {"common": self.common, "intentional_differences": self.differences,
                "cleanup_close_acknowledgements": acknowledged}


def wait_port(process, port):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        assert process.poll() is None, f"service exited with {process.returncode}"
        try:
            with socket.create_connection(("127.0.0.1", port), timeout=.1):
                return
        except OSError:
            time.sleep(.025)
    raise TimeoutError(f"port {port} did not become ready")


def isolated(args):
    netns = os.readlink("/proc/self/ns/net")
    assert netns != args.host_netns, "refusing to run reference listeners in the host network namespace"
    records, processes, logs = {}, [], []
    started = time.monotonic()
    library_dir = CACHE / "root/usr/lib/x86_64-linux-gnu"
    # Do not inherit credentials or environment-based operator settings.
    env = {key: value for key, value in os.environ.items()
           if not key.startswith("WAR_") and key not in ("LD_PRELOAD", "LD_LIBRARY_PATH")}
    reference_env = dict(env, LD_LIBRARY_PATH=str(library_dir), OPENSSL_CONF="/dev/null")
    commands = [
        ("cpp", [str(CACHE / "app")], reference_env, 1338),
        ("native", [str(args.relay), "--bind", "127.0.0.1:19138", "--admin-bind", "127.0.0.1:19139",
                    "--workers", "4", "--ping-interval-secs", "0", "--idle-timeout-secs", "0"],
         dict(env, WAR_AUTH_PASSWORD=TEST_PASSWORD.decode()), 19138),
        ("bridge", [str(args.bridge), "--bind", "127.0.0.1:19137", "--upstream", "ws://127.0.0.1:19138"], env, 19137),
    ]
    try:
        for label, command_line, service_env, port in commands:
            log = open(CACHE / f"{label}.log", "wb")
            logs.append(log)
            process = subprocess.Popen(command_line, cwd=ROOT, env=service_env, stdout=log, stderr=log)
            processes.append(process)
            records[label] = {"command": command_line, "sha256": sha(command_line[0])}
            wait_port(process, port)
        paths = {line.split()[-1] for line in Path(f"/proc/{processes[0].pid}/maps").read_text().splitlines()}
        records["cpp"]["loaded_libraries"] = {path: sha(path) for path in sorted(paths)
                                               if path.startswith("/") and ".so" in path}
        results = {label: Suite(port, label, started).run() for label, port in (("cpp", 1338), ("rust", 19137))}
        assert results["cpp"]["common"] == results["rust"]["common"], "unexpected wire behavior difference"
        assert all(process.poll() is None for process in processes), "a service exited during comparison"
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
        deadline = time.monotonic() + 5
        gauges = {"war_sessions": 0, "war_connections": 0, "war_queued_bytes": 0,
                  "war_variable_bytes": 0, "war_channels": 1}
        while True:
            with opener.open("http://127.0.0.1:19139/metrics", timeout=1) as response:
                metrics = {name: int(value) for line in response.read().decode().splitlines()
                           if line.startswith("war_") for name, value in [line.split()]}
            if all(metrics[name] == value for name, value in gauges.items()):
                break
            assert time.monotonic() < deadline, metrics
            time.sleep(.025)
        records["rust_final_metrics"] = metrics
        for name in ("war_write_errors_total", "war_write_timeouts_total",
                     "war_close_failures_total", "war_transport_disconnects_total"):
            assert metrics[name] == 0, (name, metrics[name])
        records["results"] = results
        records["passed"] = True
        records["common_cases"] = len(results["cpp"]["common"])
        records["elapsed_seconds"] = time.monotonic() - started
    finally:
        for process in reversed(processes):
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        for log in logs:
            log.close()
        for (label, *_), process in zip(commands, processes):
            records[label]["exit_code"] = process.returncode
    assert records["native"]["exit_code"] == records["bridge"]["exit_code"] == 0, "Rust service shutdown failed"
    records["isolation"] = {"host_netns": args.host_netns, "test_netns": netns,
                            "root_mount": "read-only", "writable_path": str(CACHE)}
    (CACHE / "result.json").write_text(json.dumps(records, indent=2) + "\n")
    print(f"Passed {records['common_cases']} common cases and "
          f"{len(results['cpp']['intentional_differences'])} explicit differences on both runtimes.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--relay", type=Path, default=ROOT / "target/release/war-relay")
    parser.add_argument("--bridge", type=Path, default=ROOT / "target/release/war-legacy-bridge")
    parser.add_argument("--output", type=Path, default=CACHE / "report.json")
    parser.add_argument("--isolated", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--host-netns", help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.relay, args.bridge = args.relay.resolve(), args.bridge.resolve()
    if args.isolated:
        isolated(args)
        return
    for executable in ("bwrap", "ar"):
        if not shutil.which(executable):
            parser.error(f"{executable} is required")
    for binary in (args.relay, args.bridge):
        if not binary.is_file():
            parser.error(f"build the Rust binaries first: {binary}")
    if platform.system() != "Linux" or platform.machine() != "x86_64":
        parser.error("the pinned upstream executable requires Linux/x86-64")
    packages = prepare()
    (CACHE / "result.json").unlink(missing_ok=True)
    command_line = ["bwrap", "--unshare-net", "--unshare-pid", "--die-with-parent", "--ro-bind", "/", "/",
                    "--bind", str(CACHE), str(CACHE), "--proc", "/proc", "--dev", "/dev", "--",
                    sys.executable, str(Path(__file__).resolve()), "--isolated", "--host-netns",
                    os.readlink("/proc/self/ns/net"), "--relay", str(args.relay), "--bridge", str(args.bridge)]
    subprocess.run(command_line, check=True)
    report = json.loads((CACHE / "result.json").read_text())
    report.update({"timestamp_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
                   "upstream_commit": UPSTREAM, "packages": packages,
                   "harness_sha256": sha(__file__), "python": platform.python_version(),
                   "host": platform.platform(),
                   "method": "independent raw WebSocket client; exact payload/order per recipient; only random IDs and channel-list ordering normalized; legacy bridge, not native CBOR wire parity"})
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(args.output)


if __name__ == "__main__":
    main()
