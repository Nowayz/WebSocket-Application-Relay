#!/usr/bin/env python3
"""Sample bounded fragment assembly and repeated reclamation beside healthy routes."""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import selectors
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("war_benchmark", ROOT / "scripts/benchmark.py")
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)


def run(args):
    port, admin = runtime.free_port(), runtime.free_port()
    env = {k: v for k, v in os.environ.items() if not k.startswith("WAR_")}
    server_command = [str(args.server.resolve()), "--bind", f"127.0.0.1:{port}",
                      "--admin-bind", f"127.0.0.1:{admin}", "--workers", "4",
                      "--messages-per-second", "0", "--bytes-per-second", "0",
                      "--ping-interval-secs", "0", "--idle-timeout-secs", "0",
                      "--max-message-bytes", str(args.max_message_bytes),
                      "--queue-bytes", str(max(32*1024*1024, args.max_message_bytes+10)),
                      "--total-read-buffer-bytes", str(args.staging_bytes),
                      "--read-buffer-idle-secs", str(args.reclaim_secs),
                      "--total-queue-bytes", str(max(256*1024*1024, 2*(args.clients+1)*(args.message_bytes+10)))]
    client_command = [str(args.client.resolve()), "--address", f"127.0.0.1:{port}",
                      "--clients", str(args.clients), "--cycles", str(args.cycles),
                      "--message-bytes", str(args.message_bytes), "--split-bytes", str(args.split_bytes),
                      "--hold-ms", str(args.hold_ms), "--cooldown-secs", str(args.cooldown_secs),
                      "--expect-completed", str(args.expect_completed), "--expect-rejected", str(args.expect_rejected)]
    client = None
    with tempfile.TemporaryFile() as log:
        server = subprocess.Popen(server_command, env=env, stdout=log, stderr=log)
        try:
            for _ in range(200):
                if server.poll() is not None:
                    log.seek(0)
                    raise RuntimeError(log.read().decode())
                try:
                    runtime.metrics(admin)
                    break
                except OSError:
                    time.sleep(.02)
            else:
                raise TimeoutError("server startup")
            baseline = runtime.snapshot(server.pid)
            client = subprocess.Popen(client_command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, env=env)
            samples, events, pending, phase = [], [], b"", ("startup", None)
            started = time.monotonic()
            with selectors.DefaultSelector() as selector:
                selector.register(client.stdout, selectors.EVENT_READ)

                def consume(wait=0):
                    nonlocal pending, phase
                    while selector.select(wait):
                        data = os.read(client.stdout.fileno(), 65536)
                        if not data:
                            selector.unregister(client.stdout)
                            break
                        pending += data
                        while b"\n" in pending:
                            line, pending = pending.split(b"\n", 1)
                            value = json.loads(line)
                            events.append(value)
                            phase = (value["event"], value.get("wave"))
                        wait = 0

                while client.poll() is None:
                    if time.monotonic()-started > args.cycles*(args.cooldown_secs+args.hold_ms/1000+10)+20:
                        raise TimeoutError("fragment-wave deadline")
                    consume(.05)
                    before = phase
                    observed, process = runtime.metrics(admin), runtime.snapshot(server.pid)
                    consume()
                    stable = before == phase
                    samples.append({"seconds":time.monotonic()-started,
                                    "phase":phase[0] if stable else "transition", "wave":phase[1] if stable else None,
                                    "phase_before":before,"phase_after":phase,
                                    "metrics":observed,"process":process})
            stdout, stderr = client.communicate(timeout=2)
            events.extend(json.loads(line) for line in (pending+stdout).splitlines() if line)
            for _ in range(200):
                final = runtime.metrics(admin)
                clean = all(int(final[k]) == 0 for k in ("war_connections","war_sessions","war_queued_bytes","war_variable_bytes","war_read_buffer_reserved_bytes")) and int(final["war_channels"]) == 1
                if clean:
                    break
                time.sleep(.02)
            result = next((e for e in events if e["event"] == "result"), None)
            counts = result is not None and (
                int(final["war_incoming_messages_total"]) == result["expected_incoming_messages"]
                and int(final["war_incoming_bytes_total"]) == result["expected_incoming_bytes"]
                and int(final["war_joined_total"]) == result["expected_joins"]
                and int(final["war_read_buffer_reservations_rejected_total"]) == len(result["rejected_codes"]))
            errors = all(int(final[k]) == 0 for k in ("war_protocol_errors_total","war_write_errors_total","war_write_timeouts_total","war_close_failures_total","war_slow_consumers_total","war_global_pressure_total","war_transport_disconnects_total","war_handshake_errors_total"))
            healthy = result is not None and result["healthy_verified"] > 0
            limit = int(final["war_read_buffer_limit_bytes"])
            bounded = not limit or (int(final["war_read_buffer_peak_reserved_bytes"]) <= limit
                and all(int(s["metrics"]["war_read_buffer_reserved_bytes"]) <= limit for s in samples))
            settled = []
            reclaimed = True
            large_input = args.message_bytes > 65536
            for wave in range(args.cycles):
                held = [s for s in samples if s["phase"] == "settled" and s["wave"] == wave]
                event = next((e for e in events if e["event"] == "settled" and e["wave"] == wave), None)
                last = held[-1] if held else None
                # Only assert automatic resets when the client waited beyond
                # the configured deadline. Disabled/no-wait cases still record
                # high-water retention without pretending it was reclaimed.
                expected = 8192*(event["active"]+1) if event else None
                required = bool(limit and large_input and args.reclaim_secs and args.cooldown_secs > args.reclaim_secs)
                ok = not required or (last is not None and int(last["metrics"]["war_read_buffer_reserved_bytes"]) == expected)
                reclaimed = reclaimed and ok
                settled.append({"wave":wave,"expected_base_bytes":expected,"reclaim_required":required,
                                "observed_reserved_bytes":int(last["metrics"]["war_read_buffer_reserved_bytes"]) if last else None,
                                "sample_count":len(held),"passed":ok})
            resets_expected = None
            if not large_input or not args.reclaim_secs:
                resets_expected = 0
            elif args.cooldown_secs > args.reclaim_secs and result is not None:
                resets_expected = result["completed"]
            resets_match = resets_expected is None or int(final["war_read_buffer_reclaims_total"]) == resets_expected
            return {"server_command":server_command,"client_command":client_command,
                    "server_sha256":hashlib.sha256(args.server.read_bytes()).hexdigest(),
                    "client_sha256":hashlib.sha256(args.client.read_bytes()).hexdigest(),
                    "baseline":baseline,"samples":samples,"events":events,"final_metrics":final,
                    "stderr":stderr.decode(),"exit_code":client.returncode,"cleaned_up":clean,
                    "counts_match":counts,"error_free":errors,"healthy_verified":healthy,
                    "reservation_bound_verified":bounded,"settled_waves":settled,"reclamation_verified":reclaimed,
                    "expected_cache_resets":resets_expected,"cache_resets_match":resets_match,
                    "peak_sampled_rss_kib":max([baseline["VmRSS"]]+[s["process"]["VmRSS"] for s in samples]),
                    "passed":client.returncode == 0 and clean and counts and errors and healthy and bounded and reclaimed and resets_match}
        finally:
            if client is not None and client.poll() is None:
                client.kill()
                client.communicate()
            server.terminate()
            try:
                server.wait(timeout=12)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT/"target/release/war-relay")
    parser.add_argument("--client", type=Path, default=ROOT/"target/release/examples/fragment_waves")
    parser.add_argument("--clients", type=int, default=8)
    parser.add_argument("--cycles", type=int, default=3)
    parser.add_argument("--message-bytes", type=int, default=4*1024*1024)
    parser.add_argument("--max-message-bytes", type=int, help="Server ceiling; defaults to the actual message size")
    parser.add_argument("--split-bytes", type=int)
    parser.add_argument("--hold-ms", type=int, default=250)
    parser.add_argument("--cooldown-secs", type=int, default=2)
    parser.add_argument("--reclaim-secs", type=int, default=1)
    parser.add_argument("--staging-bytes", type=int, default=256*1024*1024)
    parser.add_argument("--expect-completed", type=int)
    parser.add_argument("--expect-rejected", type=int, default=0)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.max_message_bytes = args.max_message_bytes if args.max_message_bytes is not None else args.message_bytes
    args.split_bytes = args.split_bytes if args.split_bytes is not None else args.message_bytes*3//4
    args.expect_completed = args.expect_completed if args.expect_completed is not None else args.clients*args.cycles
    if not (1 <= args.clients <= 64 and 1 <= args.cycles <= 8 and 65536 <= args.message_bytes <= 8*1024*1024
            and args.message_bytes <= args.max_message_bytes <= 64*1024*1024
            and 0 < args.split_bytes < args.message_bytes and 50 <= args.hold_ms <= 2000
            and 0 <= args.reclaim_secs <= 10 and 0 <= args.cooldown_secs <= 10
            and 0 <= args.expect_completed <= args.clients*args.cycles and 0 <= args.expect_rejected <= args.clients):
        parser.error("invalid workload bounds; see --help")
    # Every first fragment must be admitted before the shared final-fragment
    # transition. This probe targets assembly growth, not an incomplete frame.
    first_frame = max(4096, args.split_bytes + (4096 if args.split_bytes > 4096 else 0))
    first_needed = args.clients*2*(first_frame+max(8,args.split_bytes))+8192
    if args.staging_bytes and args.staging_bytes < max(first_needed, args.clients*8192+40960):
        parser.error(f"staging allowance must fit every first fragment: at least {first_needed} bytes")
    record = {"host":platform.platform(),"revision":subprocess.check_output(["git","rev-parse","HEAD"],cwd=ROOT,text=True).strip(),
              "notes":"Fresh four-worker plaintext loopback server; rates, heartbeats and application idle disabled. Every source self-routes one definite-CBOR message per wave in two masked frames, checks an interleaved Pong after assembly of the first fragment, and verifies its returned encoded value byte for byte or the expected 1013 refusal. Independent 1 KiB healthy self echoes continue throughout. Cooldown allows configured read-cache reclamation; a fresh join/route and normal Close follow each wave. Phase sampling brackets both event and wave number. Server-only RSS is distinct from staging allowance. Refused decoder Close is best effort; normal surviving sessions verify every Close handshake. No production-capacity claim.",
              "case":run(args)}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(record,indent=2)+"\n")
    c = record["case"]
    print(json.dumps({"passed":c["passed"],"peak_reserved_bytes":c["final_metrics"]["war_read_buffer_peak_reserved_bytes"],
                      "peak_rss_kib":c["peak_sampled_rss_kib"],"counts_match":c["counts_match"],"settled":c["settled_waves"]}))
    if not c["passed"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
