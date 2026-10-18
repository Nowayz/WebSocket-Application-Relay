#!/usr/bin/env python3
"""Measure concurrent inspection, verified private traffic and connection churn.

Build --release --bins --example inspection first. Phases are gated to separate
population setup/teardown from the mixed workload. Profiles are diagnostic only.
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
import urllib.request

import benchmark as runtime

ROOT = runtime.ROOT
TOKEN = "inspection-benchmark"  # Disposable fresh loopback process only.
ERRORS = ["war_handshake_errors_total", "war_origin_rejected_total", "war_join_rejected_total",
          "war_rejected_total", "war_protocol_errors_total", "war_rate_limited_total",
          "war_write_timeouts_total", "war_write_errors_total", "war_close_failures_total",
          "war_aborted_writes_total", "war_transport_disconnects_total", "war_slow_consumers_total",
          "war_global_pressure_total", "war_read_buffer_reservations_rejected_total", "war_idle_timeouts_total"]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def dump_heap(pid, prefix):
    """Capture a quiescent gate with gperftools' documented dump signal."""
    # Current gperftools includes the PID in its prefix; older builds do not.
    before = set(prefix.parent.glob(prefix.name + "*.heap"))
    os.kill(pid, signal.SIGUSR2)
    deadline, previous = time.monotonic() + 5, None
    while time.monotonic() < deadline:
        created = set(prefix.parent.glob(prefix.name + "*.heap")) - before
        if len(created) == 1:
            path = created.pop()
            data = path.read_bytes()
            if data == previous and b"MAPPED_LIBRARIES:" in data and data.endswith(b"\n"):
                return {"path": str(path), "sha256": digest(path),
                        "header": data.splitlines()[0].decode()}
            previous = data
        time.sleep(.025)
    raise TimeoutError("heap dump did not complete at phase gate")


def metrics(port):
    request = urllib.request.Request(f"http://127.0.0.1:{port}/metrics",
                                     headers={"Authorization": f"Bearer {TOKEN}"})
    with urllib.request.urlopen(request, timeout=3) as response:
        return dict((key, int(value)) for key, value in
                    (line.split() for line in response.read().decode().splitlines() if line.startswith("war_")))


def check(event, stable_sessions, stable_channels, read_storage="worker", workers=4, variable_bytes=6):
    observed, counts = event["metrics"], event["details"]["counts"]
    # Older frozen binaries predate the admission error counters.
    assert observed.get("war_accept_errors_total", 0) == 0
    assert observed.get("war_setup_errors_total", 0) == 0
    expected = {"war_accepted_total": counts["joins"], "war_joined_total": counts["joins"],
                "war_disconnected_total": counts["normal_closes"],
                "war_incoming_messages_total": counts["sent_messages"],
                "war_incoming_bytes_total": counts["sent_bytes"],
                "war_deliveries_total": counts["deliveries"],
                "war_delivered_bytes_total": counts["delivered_bytes"],
                "war_routing_reused_bytes_total": counts["value_bytes"],
                "war_routing_copied_bytes_total": 0, "war_queued_bytes": 0}
    done = event["event"] == "done"
    expected.update(war_connections=0 if done else stable_sessions,
                    war_sessions=0 if done else stable_sessions,
                    war_channels=1 if done else stable_channels,
                    war_variable_bytes=0 if done else variable_bytes)
    reserved = observed["war_read_buffer_reserved_bytes"]
    if read_storage == "connection":
        expected["war_read_buffer_reserved_bytes"] = 0 if done else stable_sessions * 8192
    elif read_storage == "completion":
        # This experimental backend rents one maximum WebSocket header (14
        # bytes) per idle receive, with the decoder's 2x growth allowance. All
        # application input is complete at these gates. Large body/fragment
        # leases must have retired; final cleanup still requires exactly zero.
        bound = 0 if done else stable_sessions * 28
        assert 0 <= reserved <= bound, (event["event"], "completion receive allowance", reserved, bound)
        expected["war_read_buffer_reserved_bytes"] = reserved
        event["completion_read_allowance_bound"] = bound
    else:
        # At these quiescent gates all application input is complete. Empty
        # decoders own no receive allocation; each worker may keep one 4KiB
        # scratch slab and its 2x growth allowance. An empty worker releases it.
        bound = 0 if done else workers * 8192
        assert 0 <= reserved <= bound, (event["event"], "worker receive allowance", reserved, bound)
        expected["war_read_buffer_reserved_bytes"] = reserved
        event["worker_read_allowance_bound"] = bound
    expected.update({key: 0 for key in ERRORS})
    for key, value in expected.items():
        assert observed[key] == value, (event["event"], key, observed[key], value)
    event["exact_counter_checks"] = expected


def run(args):
    port, admin = runtime.free_port(), runtime.free_port()
    while admin == port:
        admin = runtime.free_port()
    environment = {key: value for key, value in os.environ.items() if not key.startswith("WAR_")}
    environment.update(WAR_AUTH_PASSWORD=TOKEN, WAR_ADMIN_TOKEN=TOKEN)
    server_environment = environment.copy()
    if args.heap_profile:
        args.heap_profile = args.heap_profile.resolve()
        args.heap_profile.parent.mkdir(parents=True, exist_ok=True)
        assert args.heap_library.is_file(), "gperftools library is missing"
        assert not list(args.heap_profile.parent.glob(args.heap_profile.name + "*.heap")), "use a fresh heap-profile prefix"
        server_environment.update(LD_PRELOAD=str(args.heap_library.resolve()),
                                  HEAPPROFILE=str(args.heap_profile), HEAPPROFILESIGNAL=str(signal.SIGUSR2),
                                  HEAP_PROFILE_ALLOCATION_INTERVAL=str(100 * 1024**3),
                                  HEAP_PROFILE_INUSE_INTERVAL=str(100 * 1024**3))
    server_command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}",
                      "--workers", str(args.workers), "--messages-per-second", "0", "--bytes-per-second", "0", "--ping-interval-secs", "0"]
    if getattr(args, "pin_workers", False):
        server_command.append("--pin-workers")
    command = [str(args.client.resolve()), "--url", f"ws://127.0.0.1:{port}", "--admin", f"127.0.0.1:{admin}",
               "--mode", args.mode, "--rooms", str(args.rooms), "--seconds", str(args.seconds),
               "--inspectors", str(args.inspectors), "--inspection-interval-ms", str(args.inspection_interval_ms),
               "--pairs", str(args.pairs), "--churn-workers", str(args.churn_workers)]
    if getattr(args, "client_workers", None) is not None:
        command += ["--client-workers", str(args.client_workers)]
    if getattr(args, "page_size", None) is not None:
        command += ["--page-size", str(args.page_size)]
    if args.fanout_readers:
        command += ["--fanout-readers", str(args.fanout_readers)]
    if getattr(args, "fanout_global_observers", False):
        command += ["--fanout-global-observers"]
    if getattr(args, "fanout_churn_every", 0):
        command += ["--fanout-churn-every", str(args.fanout_churn_every)]
    if getattr(args, "fanout_window", 16) != 16:
        command += ["--fanout-window", str(args.fanout_window)]
    if getattr(args, "fanout_close_window", 1) != 1:
        command += ["--fanout-close-window", str(args.fanout_close_window)]
    if args.server_cpus:
        server_command = ["taskset", "-c", args.server_cpus] + server_command
    if args.client_cpus:
        command = ["taskset", "-c", args.client_cpus] + command
    adjacent_sources = args.client.resolve().parent
    source_root = getattr(args, "client_source_root", None)
    if source_root is None:
        source_root = adjacent_sources if (adjacent_sources / "examples/inspection.rs").is_file() else ROOT
    result = {"timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "host": platform.platform(), "server_command": server_command, "command": command,
              "server_sha256": digest(args.server), "client_sha256": digest(args.client),
              "client_source_root": str(source_root.resolve()),
              "client_source_attribution": "Source snapshot only; binary/source association requires the caller's build record.",
              "client_source_sha256": digest(source_root / "examples/inspection.rs"), "runner_sha256": digest(__file__),
              "client_sources_sha256": {str(p.relative_to(source_root)): digest(p) for p in
                                       [source_root / "examples/inspection.rs"] + sorted((source_root / "examples/inspection").glob("*.rs"))},
              "args": {key: str(value) if isinstance(value, Path) else value for key, value in vars(args).items()},
              "profile_diagnostic": args.profile or bool(args.heap_profile), "events": [], "thread_affinity": [], "passed": False,
              "scope": "Gated setup/mixed workload/cleanup. Stable single-member rooms include binary and Unicode names, private pairs have distinct rooms, health0 keeps a variable. Inspectors use authenticated HTTP or CBOR on a fixed per-worker interval, skipping missed ticks; every name, member count, field, ordering and complete response is verified. During churn, known churn rooms may appear with one member; quiescent snapshots before and after must contain exactly the stable rooms. Healthy traffic uses 64 cyclic 1KiB templates and 16-message windows; full sender/value/FIFO checked. Churn verifies welcome, exact 8-byte-counter self delivery, normal Close and EOF. All stable peers stay connected until the mixed workload ends; every close and final resource/counter/byte total is checked. Latencies include client-side verification and competing client tasks on separate CPU cores. Native CPU covers all mixed work, not an isolated cost per delivery. Loopback, no TLS, rates/heartbeat disabled, default queues. Event-boundary RSS is not continuous peak or allocated bytes. Profiles excluded from timing medians."}
    if args.heap_profile:
        result["heap_profiler"] = {"library": str(args.heap_library.resolve()),
                                   "library_sha256": digest(args.heap_library),
                                   "environment": {key: value for key, value in server_environment.items()
                                                   if key == "LD_PRELOAD" or key.startswith("HEAP")},
                                   "scope": "Server allocator replaced by gperftools. Requested live allocations only; profile timing and RSS are diagnostic and excluded from ordinary comparisons."}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile() as logs, tempfile.TemporaryFile() as client_errors:
        server = subprocess.Popen(server_command, stdout=logs, stderr=logs, env=server_environment)
        client = perf = None
        perf_log = None
        try:
            started = time.monotonic()
            while True:
                if server.poll() is not None:
                    raise RuntimeError("relay stopped during readiness")
                try:
                    metrics(admin)
                    break
                except OSError:
                    if time.monotonic() - started > 5:
                        raise
                    time.sleep(.02)
            client = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      stderr=client_errors, text=True, env=environment)
            ready = None
            stable_sessions = stable_channels = None
            while True:
                if time.monotonic() - started > args.seconds + 75:
                    raise TimeoutError("inspection workload exceeded outer deadline")
                if not select.select([client.stdout], [], [], .25)[0]:
                    continue
                line = client.stdout.readline()
                if not line:
                    raise RuntimeError("inspection client stopped before final gate")
                event = json.loads(line)
                event.update(metrics=metrics(admin), process=runtime.snapshot(server.pid),
                             client_process=runtime.snapshot(client.pid), seconds=time.monotonic() - started)
                result["events"].append(event)
                if event["event"] == "ready":
                    if getattr(args, "pin_workers", False):
                        result["thread_affinity"].extend(runtime.pin_child_workers(server.pid, args.workers, True, apply=False))
                    if getattr(args, "pin_client_workers", False):
                        result["thread_affinity"].extend(runtime.pin_child_workers(client.pid, args.client_workers, False))
                    stable_sessions, stable_channels = event["details"]["stable_sessions"], event["details"]["stable_channels"]
                    check(event, stable_sessions, stable_channels, args.read_storage, args.workers, 6 if args.pairs else 0)
                    if args.profile:
                        profile = args.output.with_suffix(".perf.data")
                        perf_log = tempfile.TemporaryFile()
                        perf = subprocess.Popen(["perf", "record", "-e", "cycles:u", "-F", "499", "--call-graph", "dwarf,8192",
                                                 "-p", str(server.pid), "-o", str(profile)], stdout=perf_log, stderr=perf_log)
                        time.sleep(.2)
                        if perf.poll() is not None:
                            raise RuntimeError("perf could not attach")
                        result["profile_path"] = str(profile)
                    ready = event
                elif event["event"] == "measured":
                    check(event, stable_sessions, stable_channels, args.read_storage, args.workers, 6 if args.pairs else 0)
                    if perf is not None:
                        perf.send_signal(signal.SIGINT)
                        result["perf_exit_code"] = perf.wait(timeout=10)
                        perf_log.seek(0)
                        result["perf_log"] = perf_log.read().decode(errors="replace")
                        perf_log.close()
                        perf = perf_log = None
                        assert result["perf_exit_code"] in (0, -signal.SIGINT), "perf failed"
                    mixed = dict(event["details"])
                    mixed["server_cpu_seconds"] = event["process"]["cpu_seconds"] - ready["process"]["cpu_seconds"]
                    mixed["server_cpu_seconds_per_wall_second"] = mixed["server_cpu_seconds"] / mixed["elapsed_seconds"]
                    mixed["client_cpu_seconds"] = event["client_process"]["cpu_seconds"] - ready["client_process"]["cpu_seconds"]
                    mixed["client_cpu_seconds_per_wall_second"] = mixed["client_cpu_seconds"] / mixed["elapsed_seconds"]
                    assert (mixed["healthy"]["verified"] > 0) == (args.pairs > 0)
                    assert (mixed["churn"]["verified"] > 0) == (args.churn_workers > 0)
                    assert (mixed["inspection"]["verified"] > 0) == (args.mode != "none" and args.inspectors > 0)
                    assert (mixed.get("fanout", {}).get("verified", 0) > 0) == (args.fanout_readers > 0)
                    assert (mixed.get("fanout_membership", {}).get("verified", 0) > 0) == (getattr(args, "fanout_churn_every", 0) > 0)
                    result["mixed"] = mixed
                    print(json.dumps(mixed), flush=True)
                elif event["event"] == "done":
                    check(event, stable_sessions, stable_channels, args.read_storage, args.workers, 6 if args.pairs else 0)
                    assert event["details"]["passed"]
                    assert event["details"]["counts"]["joins"] == event["details"]["counts"]["normal_closes"]
                else:
                    raise RuntimeError("unknown phase gate")
                for pinned in result["thread_affinity"]:
                    assert os.sched_getaffinity(pinned["tid"]) == {pinned["cpu"]}, "thread affinity changed during workload"
                if args.heap_profile:
                    event["heap"] = dump_heap(server.pid, args.heap_profile)
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
            result["server_forced_kill"] = False
            try:
                result["server_exit_code"] = server.wait(timeout=12)
            except subprocess.TimeoutExpired:
                result["server_forced_kill"] = True
                server.kill()
                result["server_exit_code"] = server.wait()
            logs.seek(0)
            client_errors.seek(0)
            result.update(server_log=logs.read().decode(errors="replace"), client_log=client_errors.read().decode(errors="replace"))
            if result["server_exit_code"] != 0 or result["server_forced_kill"]:
                result["passed"] = False
                result.setdefault("error", "relay did not exit cleanly")
            args.output.write_text(json.dumps(result, indent=2) + "\n")
    return result


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=runtime.SERVER)
    parser.add_argument("--client", type=Path, default=ROOT / "target/release/examples/inspection")
    parser.add_argument("--client-source-root", type=Path,
                        help="Source snapshot root; defaults to sources beside a frozen client, then checkout context")
    parser.add_argument("--mode", choices=["none", "cbor", "http"], default="http")
    parser.add_argument("--page-size", type=int, help="HTTP first-page size during traffic; complete traversal verified at quiescent gates")
    parser.add_argument("--rooms", type=int, default=8192)
    parser.add_argument("--seconds", type=int, default=8)
    parser.add_argument("--inspectors", type=int, default=2)
    parser.add_argument("--inspection-interval-ms", type=int, default=10)
    parser.add_argument("--pairs", type=int, default=2, help="Verified private pairs, up to 64 with the current client")
    parser.add_argument("--client-workers", type=int, choices=range(1, 33),
                        help="Client runtime threads; omitted for compatibility with frozen clients")
    parser.add_argument("--churn-workers", type=int, default=8)
    parser.add_argument("--workers", type=int, default=4, help="Native server worker count")
    parser.add_argument("--fanout-readers", type=int, default=0,
                        help="Add a verified broadcast room; requires the updated client")
    parser.add_argument("--fanout-global-observers", action="store_true",
                        help="Use authenticated global channel-message observers as fanout readers")
    parser.add_argument("--fanout-churn-every", type=int, default=0,
                        help="Join/retire a verifying room member every N fanout windows; zero disables")
    parser.add_argument("--fanout-window", type=int, default=16,
                        help="Broadcast window size in 1..=32; requires updated client when not16")
    parser.add_argument("--fanout-close-window", type=int, choices=range(1, 33), default=1,
                        help="Departure drain batch outside timing; 32 shortens large-audience cleanup")
    parser.add_argument("--read-storage", choices=["worker", "connection", "completion"], default="worker",
                        help="staging contract: worker scratch, frozen async connection, or experimental completion header receives")
    parser.add_argument("--pin-workers", action="store_true",
                        help="Request and verify native --pin-workers startup policy")
    parser.add_argument("--pin-client-workers", action="store_true",
                        help="Pin each client I/O worker to one allowed CPU; requires --client-workers")
    parser.add_argument("--server-cpus", default="0-3")
    parser.add_argument("--client-cpus", default="4-7")
    profiles = parser.add_mutually_exclusive_group()
    profiles.add_argument("--profile", action="store_true")
    profiles.add_argument("--heap-profile", type=Path, help="diagnostic only: gperftools heap prefix; changes allocator, timings/RSS excluded from ordinary medians")
    parser.add_argument("--heap-library", type=Path, default=Path("/usr/lib/libtcmalloc.so"))
    parser.add_argument("--output", type=Path, required=True)
    arguments = parser.parse_args()
    if arguments.pin_client_workers and arguments.client_workers is None:
        parser.error("--pin-client-workers requires --client-workers")
    outcome = run(arguments)
    print(json.dumps({"passed": outcome["passed"], "error": outcome.get("error")}), flush=True)
    raise SystemExit(0 if outcome["passed"] else 1)
