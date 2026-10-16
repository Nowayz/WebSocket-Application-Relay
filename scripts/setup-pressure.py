#!/usr/bin/env python3
"""Verify native socket setup failures stay local and retain admission until close.

Linux-only child-scoped LD_PRELOAD fault injection; no system settings change.
Compiles setup-pressure.c in a temporary directory with the configured C compiler.
Ordinary benchmarks never preload this library. Both syscall faults are exercised
with one and four workers, queued-flow recovery, exact CBOR echoes, normal Close,
resource accounting, and shutdown during the setup retry delay.
"""
import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time
import traceback

import benchmark as runtime

spec = importlib.util.spec_from_file_location("accept_pressure", Path(__file__).with_name("accept-pressure.py"))
pressure = importlib.util.module_from_spec(spec)
spec.loader.exec_module(pressure)


def wait_until(action, message):
    deadline = time.monotonic() + 5
    while not action():
        assert time.monotonic() < deadline, message
        time.sleep(.01)


def run_case(args, library, mode, workers):
    record = {"mode": mode, "workers": workers, "waves": [], "passed": False}
    process, management, healthy, failed, pending = None, None, None, None, None
    with tempfile.TemporaryDirectory(prefix="war-setup-gates-") as temporary, tempfile.TemporaryFile() as logs:
        directory = Path(temporary)
        release = directory / "release"
        release.touch()
        try:
            port, admin = runtime.free_port(), runtime.free_port()
            assert port != admin
            command = [str(args.server.resolve()), "--workers", str(workers),
                       "--bind", f"127.0.0.1:{port}", "--admin-bind", f"127.0.0.1:{admin}",
                       "--max-connections-per-ip", "2", "--ping-interval-secs", "0"]
            env = {k: v for k, v in os.environ.items() if not k.startswith("WAR_") and k != "LD_PRELOAD"}
            env.update(LD_PRELOAD=str(library), WAR_PROBE_DIRECTORY=str(directory),
                       WAR_PROBE_PORT=str(port), WAR_PROBE_MODE=mode, RUST_LOG="war_relay=info")
            record["command"] = command
            process = subprocess.Popen(command, env=env, stdout=logs, stderr=logs)
            management = pressure.wait_live(process, lambda: pressure.Management(admin))
            healthy = pressure.Client(pressure.socket.create_connection(("127.0.0.1", port), timeout=3), False)
            initial = management.metrics()
            assert initial["war_connections"] == initial["war_sessions"] == 1
            record["initial_metrics"] = initial
            for wave in range(1, 4):
                (directory / "closing").unlink(missing_ok=True)
                release.unlink()
                (directory / "trigger").touch()
                failed = pressure.socket.create_connection(("127.0.0.1", port), timeout=3)
                failed.settimeout(5)
                wait_until(lambda: (directory / "closing").exists(), "failed socket never reached close")
                held = management.metrics()
                # A real FD remains open behind the close gate. Even a refused
                # socket must keep its physical admission slot until that close.
                row = {"wave": wave, "held_metrics": held,
                       "slot_held_until_descriptor_close": held["war_connections"] == 2}
                record["waves"].append(row)
                release.touch()
                try:
                    assert failed.recv(1) == b"", "failed setup sent unexpected bytes"
                except ConnectionResetError:
                    pass
                failed.close()
                failed = None
                # Other workers may accept immediately; release the failed
                # socket's IP lease before placing the queued recovery flow.
                wait_until(lambda: management.metrics()["war_connections"] == 1,
                           "failed socket did not release physical admission")
                pending = pressure.socket.create_connection(("127.0.0.1", port), timeout=3)
                if wave == 3:
                    # Cancellation must wake a listener still in its one-second
                    # retry delay, while an established peer gets a valid Close.
                    started = time.monotonic()
                    process.send_signal(signal.SIGTERM)
                    opcode, body = healthy.read()
                    assert opcode == 8 and int.from_bytes(body[:2], "big") == 1001
                    # Shutdown flushes its Close and retires TCP without waiting
                    # for an acknowledgement. Sending a late reply would race
                    # that intentional retirement and can induce a TCP reset.
                    assert healthy.stream.recv(1) == b""
                    assert process.wait(timeout=3) == 0
                    row["shutdown_seconds"] = time.monotonic() - started
                    row["shutdown_close"] = 1001
                    break
                assert process.poll() is None, f"setup fault terminated native service: {process.returncode}"
                healthy.echo()
                started = time.monotonic()
                recovered = pressure.Client(pending, False, b"recovered")
                pending = None
                recovered.close()
                healthy.echo()
                row["queued_flow_recovery_seconds"] = time.monotonic() - started

                def cleaned():
                    observed = management.metrics()
                    row["recovered_metrics"] = observed
                    return observed["war_connections"] == observed["war_sessions"] == 1
                wait_until(cleaned, "failed setup retained admission, read allowance or a session")
                metrics = row["recovered_metrics"]
                assert metrics["war_channels"] == 2
                assert metrics["war_accepted_total"] == 1 + wave * 2
                assert metrics["war_rejected_total"] == metrics["war_setup_errors_total"] == wave
                assert metrics["war_joined_total"] == 1 + wave
                assert metrics["war_disconnected_total"] == wave
                assert metrics["war_accept_errors_total"] == metrics["war_ip_rejected_total"] == 0
                for key in ["war_queued_bytes", "war_variable_bytes", "war_output_queue_capacity_bytes",
                            "war_routing_copied_bytes_total", "war_handshake_errors_total"]:
                    assert metrics[key] == 0, (key, metrics[key])
                assert metrics["war_read_buffer_reserved_bytes"] == initial["war_read_buffer_reserved_bytes"]
                assert row["slot_held_until_descriptor_close"], "admission released before physical TCP close"
            assert all(row["slot_held_until_descriptor_close"] for row in record["waves"])
            expected = "EPOLL_CTL_ADD ENOMEM\n" if mode == "register" else "TCP_NODELAY ENOBUFS\n"
            record["injected_calls"] = (directory / "events").read_text().splitlines()
            assert (directory / "events").read_text() == expected * 3
            record["passed"] = True
        except Exception as error:
            record.update(error=repr(error), traceback=traceback.format_exc())
        finally:
            release.touch()
            for stream in [failed, pending, healthy.stream if healthy else None]:
                if stream is not None:
                    stream.close()
            if management is not None:
                management.close()
            if process is not None:
                if process.poll() is None:
                    process.send_signal(signal.SIGTERM)
                try:
                    record["process_exit_code"] = process.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    process.kill()
                    record["process_exit_code"] = process.wait(timeout=5)
                record["passed"] &= record["process_exit_code"] == 0
            if (directory / "events").exists():
                record["injected_calls"] = (directory / "events").read_text().splitlines()
        logs.seek(0)
        record["server_log"] = logs.read().decode(errors="replace")
    return record


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=runtime.SERVER)
    parser.add_argument("--cc", default="cc")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if not Path("/proc/self/fd").is_dir():
        parser.error("setup-pressure requires Linux /proc and a dynamically linked native server")
    source = Path(__file__).with_suffix(".c")
    result = {"scope": __doc__, "source": Path(__file__).read_text(), "shim_source": source.read_text(),
              "dependencies": {name: hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest()
                               for name in ["accept-pressure.py", "benchmark.py"]},
              "server_sha256": hashlib.sha256(args.server.read_bytes()).hexdigest(),
              "cases": [], "passed": False}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="war-setup-probe-") as temporary:
        library = Path(temporary) / "setup-pressure.so"
        command = [args.cc, "-std=c11", "-O2", "-Wall", "-Wextra", "-Werror", "-fPIC", "-shared",
                   str(source), "-o", str(library), "-ldl"]
        subprocess.run(command, check=True, capture_output=True, text=True)
        result["compiler"] = subprocess.check_output([args.cc, "--version"], text=True).splitlines()[0]
        result["compile_command"] = command
        for mode in ["nodelay", "register"]:
            for workers in [1, 4]:
                case = run_case(args, library, mode, workers)
                result["cases"].append(case)
                args.output.write_text(json.dumps(result, indent=2) + "\n")
                print(json.dumps({k: case.get(k) for k in ["mode", "workers", "passed", "process_exit_code", "error"]}), flush=True)
    result["passed"] = all(case["passed"] for case in result["cases"])
    args.output.write_text(json.dumps(result, indent=2) + "\n")
    raise SystemExit(0 if result["passed"] else 1)


if __name__ == "__main__":
    main()
