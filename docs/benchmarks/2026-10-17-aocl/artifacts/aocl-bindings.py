#!/usr/bin/env python3
"""Diagnostic only: prove libc/AOCL symbol bindings in the same relay binary."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import benchmark as runtime

BINARY = ROOT / "artifacts/baselines/c10m-sendmsg/war-relay"
CLIENT = ROOT / "artifacts/baselines/capacity-final/stress"
LIBRARY = (ROOT / "artifacts/aocl-libmem-5.3.2-zen5/lib/libaocl-libmem.so").resolve()
output = {"server_sha256": hashlib.sha256(BINARY.read_bytes()).hexdigest(), "diagnostic_only": True, "runs": []}
for label, library in [("glibc", None)] + [(name, (ROOT / f"artifacts/aocl-libmem-5.3.2-{name}/lib/libaocl-libmem.so").resolve()) for name in ("zen5", "dispatch", "avx2")]:
    port, admin = runtime.free_port(), runtime.free_port()
    env = {key: value for key, value in os.environ.items() if not key.startswith("WAR_")}
    server_env = runtime.preload_environment(env, library)
    prefix = ROOT / f"artifacts/aocl-bindings-{label}"
    server_env.update(LD_DEBUG="bindings", LD_DEBUG_OUTPUT=str(prefix))
    command = ["taskset", "-c", "0-3", str(BINARY), "--workers", "4", "--bind", f"127.0.0.1:{port}",
               "--admin-bind", f"127.0.0.1:{admin}", "--messages-per-second", "0", "--bytes-per-second", "0"]
    with (ROOT / f"artifacts/aocl-bindings-{label}-server.log").open("w") as logs:
        server = subprocess.Popen(command, env=server_env, stdout=logs, stderr=logs)
        try:
            deadline = time.monotonic() + 10
            while True:
                assert server.poll() is None, "server exited"
                try:
                    runtime.metrics(admin)
                    break
                except OSError:
                    assert time.monotonic() < deadline, "readiness timeout"
                    time.sleep(.02)
            preload = runtime.preload_metadata(library, server.pid)
            process = subprocess.Popen(["taskset", "-c", "4-7", str(CLIENT), "--url", f"ws://127.0.0.1:{port}",
                                     "--mode", "traffic", "--clients", "8", "--window", "4",
                                     "--duration-secs", "1", "--payload-bytes", "65520"],
                                    env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
            time.sleep(.1)
            client_maps = Path(f"/proc/{process.pid}/maps").read_text()
            assert "libaocl-libmem" not in client_maps
            stdout, stderr = process.communicate(timeout=30)
            assert process.returncode == 0, stderr
        finally:
            server.terminate()
            server.wait(timeout=12)
        assert server.returncode == 0
    debug_path = Path(f"{prefix}.{server.pid}")
    symbols = ("memcpy", "memmove", "memset", "memcmp", "bcmp", "strlen", "memchr")
    bindings = [line.strip() for line in debug_path.read_text().splitlines()
                if str(BINARY) in line and any(f"symbol `{symbol}'" in line for symbol in symbols)]
    assert any("symbol `memcpy'" in line for line in bindings), bindings
    if library:
        assert any("libaocl-libmem" in line and "symbol `memcpy'" in line for line in bindings)
    output["runs"].append({"label": label, "server_command": command, "server_preload": preload,
                           "bindings": bindings, "client_output": [json.loads(line) for line in stdout.splitlines()],
                           "server_exit_code": server.returncode, "client_aocl_mapping_absent": True, "debug_path": str(debug_path)})
(ROOT / "artifacts/aocl-bindings.json").write_text(json.dumps(output, indent=2) + "\n")
print(json.dumps({"diagnostic_passed": True, "bindings": {row["label"]: row["bindings"] for row in output["runs"]}}, indent=2))
