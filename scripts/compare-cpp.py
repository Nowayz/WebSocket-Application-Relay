#!/usr/bin/env python3
"""Compare original C++ binary and native Rust CBOR using common application work.

Build --release --bins --example inspection first. Each fresh process and client
run in their own private network namespace. No legacy bridge is on the hot path.
Timing runs must not overlap builds, profiles or other load tests.
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
import statistics
import subprocess
import sys
import tempfile
import time

import benchmark as runtime
import inspection
import reference

ROOT = runtime.ROOT
CACHE = reference.CACHE


def sha(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def rebuild_cpp(workers):
    """Apply only explicit worker configuration to a temporary upstream copy."""
    assert 1 <= workers <= 32
    build = CACHE / f"build-{workers}"
    build.mkdir(parents=True, exist_ok=True)
    original = (ROOT / "relay.cpp").read_bytes()
    before = b"std::vector<std::thread *> threads(std::thread::hardware_concurrency());"
    after = f"std::vector<std::thread *> threads({workers});".encode()
    assert original.count(before) == 1, "upstream worker configuration changed"
    source, binary = build / "relay.cpp", build / "app"
    source.write_bytes(original.replace(before, after))
    files = sorted((ROOT / "uws").glob("*.cpp"))
    command = ["clang++", "-O3", "-march=native", "-mrdrnd", "-fomit-frame-pointer", "-std=c++1z",
               "-o", str(binary), str(source), *map(str, files), f"-I{ROOT / 'uws'}",
               "-ltbb", "-lz", "-pthread", "-lssl", "-luv", "-lcrypto"]
    record = {"scope": "Upstream source with only worker-count configuration in a temporary copy. Routing/storage/protocol/uWS code unchanged; upstream build.sh flags, current system libraries.",
              "worker_configuration": {"before": before.decode(), "after": after.decode(), "workers": workers},
              "command": command, "compiler": subprocess.check_output(["clang++", "--version"], text=True),
              "library_versions": subprocess.check_output(["pkg-config", "--modversion", "openssl", "tbb", "libuv", "zlib"], text=True).splitlines(),
              "upstream_sources": {str(p.relative_to(ROOT)): sha(p) for p in
                                   [ROOT / "relay.cpp", ROOT / "build.sh", *sorted((ROOT / "uws").glob("*"))] if p.is_file()},
              "configured_source_sha256": sha(source)}
    with tempfile.TemporaryFile() as log:
        completed = subprocess.run(command, cwd=ROOT, stdout=log, stderr=log)
        log.seek(0)
        record.update(exit_code=completed.returncode, build_log=log.read().decode(errors="replace"))
    assert completed.returncode == 0, record["build_log"]
    record["sha256"] = sha(binary)
    (build / "build.json").write_text(json.dumps(record, indent=2) + "\n")
    return binary, record


def trial(args):
    namespace = os.readlink("/proc/self/ns/net")
    assert namespace != args.host_netns, "refusing reference listeners in host network namespace"
    cpp = args.kind == "cpp"
    port, admin = (1338, 19139) if cpp else (19138, 19139)
    env = {k: v for k, v in os.environ.items()
           if not k.startswith("WAR_") and k not in ("LD_PRELOAD", "LD_LIBRARY_PATH")}
    if cpp:
        server_command = [str(args.cpp_executable)]
        env.update(OPENSSL_CONF="/dev/null")
        if not args.cpp_system_libraries:
            env.update(LD_LIBRARY_PATH=str(CACHE / "root/usr/lib/x86_64-linux-gnu"))
    else:
        server_command = [str(args.relay), "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}",
                          "--workers", str(args.workers), "--messages-per-second", "0", "--bytes-per-second", "0",
                          "--ping-interval-secs", "0", "--idle-timeout-secs", "0"]
        env.update(WAR_AUTH_PASSWORD=inspection.TOKEN, WAR_ADMIN_TOKEN=inspection.TOKEN)
    churn = args.churn_workers if args.case == "churn" else 0
    mode = ("binary" if cpp else "cbor") if args.case == "query" else "none"
    client_command = [str(args.client), "--url", f"ws://127.0.0.1:{port}", "--wire", "cpp" if cpp else "cbor",
                      "--protocol-only", "--mode", mode, "--rooms", str(args.rooms), "--seconds", str(args.seconds),
                      "--pairs", str(args.pairs), "--churn-workers", str(churn), "--inspectors", str(args.inspectors),
                      "--inspection-interval-ms", str(args.inspection_interval_ms)]
    if args.server_cpus:
        server_command = ["taskset", "-c", args.server_cpus] + server_command
    if args.client_cpus:
        client_command = ["taskset", "-c", args.client_cpus] + client_command
    if args.audit_ids:
        client_command.append("--audit-ids")
    record = {"kind": args.kind, "case": args.case, "server_command": server_command,
              "client_command": client_command, "server_sha256": sha(args.cpp_executable if cpp else args.relay),
              "client_sha256": sha(args.client), "runner_sha256": sha(__file__),
              "client_source_sha256": sha(ROOT / "examples/inspection.rs"),
              "isolation": {"host_netns": args.host_netns, "test_netns": namespace,
                            "root_mount": "read-only", "writable_path": str(CACHE)},
              "events": [], "passed": False}
    started = time.monotonic()
    with tempfile.TemporaryFile(dir=CACHE) as logs, tempfile.TemporaryFile(dir=CACHE) as client_errors:
        server = subprocess.Popen(server_command, cwd=ROOT, env=env, stdout=logs, stderr=logs)
        client = None
        try:
            if cpp:
                reference.wait_port(server, port)
            else:
                while True:
                    assert server.poll() is None, "Rust stopped during readiness"
                    try:
                        inspection.metrics(admin)
                        break
                    except OSError:
                        assert time.monotonic() - started < 5, "Rust management not ready"
                        time.sleep(.025)
            if cpp:
                # The unchanged program staggers one loop per hardware thread by
                # 100ms, then creates its collector. Wait for the collector too.
                deadline = started + (args.cpp_loop_count or os.cpu_count()) * .1 + .5
                while time.monotonic() < deadline:
                    assert server.poll() is None, "reference stopped during thread setup"
                    time.sleep(.025)
                # libstdc++ may report affinity-aware hardware concurrency; do
                # not assume Python's host CPU count equals the resulting loops.
                record["startup_server"] = runtime.snapshot(server.pid)
                listeners = [line for file in ("tcp", "tcp6") for line in Path(f"/proc/net/{file}").read_text().splitlines()[1:]
                             if line.split()[1].endswith(f":{port:04X}") and line.split()[3] == "0A"]
                record["listening_sockets"] = len(listeners)
                if args.cpp_loop_count:
                    assert record["startup_server"]["Threads"] == args.cpp_loop_count + 2, "C++ loop count differs"
                    assert len(listeners) == args.cpp_loop_count, "C++ listeners failed to bind"
                paths = {line.split()[-1] for line in Path(f"/proc/{server.pid}/maps").read_text().splitlines()}
                record["loaded_libraries"] = {p: sha(p) for p in sorted(paths) if p.startswith("/") and ".so" in p}
            client = subprocess.Popen(client_command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=client_errors, text=True, env=env)
            ready = None
            while True:
                # The C++ lifetime contract below intentionally measures before
                # its first 30s collection. Retained member counts must be exact.
                assert time.monotonic() - started < (25 if cpp else args.seconds + 45), "case exceeded lifetime bound"
                assert server.poll() is None, "server stopped during workload"
                if not select.select([client.stdout], [], [], .1)[0]:
                    continue
                line = client.stdout.readline()
                assert line, "client exited before final gate"
                event = json.loads(line)
                event.update(server=runtime.snapshot(server.pid), client=runtime.snapshot(client.pid),
                             process_seconds=time.monotonic() - started)
                if not cpp:
                    event["metrics"] = inspection.metrics(admin)
                    if event["event"] == "ready":
                        stable_sessions = event["details"]["stable_sessions"]
                        stable_channels = event["details"]["stable_channels"]
                    inspection.check(event, stable_sessions, stable_channels)
                record["events"].append(event)
                if event["event"] == "ready":
                    ready = event
                elif event["event"] == "measured":
                    assert ready is not None
                    mixed = dict(event["details"])
                    for side in ("server", "client"):
                        mixed[f"{side}_cpu_seconds"] = event[side]["cpu_seconds"] - ready[side]["cpu_seconds"]
                        mixed[f"{side}_cpu_per_wall_second"] = mixed[f"{side}_cpu_seconds"] / mixed["elapsed_seconds"]
                    assert mixed["healthy"]["verified"] > 0
                    assert (mixed["churn"]["verified"] > 0) == (churn > 0)
                    assert (mixed["inspection"]["verified"] > 0) == (args.case == "query")
                    if args.case == "query":
                        wanted = args.seconds * 1000 // args.inspection_interval_ms * args.inspectors
                        assert mixed["inspection"]["verified"] == wanted, "inspection pacing fell behind"
                    record["mixed"] = mixed
                elif event["event"] == "done":
                    details = event["details"]
                    assert details["passed"] and details["counts"]["joins"] == details["counts"]["normal_closes"]
                    assert details["retired_churn_members_verified"] == (record["mixed"]["churn"]["verified"] if cpp else 0)
                else:
                    raise RuntimeError("unknown phase")
                client.stdin.write("continue\n")
                client.stdin.flush()
                if event["event"] == "done":
                    assert client.wait(timeout=5) == 0
                    record["passed"] = True
                    break
        except Exception as error:
            record["error"] = repr(error)
        finally:
            if client is not None:
                if client.poll() is None:
                    client.kill()
                    client.wait()
                record["client_exit_code"] = client.returncode
                client.stdin.close()
                client.stdout.close()
            server.terminate()
            record["server_forced_kill"] = False
            try:
                record["server_exit_code"] = server.wait(timeout=12)
            except subprocess.TimeoutExpired:
                record["server_forced_kill"] = True
                server.kill()
                record["server_exit_code"] = server.wait()
            # C++ has no signal handler. Its expected stop is SIGTERM, whereas
            # Rust must complete its graceful shutdown and return zero.
            if record["server_forced_kill"] or record["server_exit_code"] != (-signal.SIGTERM if cpp else 0):
                record["passed"] = False
                record.setdefault("error", "unexpected server termination")
            logs.seek(0)
            client_errors.seek(0)
            record.update(server_log=logs.read().decode(errors="replace"),
                          client_log=client_errors.read().decode(errors="replace"))
            args.output.write_text(json.dumps(record, indent=2) + "\n")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--relay", type=Path, default=ROOT / "target/release/war-relay")
    parser.add_argument("--client", type=Path, default=ROOT / "target/release/examples/inspection")
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--rooms", type=int, default=8192)
    parser.add_argument("--seconds", type=int, default=8, choices=range(1, 13))
    parser.add_argument("--pairs", type=int, default=2)
    parser.add_argument("--churn-workers", type=int, default=8)
    parser.add_argument("--inspectors", type=int, default=2)
    parser.add_argument("--inspection-interval-ms", type=int, default=10)
    parser.add_argument("--server-cpus", default="0-3")
    parser.add_argument("--client-cpus", default="4-7")
    parser.add_argument("--repetitions", type=int, default=3)
    parser.add_argument("--rebuild-cpp", action="store_true", help="build temporary upstream copy with explicit matching worker count and system libraries")
    parser.add_argument("--audit-ids", action="store_true", help="diagnostic only: check welcome ID reuse before C++ GC")
    parser.add_argument("--cases", nargs="+", choices=["private", "churn", "query"], default=["private", "churn", "query"])
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--isolated", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--host-netns", help=argparse.SUPPRESS)
    parser.add_argument("--kind", choices=["cpp", "rust"], help=argparse.SUPPRESS)
    parser.add_argument("--case", choices=["private", "churn", "query"], help=argparse.SUPPRESS)
    parser.add_argument("--cpp-executable", type=Path, default=CACHE / "app", help=argparse.SUPPRESS)
    parser.add_argument("--cpp-system-libraries", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--cpp-loop-count", type=int, default=0, help=argparse.SUPPRESS)
    args = parser.parse_args()
    args.relay, args.client, args.output = (p.resolve() for p in (args.relay, args.client, args.output))
    args.cpp_executable = args.cpp_executable.resolve()
    if args.isolated:
        result = trial(args)
        print(json.dumps({"kind": args.kind, "case": args.case, "passed": result["passed"], "error": result.get("error")}), flush=True)
        return 0 if result["passed"] else 1
    assert args.repetitions > 0 and len(set(args.cases)) == len(args.cases)
    if args.rebuild_cpp:
        args.cpp_executable, cpp_build = rebuild_cpp(args.workers)
        args.cpp_system_libraries, args.cpp_loop_count = True, args.workers
        packages = []
    else:
        packages = reference.prepare()
        cpp_build = {"scope": "Pinned original shipped ELF; unknown build flags, isolated old libraries. Active listeners/threads are observed, not assumed from source.", "sha256": reference.APP_SHA}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    record = {"timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(), "host": platform.platform(),
              "cpu": subprocess.check_output(["lscpu", "-J"], text=True), "upstream_commit": reference.UPSTREAM,
              "packages": packages, "args": {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()},
              "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "runner_sha256": sha(__file__), "client_source_sha256": sha(ROOT / "examples/inspection.rs"),
              "client_sha256": sha(args.client), "rust_sha256": sha(args.relay), "cpp_sha256": sha(args.cpp_executable), "cpp_build": cpp_build,
              "scope": "Common application work, native Rust CBOR versus C++ binary protocol; no bridge. Same frozen client, 64 cyclic 1027-byte CBOR byte-string values (1024 application bytes), 16-message windows, two private pairs and idle rooms. C++ treats values as opaque; Rust validates CBOR. Each case runs fresh in a private net namespace, sequentially, pinned to the same server CPU set with client cores separate. Worker/library/build configuration is recorded in cpp_build; active listeners/threads checked for rebuilt C++. Case private: only private traffic. Case churn: private traffic plus fixed-room reconnect workers; every welcome, self delivery, Close/EOF checked, and C++ retired member counts verified before its first 30s GC. Case query: private traffic plus authenticated channel inspectors at fixed intervals, no churn; names/counts and full replies verified, sorting normalized only for C++ which does not promise it. HTTP management has no C++ counterpart. Setup and cleanup gated outside timing, every normal close counted, Rust counters/bytes/reservations checked. Latencies include client validation and concurrent client tasks, logarithmic percentile bucket upper bounds. RSS sampled at gates, not continuous peak/allocated memory. Server/client CPU covers all mixed work. Plaintext loopback, no TLS, rates/heartbeats off, default Rust queues; not a language-only or production-capacity comparison.",
              "cases": [], "passed": False}
    for repetition in range(1, args.repetitions + 1):
        offset = (repetition - 1) % len(args.cases)
        for case in args.cases[offset:] + args.cases[:offset]:
            for kind in (["cpp", "rust"] if repetition % 2 else ["rust", "cpp"]):
                path = CACHE / f"bench-{case}-{kind}-{repetition}.json"
                path.unlink(missing_ok=True)
                command = ["bwrap", "--unshare-net", "--unshare-pid", "--die-with-parent", "--ro-bind", "/", "/",
                           "--bind", str(CACHE), str(CACHE), "--proc", "/proc", "--dev", "/dev", "--",
                           sys.executable, str(Path(__file__).resolve()), "--isolated", "--host-netns",
                           os.readlink("/proc/self/ns/net"), "--kind", kind, "--case", case,
                           "--relay", str(args.relay), "--client", str(args.client), "--workers", str(args.workers),
                           "--rooms", str(args.rooms), "--seconds", str(args.seconds), "--pairs", str(args.pairs),
                           "--churn-workers", str(args.churn_workers), "--inspectors", str(args.inspectors),
                           "--inspection-interval-ms", str(args.inspection_interval_ms),
                           "--cpp-executable", str(args.cpp_executable), "--cpp-loop-count", str(args.cpp_loop_count),
                           "--server-cpus", args.server_cpus, "--client-cpus", args.client_cpus, "--output", str(path)]
                if args.cpp_system_libraries:
                    command.append("--cpp-system-libraries")
                if args.audit_ids:
                    command.append("--audit-ids")
                completed = subprocess.run(command, cwd=ROOT)
                detail = json.loads(path.read_text())
                trace = args.output.with_name(f"{args.output.stem}-{case}-{kind}-{repetition}.json")
                trace.write_text(json.dumps(detail, indent=2) + "\n")
                row = {"kind": kind, "case": case, "repetition": repetition, "trace": str(trace.relative_to(ROOT)),
                       "trace_sha256": sha(trace), "passed": detail["passed"]}
                if "mixed" in detail:
                    row.update(mixed=detail["mixed"], sampled_server_rss_kib=[e["server"]["VmRSS"] for e in detail["events"]],
                               server_threads=detail["events"][0]["server"]["Threads"],
                               normal_closes=detail["events"][-1]["details"]["counts"]["normal_closes"])
                record["cases"].append(row)
                args.output.write_text(json.dumps(record, indent=2) + "\n")
                assert completed.returncode == 0 and detail["passed"], detail.get("error")
                assert detail["client_sha256"] == record["client_sha256"]
                assert detail["server_sha256"] == record[f"{kind}_sha256"]
                assert detail["client_source_sha256"] == record["client_source_sha256"]
                assert detail["runner_sha256"] == record["runner_sha256"]
                print(json.dumps(row), flush=True)
    record["medians"] = {}
    for case in args.cases:
        for kind in ("cpp", "rust"):
            rows = [r for r in record["cases"] if r["kind"] == kind and r["case"] == case]
            middle = {activity: {k: statistics.median(r["mixed"][activity][k] for r in rows)
                                for k in ("per_second", "p50_upper_ms", "p99_upper_ms", "max_ms")}
                      for activity in ("healthy", "churn", "inspection")}
            for activity in ("healthy", "churn", "inspection"):
                middle[activity]["throughput_range"] = [min(r["mixed"][activity]["per_second"] for r in rows),
                                                        max(r["mixed"][activity]["per_second"] for r in rows)]
            for side in ("server", "client"):
                middle[f"{side}_cpu_per_wall_second"] = statistics.median(r["mixed"][f"{side}_cpu_per_wall_second"] for r in rows)
            middle["rss_ready_mib"] = statistics.median(r["sampled_server_rss_kib"][0] / 1024 for r in rows)
            middle["rss_measured_mib"] = statistics.median(r["sampled_server_rss_kib"][1] / 1024 for r in rows)
            record["medians"][f"{case}/{kind}"] = middle
    record["passed"] = True
    args.output.write_text(json.dumps(record, indent=2) + "\n")
    print(json.dumps({"passed": True, "medians": record["medians"]}), flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
