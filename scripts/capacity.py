#!/usr/bin/env python3
"""Verify a real connected population, paced private traffic and complete cleanup.

Build --release --bins --example capacity. Runs one fresh server at a time.
No sysctls or machine-wide descriptor limits are changed. Gate scans are
outside traffic.
"""
import benchmark_transport
import argparse
import datetime
import errno
import hashlib
import json
import os
from pathlib import Path
import platform
import resource
import select
import signal
import subprocess
import tempfile
import time
import urllib.request

import benchmark as runtime
import socket_resources

ROOT = runtime.ROOT
TOKEN = "capacity-benchmark"  # Only the disposable local server.
ERRORS = ["war_handshake_errors_total", "war_origin_rejected_total", "war_rejected_total",
          "war_write_timeouts_total", "war_write_errors_total",
          "war_slow_consumers_total", "war_global_pressure_total",
          "war_read_buffer_reservations_rejected_total", "war_idle_timeouts_total"]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def resident_kib(pid):
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith("VmRSS:"):
            return int(line.split()[1])
    raise RuntimeError("missing resident-memory observation")


def cpu_stat_snapshot():
    rows = {}
    for line in Path("/proc/stat").read_text().splitlines():
        fields = line.split()
        if fields and fields[0].startswith("cpu"):
            rows[fields[0]] = [int(value) for value in fields[1:]]
    return {"ticks_per_second": os.sysconf("SC_CLK_TCK"),
            "monotonic_seconds": time.monotonic(), "cpus": rows}


# Preserve the existing tooling import while sharing placement with inspection.
pin_child_workers = runtime.pin_child_workers

def metrics(port):
    request = urllib.request.Request(f"http://127.0.0.1:{port}/metrics",
                                     headers={"Authorization": f"Bearer {TOKEN}"})
    with urllib.request.urlopen(request, timeout=5) as response:
        return {key: int(value) for key, value in
                (row.split() for row in response.read().decode().splitlines() if row.startswith("war_"))}


def udp_counters():
    rows = Path("/proc/net/snmp").read_text().splitlines()
    for headers, values in zip(rows, rows[1:]):
        if headers.startswith("Udp:") and values.startswith("Udp:"):
            return dict(zip(headers.split()[1:], map(int, values.split()[1:])))
    return {}


def machine():
    return {"socket_memory_global": Path("/proc/net/sockstat").read_text(),
            "memory_kib": {key.rstrip(":"): int(value.split()[0]) for key, value in
                           (row.split(":", 1) for row in Path("/proc/meminfo").read_text().splitlines())
                           if key in ("MemAvailable", "MemFree", "Slab", "SReclaimable", "SUnreclaim")}}


def process(pid, socket_inodes=None):
    snapshot = runtime.snapshot(pid)
    stat = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    ticks = os.sysconf("SC_CLK_TCK")
    snapshot["user_cpu_seconds"] = int(stat[11]) / ticks
    snapshot["system_cpu_seconds"] = int(stat[12]) / ticks
    descriptors = list(Path(f"/proc/{pid}/fd").iterdir())
    snapshot["fd_count"] = len(descriptors)
    sockets = 0
    for fd in descriptors:
        try:
            target = os.readlink(fd)
            if target.startswith("socket:["):
                sockets += 1
                if socket_inodes is not None:
                    socket_inodes.add(target[8:-1])
        except FileNotFoundError:
            pass
    snapshot["socket_fd_count"] = sockets
    return snapshot


def udp_census(owned_inodes):
    total = {"owned_sockets": 0, "receive_queue_bytes": 0, "send_queue_bytes": 0, "receive_drops": 0}
    for name in ("udp", "udp6"):
        for row in Path("/proc/net/" + name).read_text().splitlines()[1:]:
            columns = row.split()
            if columns[9] not in owned_inodes:
                continue
            sent, received = (int(value, 16) for value in columns[4].split(":"))
            total["owned_sockets"] += 1
            total["receive_queue_bytes"] += received
            total["send_queue_bytes"] += sent
            total["receive_drops"] += int(columns[-1])
    return total


def check(observed, event, args):
    counts = event["counts"]
    done = event["event"] == "done"
    expected = {"war_connections": 0 if done else args.users,
                "war_sessions": 0 if done else args.users,
                "war_channels": 1 if done else args.users // args.room_size + 1,
                "war_accepted_total": counts["joins"], "war_joined_total": counts["joins"],
                "war_disconnected_total": counts["normal_closes"],
                "war_incoming_messages_total": counts["sent_messages"],
                "war_incoming_bytes_total": counts["sent_bytes"],
                "war_deliveries_total": counts["deliveries"],
                "war_delivered_bytes_total": counts["delivered_bytes"],
                "war_quic_connections": 0 if done else args.users,
                # Every record is complete at a gate, so no staging lease remains.
                "war_read_buffer_reserved_bytes": 0,
                "war_queued_bytes": 0, "war_variable_bytes": 0}
    expected.update({name: 0 for name in ERRORS})
    for key, value in expected.items():
        assert observed[key] == value, (event["event"], key, observed[key], value)
    event["exact_counter_checks"] = expected


def _preflight_outputs(args):
    paths = [args.output]
    if args.user_latency_output:
        csv = args.user_latency_output
        # Match Rust Path::with_extension, including an empty final extension.
        name = csv.name
        if not name or name == '..':
            raise argparse.ArgumentError(None, 'user-latency-output must name a file')
        stem, dot, _ = name.rpartition('.')
        timeline = csv.with_name((stem if dot and stem else name) + '.windows.json')
        paths += [csv, timeline]
    if args.user_latency_histogram_output:
        paths.append(args.user_latency_histogram_output)
    if args.profile:
        paths.append(args.output.with_suffix('.perf-stat.csv' if args.profile_counters else '.perf.data'))
    seen = {}
    for path in paths:
        identity = path.resolve()
        if identity in seen:
            raise argparse.ArgumentError(None, f'output paths overlap: {seen[identity]} and {path}')
        seen[identity] = path
        if os.path.lexists(path):
            raise FileExistsError(errno.EEXIST, 'output already exists', str(path))


def run(args):
    _preflight_outputs(args)
    kernel_resources = getattr(args, "kernel_socket_resources", False)
    slab_file = getattr(args, "kernel_slab_sizes", None)
    if slab_file and not kernel_resources:
        raise ValueError("kernel-slab-sizes requires kernel-socket-resources")
    slab_sizes = json.loads(slab_file.read_text()) if slab_file else None
    port, admin = runtime.free_port(), runtime.free_port()
    while port == admin:
        admin = runtime.free_port()
    environment = {k: v for k, v in os.environ.items() if not k.startswith("WAR_")}
    environment.update(WAR_ADMIN_TOKEN=TOKEN)
    server_environment = runtime.preload_environment(environment, args.server_preload)
    server_command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}",
                      "--admin-bind", f"127.0.0.1:{admin}", "--workers", str(args.workers),
                      "--max-message-bytes", "65536", "--queue-bytes", "262144",
                      "--total-read-buffer-bytes", "268435456",
                      "--ping-interval-secs", str(args.ping_interval_secs), "--disable-global-broadcast"]
    if args.pin_workers:
        server_command.append("--pin-workers")
    data_url = benchmark_transport.prepare(args, server_command, server_environment, port)
    client_command = [str(args.client.resolve()), "--url", data_url]
    for name in ("users", "room_size", "concurrency", "seconds", "payload_bytes", "active_percent",
                 "client_workers", "messages_per_second", "churn_workers", "waves", "udp_receive_buffer_bytes"):
        client_command += ["--" + name.replace("_", "-"), str(getattr(args, name))]
    if args.user_latency_output:
        client_command += ["--user-latency-output", str(args.user_latency_output.resolve())]
    if args.user_latency_histogram_output:
        client_command += ["--user-latency-histogram-output", str(args.user_latency_histogram_output.resolve())]
    if args.server_cpus:
        server_command = ["taskset", "-c", args.server_cpus] + server_command
    if args.client_cpus:
        client_command = ["taskset", "-c", args.client_cpus] + client_command
    result = {"timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "host": platform.platform(), "server_command": server_command, "client_command": client_command,
              "server_sha256": digest(args.server), "client_sha256": digest(args.client),
              "client_source_sha256": digest(ROOT / "examples/capacity.rs"), "runner_sha256": digest(__file__),
              "runner_sources_sha256": {name: digest(ROOT / name) for name in
                  ("scripts/capacity.py", "scripts/socket_resources.py", "scripts/benchmark.py", "scripts/benchmark_transport.py", "examples/support/socket.rs")},
              "source_attribution": "Source snapshot; binary association requires the accompanying build record.",
              "fd_limit": resource.getrlimit(resource.RLIMIT_NOFILE),
              "profile_diagnostic": args.profile or kernel_resources,
              "args": {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()},
              "events": [], "management_samples": [], "thread_affinity": [], "passed": False,
              "scope": "Loopback WebTransport with a pinned development certificate; each session departs with an empty record, its acknowledgement and a QUIC close. Real connected CBOR users in small rooms. All live welcome IDs unique; every stable user routes a complete independently checked private ring message before measurement. Selected rooms send configurable payloads at staggered per-user deadlines. Optional additional rooms complete paced join/ring/close cycles while the stable population remains connected. Full sender/body/sequence and exact counters checked. Round-trip includes client sending and verification; logarithmic quantiles are upper bounds. Scheduling lag and churn lifecycle measured separately. Authenticated management is polled during traffic. Every session close and room departure is checked. Process/descriptor and independent QUIC-connection scans are quiescent gates outside traffic. Global socket/slab measurements include both clients and unrelated machine activity; server RSS excludes kernel socket memory. QUIC keep-alive policy and bounded message/queue/staging limits recorded in server_command."}
    if kernel_resources:
        result["kernel_resource_diagnostic"] = {
            "scope": "Per-process SOCK_DIAG snapshots at quiescent phase gates. Diagnostic scan overhead excludes this run from ordinary timing comparisons. Kernel counters and slab object estimates are separate from RSS and are not a complete kernel allocation total.",
            "slab_metadata_sha256": digest(slab_file) if slab_file else None,
        }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with args.output.open("x") as receipt, tempfile.TemporaryFile() as logs, tempfile.TemporaryFile() as client_errors:
        server = client = None
        perf = perf_log = None
        try:
            server = subprocess.Popen(server_command, env=server_environment, stdout=logs, stderr=logs)
            started = time.monotonic()
            while True:
                if server.poll() is not None:
                    raise RuntimeError("server stopped before readiness")
                try:
                    observed = metrics(admin)
                    break
                except OSError:
                    if time.monotonic() - started > 10:
                        raise
                    time.sleep(.02)
            result["baseline"] = {"server": process(server.pid), "machine": machine(), "metrics": observed}
            result["server_preload"] = runtime.preload_metadata(args.server_preload, server.pid)
            client_command += benchmark_transport.certificate(logs)
            client = subprocess.Popen(client_command, env=environment, stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=client_errors, text=True)
            events = []
            monitoring = False
            next_sample = 0
            while True:
                if time.monotonic() - started > (args.seconds + args.timeout) * args.waves:
                    raise TimeoutError("capacity workload exceeded outer deadline")
                if not select.select([client.stdout], [], [], .25)[0]:
                    if server.poll() is not None:
                        raise RuntimeError("relay exited during workload")
                    if monitoring and time.monotonic() >= next_sample:
                        began = time.monotonic()
                        current = metrics(admin)
                        # Preserve the full numeric snapshot before a live
                        # assertion so a failed trial can identify its cause.
                        result["last_monitoring_metrics"] = current
                        assert args.users <= current["war_sessions"] <= args.users + args.churn_workers * args.room_size, (
                            "live sessions", current["war_sessions"], args.users,
                            args.users + args.churn_workers * args.room_size)
                        assert current["war_queued_bytes"] <= 256 * 1024**2
                        assert current["war_read_buffer_reserved_bytes"] <= 256 * 1024**2
                        assert all(current[name] == 0 for name in ERRORS)
                        result["management_samples"].append({"seconds":began-started,
                            "response_ms":(time.monotonic()-began)*1000,"sessions":current["war_sessions"],
                            "connections":current["war_connections"],"queued_bytes":current["war_queued_bytes"],
                            "read_reserved_bytes":current["war_read_buffer_reserved_bytes"],
                            "server_rss_kib":resident_kib(server.pid), "client_rss_kib":resident_kib(client.pid)})
                        next_sample = time.monotonic() + .25
                    continue
                row = client.stdout.readline()
                if not row:
                    raise RuntimeError("capacity client ended before done")
                event = json.loads(row)
                if event["event"] == "measured":
                    for pinned in result["thread_affinity"]:
                        assert os.sched_getaffinity(pinned["tid"]) == {pinned["cpu"]}, "thread affinity changed during measurement"
                        pinned["verified_after_measurement"] = True
                if event["event"] == "measured" and perf is not None:
                    perf.send_signal(signal.SIGINT)
                    result["perf_exit_code"] = perf.wait(timeout=15)
                    assert result["perf_exit_code"] in (0, -signal.SIGINT)
                    if args.profile_counters:
                        result["counter_cpu_end"] = cpu_stat_snapshot()
                    perf_log.seek(0)
                    result["perf_log"] = perf_log.read().decode(errors="replace")
                    perf_log.close()
                    perf = perf_log = None
                # The final session may release its admission permit just after its close.
                deadline = time.monotonic() + 5
                while True:
                    observed = metrics(admin)
                    try:
                        check(observed, event, args)
                        break
                    except AssertionError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(.02)
                server_inodes = set()
                client_inodes = set()
                event.update(metrics=observed, server_pid=server.pid, client_pid=client.pid,
                             server=process(server.pid, server_inodes), client=process(client.pid, client_inodes),
                             machine=machine(), udp_counters=udp_counters(), seconds=time.monotonic() - started)
                event["udp_server_census"] = udp_census(server_inodes)
                event["udp_client_census"] = udp_census(client_inodes)
                if kernel_resources:
                    event["kernel_sockets"] = {
                        "server": socket_resources.snapshot(server.pid, local_port=port, sizes=slab_sizes),
                        "client": socket_resources.snapshot(client.pid, sizes=slab_sizes),
                    }
                    assert event["kernel_sockets"]["server"]["owned_sockets"] == event["udp_server_census"]["owned_sockets"]
                    for snapshot in event["kernel_sockets"].values():
                        assert snapshot["missing_skmem_sockets"] == 0, "owned socket memory accounting unavailable"
                del server_inodes, client_inodes  # Release gate-only storage before traffic resumes.
                result["events"].append(event)
                # QUIC multiplexes every session over the endpoint's UDP socket(s).
                assert event["server"]["socket_fd_count"] >= 1
                assert event["client"]["socket_fd_count"] >= 1
                if event["event"] == "done":
                    assert event["server"]["fd_count"] <= result["baseline"]["server"]["fd_count"] + 2
                    connected = result["events"][-5]["client"]["fd_count"]
                    assert event["client"]["fd_count"] <= connected + 2
                events.append(event["event"])
                print(json.dumps({"event":event["event"],"server_pid":server.pid,"client_pid":client.pid,"sessions":observed["war_sessions"],
                                  "rss_mib":event["server"]["VmRSS"]/1024,
                                  "details":event["details"]}), flush=True)
                if event["event"] == "ready":
                    if args.pin_workers:
                        result["thread_affinity"].extend(pin_child_workers(server.pid, args.workers, True, apply=False))
                    if args.pin_server_workers:
                        result["thread_affinity"].extend(pin_child_workers(server.pid, args.workers, True))
                    if args.pin_client_workers:
                        result["thread_affinity"].extend(pin_child_workers(client.pid, args.client_workers, False))
                if event["event"] == "ready" and args.profile:
                    path = args.output.with_suffix(".perf-stat.csv" if args.profile_counters else ".perf.data")
                    assert not os.path.lexists(path), "use a fresh profile path"
                    perf_log = tempfile.TemporaryFile()
                    if args.profile_counters:
                        result["profile_pids"] = {"server": server.pid}
                        if args.profile_client:
                            result["profile_pids"]["client"] = client.pid
                        threads = []
                        for role, pid in result["profile_pids"].items():
                            for task in sorted(Path(f"/proc/{pid}/task").iterdir()):
                                threads.append({"role": role, "pid": pid, "tid": int(task.name),
                                                "name": (task / "comm").read_text().strip()})
                        result["profile_threads"] = threads
                        command = ["perf", "stat", "--no-inherit", "--per-thread", "-x", ";",
                            "-e", "task-clock,cycles,instructions,cache-misses,context-switches,cpu-migrations",
                            "--tid", ",".join(str(t["tid"]) for t in threads), "-o", str(path)]
                        result["profile_scope"] = "Diagnostic counters for ready-gate threads only, kernel and user included; unsupported/unmeasured events are not zero. Timings excluded from ordinary comparisons."
                    elif args.profile_scheduling:
                        # Sideband context-switch records cover both processes;
                        # low-rate clock samples identify threads without DWARF.
                        command = ["perf", "record", "-e", "cpu-clock", "-F", "49",
                            "--switch-events", "--sample-cpu", "--clockid", "mono", "-p",
                            f"{server.pid},{client.pid}", "-o", str(path)]
                        result["profile_scope"] = "Diagnostic context-switch sideband for server and client. Off-CPU includes voluntary I/O waits; not run-queue latency."
                        result["profile_pids"] = {"server": server.pid, "client": client.pid}
                    else:
                        event_name = "cycles" if args.profile_kernel else "cycles:u"
                        command = ["perf", "record", "-e", event_name, "-F", str(args.profile_frequency),
                            "--call-graph", "dwarf,8192", "-p",
                            f"{server.pid},{client.pid}" if args.profile_client else str(server.pid),
                            "-o", str(path)]
                        result["profile_pids"] = {"server": server.pid}
                        if args.profile_client:
                            result["profile_pids"]["client"] = client.pid
                        result["profile_scope"] = "Diagnostic cycle samples; per-process reports required; timings excluded from ordinary comparisons."
                    result["profile_command"] = command
                    if args.profile_counters:
                        result["counter_cpu_masks"] = {role: sorted(os.sched_getaffinity(pid))
                            for role, pid in result["profile_pids"].items()}
                        result["counter_cpu_start"] = cpu_stat_snapshot()
                    perf = subprocess.Popen(command, stdout=perf_log, stderr=perf_log)
                    time.sleep(.2)
                    assert perf.poll() is None, "perf could not attach"
                    result["profile_path"] = str(path)
                if event["event"] == "ready":
                    result["measurement_continue_monotonic_seconds"] = time.monotonic()
                client.stdin.write("continue\n")
                client.stdin.flush()
                monitoring = event["event"] == "ready"
                if event["event"] == "done" and event["wave"] == args.waves:
                    break
            assert events == ["connected", "ready", "measured", "postcheck", "done"] * args.waves, events
            result["client_exit_code"] = client.wait(timeout=10)
            assert result["client_exit_code"] == 0
            server.send_signal(signal.SIGTERM)
            result["server_exit_code"] = server.wait(timeout=15)
            assert result["server_exit_code"] == 0
            result["measurements"] = []
            for wave in range(args.waves):
                ready, measured = result["events"][wave*5+1:wave*5+3]
                wall = measured["seconds"] - ready["seconds"]
                result["measurements"].append({"wave":wave+1,"gate_wall_seconds": wall,
                "server_cpu_seconds": measured["server"]["cpu_seconds"] - ready["server"]["cpu_seconds"],
                "client_cpu_seconds": measured["client"]["cpu_seconds"] - ready["client"]["cpu_seconds"],
                "application_deliveries": measured["details"]["counts"]["routing_checks"],
                "planned_deliveries_per_second": measured["details"]["active_users"] * args.messages_per_second,
                "stable_deliveries_per_second": measured["details"]["round_trip"]["samples"] / measured["details"]["elapsed_seconds"],
                "server_user_cpu_seconds": measured["server"]["user_cpu_seconds"] - ready["server"]["user_cpu_seconds"],
                "server_system_cpu_seconds": measured["server"]["system_cpu_seconds"] - ready["server"]["system_cpu_seconds"],
                "round_trip": measured["details"]["round_trip"],
                "schedule_lag": measured["details"]["schedule_lag"]})
            result["measurement"] = result["measurements"][0]
            result["passed"] = True
        except Exception as error:
            result["error"] = repr(error)
            raise
        finally:
            for child in (perf, client, server):
                if child is not None and child.poll() is None:
                    child.terminate()
                    try:
                        child.wait(timeout=15)
                    except subprocess.TimeoutExpired:
                        child.kill()
                        child.wait(timeout=5)
                        result.setdefault("forced_kill_pids", []).append(child.pid)
            if perf_log is not None:
                perf_log.close()
            result["server_exit_code"] = server.returncode if server is not None else None
            result["client_exit_code"] = client.returncode if client is not None else None
            logs.seek(0)
            client_errors.seek(0)
            result["server_log"] = logs.read().decode(errors="replace")[-12000:]
            result["client_log"] = client_errors.read().decode(errors="replace")[-12000:]
            receipt.write(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--udp-receive-buffer-bytes", type=int, default=4 * 1024 * 1024)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/war-relay")
    parser.add_argument("--client", type=Path, default=ROOT / "target/release/examples/capacity")
    parser.add_argument("--server-preload", type=runtime.preload_library,
                        help="Preload one shared library into the server only; record its hash and verify it is mapped")
    parser.add_argument("--users", type=int, default=100_000)
    parser.add_argument("--room-size", type=int, default=16)
    parser.add_argument("--concurrency", type=int, default=64)
    parser.add_argument("--seconds", type=int, default=10)
    parser.add_argument("--waves", type=int, default=1)
    parser.add_argument("--payload-bytes", type=int, default=1024)
    parser.add_argument("--active-percent", type=int, default=100)
    parser.add_argument("--client-workers", type=int, default=8)
    parser.add_argument("--messages-per-second", type=int, default=1)
    parser.add_argument("--user-latency-output", type=Path, help="Exact per-user latency CSV; requires an instrumented client")
    parser.add_argument("--user-latency-histogram-output", type=Path,
                        help="Bounded per-user histogram CSV; percentile columns are bounds, not exact samples")
    parser.add_argument("--churn-workers", type=int, default=0)
    parser.add_argument("--ping-interval-secs", type=int, default=0, help="Relay QUIC keep-alive interval; zero disables")
    parser.add_argument("--pin-workers", action="store_true", help="Request and verify native --pin-workers startup policy")
    parser.add_argument("--pin-server-workers", action="store_true", help="Benchmark intervention: one allowed CPU per native worker")
    parser.add_argument("--pin-client-workers", action="store_true", help="Benchmark intervention: one allowed CPU per client I/O worker")
    parser.add_argument("--profile", action="store_true", help="Separate diagnostic perf capture; not an ordinary timing trial")
    parser.add_argument("--kernel-socket-resources", action="store_true",
                        help="Separate diagnostic SOCK_DIAG snapshots at phase gates; excluded from ordinary timing comparisons")
    parser.add_argument("--kernel-slab-sizes", type=Path,
                        help="Optional read-only slab-size receipt from socket_resources.py --slab-sizes-only")
    parser.add_argument("--profile-counters", action="store_true",
                        help="With --profile, collect per-thread perf stat instead of stack samples")
    parser.add_argument("--profile-client", action="store_true",
                        help="Include client cycle samples in --profile; report processes separately")
    parser.add_argument("--profile-frequency", type=int, default=499,
                        help="Cycle sample frequency for --profile (default 499 Hz)")
    parser.add_argument("--profile-scheduling", action="store_true",
                        help="With --profile, capture server/client context-switch sideband; diagnostic only")
    parser.add_argument("--profile-kernel", action="store_true",
                        help="Include kernel cycles in --profile; requires suitable perf permissions")
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--server-cpus", default="0-3")
    parser.add_argument("--client-cpus", default="4-11")
    parser.add_argument("--timeout", type=int, default=240)
    parser.add_argument("--output", type=Path, required=True, help="Fresh receipt path; existing files are never overwritten")
    args = parser.parse_args()
    if not 2 <= args.room_size <= 64 or not 2 <= args.users <= 200_000 or args.users % args.room_size:
        parser.error("users must be in 2..=200000 and divisible by room-size in 2..=64")
    if not 1 <= args.waves <= 16 or not 1 <= args.seconds <= 3600:
        parser.error("waves must be in 1..=16 and seconds in 1..=3600")
    if not 16 <= args.payload_bytes <= 65520:
        parser.error("payload-bytes must be in 16..=65520, allowing envelope headroom")
    if args.user_latency_histogram_output and (args.user_latency_output or args.waves != 1):
        parser.error("histogram output requires one wave and excludes exact sample output")
    if args.pin_workers and args.pin_server_workers:
        parser.error("choose native --pin-workers or the harness-only --pin-server-workers intervention")
    if args.profile and args.waves != 1:
        parser.error("use a single wave for an isolated diagnostic profile")
    if args.profile_scheduling and (not args.profile or args.profile_kernel):
        parser.error("--profile-scheduling requires --profile and excludes --profile-kernel")
    if args.profile_counters and (not args.profile or args.profile_scheduling or args.profile_kernel):
        parser.error("--profile-counters requires --profile and excludes scheduling/kernel sample flags")
    if args.profile_client and (not args.profile or args.profile_scheduling):
        parser.error("--profile-client requires --profile and excludes scheduling sideband")
    if args.profile_frequency <= 0:
        parser.error("profile-frequency must be positive")
    if args.profile_kernel and not args.profile:
        parser.error("--profile-kernel requires --profile")
    try:
        run(args)
    except FileExistsError as error:
        parser.error(f"output already exists; choose a fresh artifact path: {error.filename}")
    except argparse.ArgumentError as error:
        parser.error(str(error))
