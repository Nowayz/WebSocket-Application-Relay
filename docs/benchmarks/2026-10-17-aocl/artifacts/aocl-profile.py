#!/usr/bin/env python3
"""Separate one-worker large-frame kernel/user cycle diagnostic; never a timing trial."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import benchmark as runtime

port, admin = runtime.free_port(), runtime.free_port()
binary = ROOT / "artifacts/baselines/c10m-sendmsg/war-relay"
client = ROOT / "artifacts/baselines/capacity-final/stress"
output = ROOT / "artifacts/aocl-large-profile.perf.data"
assert not output.exists()
command = ["taskset", "-c", "0", str(binary), "--workers", "1", "--bind", f"127.0.0.1:{port}",
           "--admin-bind", f"127.0.0.1:{admin}", "--messages-per-second", "0", "--bytes-per-second", "0",
           "--max-message-bytes", "1048576", "--queue-bytes", "33554432"]
env = {key:value for key,value in os.environ.items() if not key.startswith("WAR_")}
assert not env.get("LD_PRELOAD")
with (ROOT / "artifacts/aocl-large-profile-server.log").open("w") as logs, (ROOT / "artifacts/aocl-large-profile-perf.log").open("w") as perf_log:
    server = subprocess.Popen(command, env=env, stdout=logs, stderr=logs)
    perf = None
    try:
        deadline=time.monotonic()+10
        while True:
            assert server.poll() is None
            try:
                runtime.metrics(admin)
                break
            except OSError:
                assert time.monotonic()<deadline
                time.sleep(.02)
        perf_command=["perf", "record", "-e", "cycles", "-F", "499", "--call-graph", "dwarf,8192",
                      "-p", str(server.pid), "-o", str(output)]
        perf=subprocess.Popen(perf_command, stdout=perf_log, stderr=perf_log)
        time.sleep(.2)
        assert perf.poll() is None
        client_command=["taskset", "-c", "4-7", str(client), "--url", f"ws://127.0.0.1:{port}", "--mode", "traffic",
                        "--clients", "6", "--duration-secs", "10", "--payload-bytes", "1048544", "--window", "4"]
        run=subprocess.run(client_command, env=env, capture_output=True, text=True, check=True, timeout=45)
        deadline=time.monotonic()+5
        while True:
            metrics=runtime.metrics(admin)
            if all(int(metrics[key])==0 for key in ("war_connections","war_sessions","war_queued_bytes","war_read_buffer_reserved_bytes")):
                break
            assert time.monotonic()<deadline
            time.sleep(.02)
    finally:
        if perf is not None:
            perf.send_signal(signal.SIGINT)
            perf.wait(timeout=15)
        server.terminate()
        server.wait(timeout=15)
    print(json.dumps({"server_exit_code":server.returncode,"perf_exit_code":perf.returncode}),flush=True)
    assert server.returncode==0 and perf.returncode in (0, -signal.SIGINT)
events=[json.loads(line) for line in run.stdout.splitlines()]
result=next(entry for entry in events if entry["event"]=="result")
assert int(metrics["war_routing_copied_bytes_total"])==0
assert int(metrics["war_incoming_messages_total"])==result["verified_deliveries"]
record={"diagnostic_only":True,"passed":True,"server_command":command,"client_command":client_command,
        "perf_command":perf_command,"client_output":events,"metrics":metrics,
        "server_exit_code":server.returncode,"perf_exit_code":perf.returncode}
(ROOT/"artifacts/aocl-large-profile.json").write_text(json.dumps(record,indent=2)+"\n")
print(json.dumps({"passed":True,"diagnostic_only":True}))
