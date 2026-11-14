"""Process measurement helpers shared by the benchmark harnesses."""
import argparse
import hashlib
import os
from pathlib import Path
import socket
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
SERVER = ROOT / "target/release/war-relay"


def preload_library(value):
    """Resolve one loader-safe library path for a server-only experiment."""
    path = Path(value).resolve()
    if not path.is_file() or any(char.isspace() or char == ":" for char in str(path)):
        raise argparse.ArgumentTypeError("preload must name a file with no whitespace or colon in its absolute path")
    return path


def preload_environment(environment, library):
    # A process-wide preload would also change the supposedly fixed client.
    if environment.get("LD_PRELOAD"):
        raise ValueError("start without LD_PRELOAD; use the server-only preload option")
    return environment | ({"LD_PRELOAD": str(library)} if library else {})


def preload_metadata(library, pid=None):
    if library is None:
        return None
    path = Path(library).resolve()
    if pid is not None:
        mappings = Path(f"/proc/{pid}/maps").read_text().splitlines()
        if not any(line.split(maxsplit=5)[-1] == str(path) for line in mappings):
            raise RuntimeError(f"requested preload was not mapped into server {pid}: {path}")
    return {"path": str(path), "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


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


def cpu_times(stat):
    # comm is parenthesized and may itself contain spaces or parentheses.
    fields = stat.rsplit(")", 1)[1].split()
    ticks = os.sysconf("SC_CLK_TCK")
    user, system = int(fields[11]), int(fields[12])
    return {"cpu_seconds": (user + system) / ticks,
            "user_cpu_seconds": user / ticks,
            "system_cpu_seconds": system / ticks}


def snapshot(pid):
    values = {}
    for line in Path(f"/proc/{pid}/status").read_text().splitlines():
        if line.startswith(("VmRSS:", "VmSize:", "RssAnon:", "Threads:")):
            key, value = line.split(":", 1)
            values[key] = int(value.split()[0])
    values.update(cpu_times(Path(f"/proc/{pid}/stat").read_text()))
    # Quiescent-gate diagnostics expose flow/worker placement without sampling
    # inside a timed request. Threads may retire between directory and stat.
    threads = {}
    for task in Path(f"/proc/{pid}/task").iterdir():
        try:
            threads[task.name] = cpu_times((task / "stat").read_text())["cpu_seconds"]
        except FileNotFoundError:
            continue
    values["thread_cpu_seconds"] = threads
    return values


def metrics(port):
    with urllib.request.urlopen(f"http://127.0.0.1:{port}/metrics", timeout=1) as response:
        return dict(line.split() for line in response.read().decode().splitlines() if line.startswith("war_"))
