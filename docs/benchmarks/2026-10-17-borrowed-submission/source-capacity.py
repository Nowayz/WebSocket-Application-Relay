#!/usr/bin/env python3
"""Verify a real connected population, paced private traffic and complete cleanup.

Build --release --bins --example capacity. Runs one fresh server at a time.
The source-IP pool avoids the single-client ephemeral-port ceiling. No sysctls
or machine-wide descriptor limits are changed. Gate scans are outside traffic.
"""
import argparse
import datetime
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
import capacity_pressure

ROOT = runtime.ROOT
TOKEN = "capacity-benchmark"  # Only the disposable local server.
ERRORS = ["war_accept_errors_total", "war_setup_errors_total", "war_handshake_errors_total",
          "war_origin_rejected_total", "war_join_rejected_total", "war_rejected_total",
          "war_protocol_errors_total", "war_rate_limited_total", "war_write_timeouts_total",
          "war_write_errors_total", "war_close_failures_total", "war_aborted_writes_total",
          "war_transport_disconnects_total", "war_slow_consumers_total", "war_global_pressure_total",
          "war_read_buffer_reservations_rejected_total", "war_idle_timeouts_total"]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def pin_child_workers(pid, count, native, apply=True):
    """Pin or verify this fresh child's known I/O threads within its allowed mask."""
    allowed = sorted(os.sched_getaffinity(pid))
    if len(allowed) < count:
        raise ValueError("one CPU per worker requires at least as many allowed CPUs as workers")
    workers = []
    for task in Path(f"/proc/{pid}/task").iterdir():
        name = (task / "comm").read_text().strip()
        if (native and name.startswith("war-io-")) or (not native and name == "tokio-rt-worker"):
            workers.append((int(task.name), name))
    workers.sort(key=lambda item: int(item[1].removeprefix("war-io-")) if native else item[0])
    if len(workers) != count:
        raise ValueError(f"expected {count} child I/O threads, found {len(workers)}")
    if native and [name for _,name in workers] != [f"war-io-{i}" for i in range(count)]:
        raise ValueError("native worker names do not match the configured worker set")
    records = []
    for (tid,name),cpu in zip(workers,allowed):
        before = sorted(os.sched_getaffinity(tid))
        if apply:
            os.sched_setaffinity(tid, {cpu})
        assert os.sched_getaffinity(tid) == {cpu}, "thread affinity was not applied"
        records.append({"pid":pid,"tid":tid,"name":name,"before":before,"cpu":cpu,"method":"harness" if apply else "server_startup"})
    return records


def metrics(port):
    request = urllib.request.Request(f"http://127.0.0.1:{port}/metrics",
                                     headers={"Authorization": f"Bearer {TOKEN}"})
    with urllib.request.urlopen(request, timeout=5) as response:
        return {key: int(value) for key, value in
                (row.split() for row in response.read().decode().splitlines() if row.startswith("war_"))}


def machine():
    return {"socket_memory_global": Path("/proc/net/sockstat").read_text(),
            "memory_kib": {key.rstrip(":"): int(value.split()[0]) for key, value in
                           (row.split(":", 1) for row in Path("/proc/meminfo").read_text().splitlines())
                           if key in ("MemAvailable", "MemFree", "Slab", "SReclaimable", "SUnreclaim")}}


def process(pid):
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
            sockets += os.readlink(fd).startswith("socket:[")
        except FileNotFoundError:
            pass
    snapshot["socket_fd_count"] = sockets
    return snapshot


def established(port):
    count = 0
    with open("/proc/net/tcp") as tcp:
        next(tcp)
        for row in tcp:
            columns = row.split()
            # Source-bound clients may themselves receive the listener's port.
            # Count the listener's address too, rather than both directions.
            address, local_port = columns[1].split(":")
            if address == "0100007F" and int(local_port, 16) == port and columns[3] == "01":
                count += 1
    return count


def check(observed, event, args, pressure=None):
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
                "war_routing_reused_bytes_total": counts["value_bytes"],
                "war_routing_copied_bytes_total": 0,
                "war_queued_bytes": 0, "war_variable_bytes": 0}
    expected.update({name: 0 for name in ERRORS})
    if pressure is not None:
        capacity_pressure.adapt_checks(expected, observed, counts, pressure, event)
    for key, value in expected.items():
        assert observed[key] == value, (event["event"], key, observed[key], value)
    allowance = 0 if done else (args.users * 28 if args.read_storage == "completion" else args.workers * 8192)
    assert 0 <= observed["war_read_buffer_reserved_bytes"] <= allowance
    event["read_allowance_bound"] = allowance
    event["exact_counter_checks"] = expected


def run(args):
    assert resource.getrlimit(resource.RLIMIT_NOFILE)[0] >= args.users + 128, "descriptor limit too low"
    port, admin = runtime.free_port(), runtime.free_port()
    while port == admin:
        admin = runtime.free_port()
    environment = {k: v for k, v in os.environ.items() if not k.startswith("WAR_")}
    environment.update(WAR_ADMIN_TOKEN=TOKEN)
    server_environment = runtime.preload_environment(environment, args.server_preload)
    extra = 10 if args.pressure_client else 0
    server_command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}",
                      "--admin-bind", f"127.0.0.1:{admin}", "--workers", str(args.workers),
                      "--max-connections", str(args.users + max(args.connection_headroom, args.churn_workers * args.room_size + extra)),
                      "--max-channels", str(args.users // args.room_size + args.churn_workers + 1 + bool(extra)),
                      "--max-message-bytes", "65536", "--queue-bytes", "262144",
                      "--total-read-buffer-bytes", "268435456",
                      "--messages-per-second", "0", "--bytes-per-second", "0",
                      "--ping-interval-secs", str(args.ping_interval_secs), "--disable-global-broadcast"]
    if args.pin_workers:
        server_command.append("--pin-workers")
    if args.tcp_send_buffer_bytes:
        server_command += ["--tcp-send-buffer-bytes", str(args.tcp_send_buffer_bytes)]
    if args.tcp_receive_buffer_bytes:
        server_command += ["--tcp-receive-buffer-bytes", str(args.tcp_receive_buffer_bytes)]
    client_command = [str(args.client.resolve()), "--url", f"ws://127.0.0.1:{port}"]
    for name in ("users", "room_size", "concurrency", "source_addresses", "seconds",
                 "payload_bytes", "active_percent", "client_workers", "messages_per_second", "churn_workers", "waves"):
        client_command += ["--" + name.replace("_", "-"), str(getattr(args, name))]
    if args.user_latency_output:
        client_command += ["--user-latency-output", str(args.user_latency_output.resolve())]
    if args.server_cpus:
        server_command = ["taskset", "-c", args.server_cpus] + server_command
    if args.client_cpus:
        client_command = ["taskset", "-c", args.client_cpus] + client_command
    result = {"timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "host": platform.platform(), "server_command": server_command, "client_command": client_command,
              "server_sha256": digest(args.server), "client_sha256": digest(args.client),
              "client_source_sha256": digest(ROOT / "examples/capacity.rs"), "runner_sha256": digest(__file__),
              "runner_sources_sha256": {name: digest(ROOT / name) for name in
                  ("scripts/capacity.py", "scripts/capacity_pressure.py", "scripts/benchmark.py")},
              "source_attribution": "Source snapshot; binary association requires the accompanying build record.",
              "fd_limit": resource.getrlimit(resource.RLIMIT_NOFILE),
              "ephemeral_ports": Path("/proc/sys/net/ipv4/ip_local_port_range").read_text().strip(),
              "profile_diagnostic": args.profile,
              "args": {k: str(v) if isinstance(v, Path) else v for k, v in vars(args).items()},
              "events": [], "management_samples": [], "thread_affinity": [], "passed": False,
              "scope": "Loopback, no TLS. Real TCP/WebSocket/CBOR users in small rooms. All live welcome IDs unique; every stable user routes a complete independently checked private ring message before measurement. Selected rooms send configurable payloads at staggered per-user deadlines. Optional additional rooms complete paced join/ring/Close/EOF cycles while the stable population remains connected. Inactive sockets answer heartbeat Pings. Full sender/body/sequence and exact counters checked. Round-trip includes client sending and verification; logarithmic quantiles are upper bounds. Scheduling lag and churn lifecycle measured separately. Authenticated management is polled during traffic. Every Close acknowledgement, EOF and room departure is checked. Process/descriptor/TCP scans are quiescent gates outside traffic. Global socket/slab measurements include both clients and unrelated machine activity; server RSS excludes kernel socket memory. Heartbeat policy and bounded message/queue/staging limits recorded in server_command."}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile() as logs, tempfile.TemporaryFile() as client_errors:
        server = subprocess.Popen(server_command, env=server_environment, stdout=logs, stderr=logs)
        client = None
        pressure = pressure_result = None
        perf = perf_log = None
        try:
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
            client = subprocess.Popen(client_command, env=environment, stdin=subprocess.PIPE,
                                      stdout=subprocess.PIPE, stderr=client_errors, text=True)
            events = []
            monitoring = False
            next_sample = 0
            pressure_start = None
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
                        assert args.users <= current["war_sessions"] <= args.users + args.churn_workers * args.room_size + extra, (
                            "live sessions", current["war_sessions"], args.users,
                            args.users + args.churn_workers * args.room_size + extra)
                        assert current["war_queued_bytes"] <= 256 * 1024**2
                        assert current["war_read_buffer_reserved_bytes"] <= 256 * 1024**2
                        allowed = capacity_pressure.EXPECTED_ERRORS if args.pressure_client else set()
                        assert all(current[name] == 0 for name in ERRORS if name not in allowed)
                        assert all(0 <= current[name] <= 8 for name in allowed)
                        result["management_samples"].append({"seconds":began-started,
                            "response_ms":(time.monotonic()-began)*1000,"sessions":current["war_sessions"],
                            "connections":current["war_connections"],"queued_bytes":current["war_queued_bytes"],
                            "read_reserved_bytes":current["war_read_buffer_reserved_bytes"]})
                        next_sample = time.monotonic() + .25
                    if args.pressure_client and pressure is None and pressure_start is not None and time.monotonic() >= pressure_start:
                        cmd = capacity_pressure.command(args.pressure_client, f"ws://127.0.0.1:{port}", args.client_cpus)
                        result["pressure_command"] = cmd
                        result["pressure_client_sha256"] = digest(args.pressure_client)
                        pressure = subprocess.Popen(cmd, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
                    continue
                row = client.stdout.readline()
                if not row:
                    raise RuntimeError("capacity client ended before done")
                event = json.loads(row)
                if event["event"] == "measured":
                    for pinned in result["thread_affinity"]:
                        assert os.sched_getaffinity(pinned["tid"]) == {pinned["cpu"]}, "thread affinity changed during measurement"
                        pinned["verified_after_measurement"] = True
                if event["event"] == "measured" and args.pressure_client:
                    assert pressure is not None, "pressure sidecar never started"
                    pressure_result = capacity_pressure.finish(pressure)
                    result["pressure"] = pressure_result
                if event["event"] == "measured" and perf is not None:
                    perf.send_signal(signal.SIGINT)
                    result["perf_exit_code"] = perf.wait(timeout=15)
                    assert result["perf_exit_code"] in (0, -signal.SIGINT)
                    perf_log.seek(0)
                    result["perf_log"] = perf_log.read().decode(errors="replace")
                    perf_log.close()
                    perf = perf_log = None
                # The final socket may release its admission permit immediately after EOF.
                deadline = time.monotonic() + 5
                while True:
                    observed = metrics(admin)
                    try:
                        check(observed, event, args, pressure_result)
                        break
                    except AssertionError:
                        if time.monotonic() >= deadline:
                            raise
                        time.sleep(.02)
                event.update(metrics=observed, server=process(server.pid), client=process(client.pid),
                             machine=machine(), seconds=time.monotonic() - started,
                             established_server_sockets=established(port))
                expected_population = 0 if event["event"] == "done" else args.users
                result["events"].append(event)
                assert event["established_server_sockets"] == expected_population, (
                    event["event"], event["established_server_sockets"], expected_population)
                assert event["server"]["socket_fd_count"] >= expected_population
                assert event["client"]["socket_fd_count"] >= expected_population
                if event["event"] == "done":
                    assert event["server"]["fd_count"] <= result["baseline"]["server"]["fd_count"] + 2
                    connected = result["events"][-5]["client"]["fd_count"]
                    assert event["client"]["fd_count"] <= connected - args.users + 2
                events.append(event["event"])
                print(json.dumps({"event":event["event"],"sessions":observed["war_sessions"],
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
                    path = args.output.with_suffix(".perf.data")
                    assert not path.exists(), "use a fresh profile path"
                    perf_log = tempfile.TemporaryFile()
                    if args.profile_scheduling:
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
                    perf = subprocess.Popen(command, stdout=perf_log, stderr=perf_log)
                    time.sleep(.2)
                    assert perf.poll() is None, "perf could not attach"
                    result["profile_path"] = str(path)
                if event["event"] == "ready":
                    result["measurement_continue_monotonic_seconds"] = time.monotonic()
                client.stdin.write("continue\n")
                client.stdin.flush()
                monitoring = event["event"] == "ready"
                if monitoring and args.pressure_client:
                    pressure_start = time.monotonic() + 1
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
            for child in (perf, pressure, client, server):
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
            logs.seek(0)
            client_errors.seek(0)
            result["server_log"] = logs.read().decode(errors="replace")[-12000:]
            result["client_log"] = client_errors.read().decode(errors="replace")[-12000:]
            args.output.write_text(json.dumps(result, indent=2) + "\n")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/war-relay")
    parser.add_argument("--client", type=Path, default=ROOT / "target/release/examples/capacity")
    parser.add_argument("--server-preload", type=runtime.preload_library,
                        help="Preload one shared library into the server only; record its hash and verify it is mapped")
    parser.add_argument("--users", type=int, default=100_000)
    parser.add_argument("--room-size", type=int, default=16)
    parser.add_argument("--concurrency", type=int, default=64)
    parser.add_argument("--source-addresses", type=int, default=64)
    parser.add_argument("--seconds", type=int, default=10)
    parser.add_argument("--waves", type=int, default=1)
    parser.add_argument("--payload-bytes", type=int, default=1024)
    parser.add_argument("--active-percent", type=int, default=100)
    parser.add_argument("--client-workers", type=int, default=8)
    parser.add_argument("--messages-per-second", type=int, default=1)
    parser.add_argument("--user-latency-output", type=Path, help="Diagnostic per-user latency CSV; requires an instrumented client")
    parser.add_argument("--churn-workers", type=int, default=0)
    parser.add_argument("--ping-interval-secs", type=int, default=0)
    parser.add_argument("--tcp-send-buffer-bytes", type=int, default=0)
    parser.add_argument("--tcp-receive-buffer-bytes", type=int, default=0)
    parser.add_argument("--connection-headroom", type=int, default=0)
    parser.add_argument("--pin-workers", action="store_true", help="Request and verify native --pin-workers startup policy")
    parser.add_argument("--pin-server-workers", action="store_true", help="Benchmark intervention: one allowed CPU per native worker")
    parser.add_argument("--pin-client-workers", action="store_true", help="Benchmark intervention: one allowed CPU per client I/O worker")
    parser.add_argument("--profile", action="store_true", help="Separate diagnostic perf capture; not an ordinary timing trial")
    parser.add_argument("--profile-client", action="store_true",
                        help="Include client cycle samples in --profile; report processes separately")
    parser.add_argument("--profile-frequency", type=int, default=499,
                        help="Cycle sample frequency for --profile (default 499 Hz)")
    parser.add_argument("--profile-scheduling", action="store_true",
                        help="With --profile, capture server/client context-switch sideband; diagnostic only")
    parser.add_argument("--profile-kernel", action="store_true",
                        help="Include kernel cycles in --profile; requires suitable perf permissions")
    parser.add_argument("--pressure-client", type=Path, help="Verified stress client: eight unread recipients beside the stable population")
    parser.add_argument("--workers", type=int, default=4)
    parser.add_argument("--read-storage", choices=["worker", "completion"], default="worker",
                        help="quiescent staging contract; completion rents one 14-byte header with 2x allowance per live user")
    parser.add_argument("--server-cpus", default="0-3")
    parser.add_argument("--client-cpus", default="4-11")
    parser.add_argument("--timeout", type=int, default=240)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not 2 <= args.room_size <= 64 or not 2 <= args.users <= 200_000 or args.users % args.room_size:
        parser.error("users must be in 2..=200000 and divisible by room-size in 2..=64")
    if not 1 <= args.waves <= 16 or not 1 <= args.seconds <= 3600:
        parser.error("waves must be in 1..=16 and seconds in 1..=3600")
    if not 16 <= args.payload_bytes <= 65520:
        parser.error("payload-bytes must be in 16..=65520, allowing envelope headroom")
    if args.pin_workers and args.pin_server_workers:
        parser.error("choose native --pin-workers or the harness-only --pin-server-workers intervention")
    if args.profile and args.waves != 1:
        parser.error("use a single wave for an isolated diagnostic profile")
    if args.profile_scheduling and (not args.profile or args.profile_kernel):
        parser.error("--profile-scheduling requires --profile and excludes --profile-kernel")
    if args.profile_client and (not args.profile or args.profile_scheduling):
        parser.error("--profile-client requires --profile and excludes scheduling sideband")
    if args.profile_frequency <= 0:
        parser.error("profile-frequency must be positive")
    if args.profile_kernel and not args.profile:
        parser.error("--profile-kernel requires --profile")
    if args.pressure_client and (args.waves != 1 or args.seconds < 25 or args.profile):
        parser.error("pressure requires one ordinary wave lasting at least 25 seconds")
    if args.connection_headroom < 0:
        parser.error("connection-headroom must be nonnegative")
    if any(value != 0 and not 4096 <= value <= 16*1024**2
           for value in (args.tcp_send_buffer_bytes, args.tcp_receive_buffer_bytes)):
        parser.error("TCP buffer requests must be zero or in 4096..=16777216")
    run(args)
