#!/usr/bin/env python3
"""Run checked freshness comparisons in a temporary, isolated Linux netns.

Requires passwordless sudo for ip/tc, not permission to change host networking.
Workload processes run as the invoking user. The namespace is removed on exit.
"""
import argparse
import json
import os
from pathlib import Path
import pwd
import subprocess
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--server", type=Path, default=ROOT / "target/release/war-relay")
    parser.add_argument("--client", type=Path, default=ROOT / "target/release/examples/state_freshness")
    parser.add_argument("--tls-front", type=Path, default=ROOT / "target/release/examples/tls_front")
    parser.add_argument("--tls-cert", type=Path, required=True)
    parser.add_argument("--tls-key", type=Path, required=True)
    parser.add_argument("--tls-ca", type=Path, required=True)
    parser.add_argument("--profiles", nargs="+", choices=["clean", "loss", "jitter", "reorder"], default=["clean", "loss", "reorder"])
    parser.add_argument("--client-packet-threshold", type=int, help="Passed to freshness.py for the WebTransport modes")
    parser.add_argument("--mtu", type=int, help="Loopback MTU inside the namespace. The default 65536 lets TCP count its window in 64 KiB segments while QUIC uses about 1.2 KiB packets; 1500 makes the two comparable")
    parser.add_argument("--repeats", type=int, default=3)
    parser.add_argument("--seconds", type=int, default=6)
    parser.add_argument("--output-dir", type=Path, required=True)
    args = parser.parse_args()
    if args.repeats < 1:
        raise ValueError("repeats must be positive")
    args.output_dir.mkdir(parents=True, exist_ok=False)
    namespace = "war-state-" + uuid.uuid4().hex[:12]
    user = pwd.getpwuid(os.getuid()).pw_name
    made = False
    runs = []
    try:
        subprocess.run(["sudo", "-n", "ip", "netns", "add", namespace], check=True)
        made = True
        subprocess.run(["sudo", "-n", "ip", "-n", namespace, "link", "set", "lo", "up"], check=True)
        if args.mtu:
            subprocess.run(["sudo", "-n", "ip", "-n", namespace, "link", "set", "lo", "mtu", str(args.mtu)], check=True)
        for profile in args.profiles:
            for repetition in range(args.repeats):
                modes = ["websocket-tls", "stream", "datagram"]
                shift = repetition % len(modes)
                modes = modes[shift:] + modes[:shift]
                for mode in modes:
                    name = f"{profile}-{mode}-{repetition+1}"
                    command = ["sudo", "-n", "ip", "netns", "exec", namespace, "runuser", "-u", user, "--", "python3", str(ROOT / "scripts/freshness.py"), "--isolated-namespace", namespace, "--server", str(args.server.resolve()), "--client", str(args.client.resolve()), "--mode", mode, "--seconds", str(args.seconds), "--seed", str(42+repetition), "--output", str((args.output_dir / (name+".json")).resolve())]
                    if mode == "websocket-tls":
                        for key in ("tls_front", "tls_cert", "tls_key", "tls_ca"):
                            command += ["--"+key.replace("_", "-"), str(getattr(args,key).resolve())]
                    if args.client_packet_threshold is not None and mode != "websocket-tls":
                        command += ["--client-packet-threshold", str(args.client_packet_threshold)]
                    if profile == "jitter":
                        # No loss: 50 ms each way with 3 ms of jitter, which netem turns into reordering.
                        command += ["--delay-ms", "50", "--jitter-ms", "3"]
                    elif profile != "clean":
                        command += ["--delay-ms", "20", "--loss-percent", "2"]
                    if profile == "reorder":
                        command += ["--jitter-ms", "8", "--reorder-percent", "25"]
                    print("START", name, flush=True)
                    start = time.monotonic()
                    with (args.output_dir / (name+".log")).open("x") as log:
                        result = subprocess.run(command, cwd=ROOT, stdout=log, stderr=log, timeout=args.seconds+110)
                    runs.append({"name":name,"command":command,"exit_code":result.returncode,"wall_seconds":time.monotonic()-start})
                    (args.output_dir / "manifest.json").write_text(json.dumps(runs,indent=2)+"\n")
                    print("DONE", name, result.returncode, flush=True)
                    if result.returncode:
                        return result.returncode
    finally:
        if made:
            # All known workloads are reaped before their process exits. On an
            # interrupted child, terminate only processes owned by this new ns.
            pids = subprocess.check_output(["sudo","-n","ip","netns","pids",namespace],text=True).split()
            if pids:
                subprocess.run(["sudo","-n","kill","-TERM",*pids],check=False)
            subprocess.run(["sudo","-n","ip","netns","delete",namespace],check=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
