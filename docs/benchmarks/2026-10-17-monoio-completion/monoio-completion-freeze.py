"""Prepare vendor diff, validate and freeze the completion experiment."""
import hashlib
import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
PROJECT = ROOT / "artifacts/experiments/monoio-completion"
parser = argparse.ArgumentParser()
parser.add_argument("--variant", default="monoio-completion")
args = parser.parse_args()
assert args.variant in ("monoio-completion", "monoio-completion-bounded")
DESTINATION = ROOT / "artifacts/candidates" / args.variant
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()

def sources():
    paths = [PROJECT / "Cargo.toml", PROJECT / "Cargo.lock"]
    for folder in ("src", "vendor", "tests", "examples"):
        paths += [path for path in (PROJECT / folder).rglob("*") if path.is_file()]
    paths += [PROJECT / "crates/completion-buf/Cargo.toml", PROJECT / "crates/completion-buf/src/lib.rs"]
    return {str(path.relative_to(PROJECT)): sha(path) for path in sorted(paths)}

provenance = json.loads((PROJECT / "vendor/provenance.json").read_text())
with tempfile.TemporaryDirectory(prefix="war-completion-vendor-") as temporary:
    base = Path(temporary)
    for folder in ("published", "changed"):
        (base / folder).mkdir()
    patches = []
    for name, info in provenance.items():
        archive = ROOT / "target/vendor-source-cache" / f"{name}-{info['version']}.crate"
        assert sha(archive) == info["package_sha256"]
        with tarfile.open(archive) as contents:
            contents.extractall(base / "published", filter="data")
        (base / "published" / f"{name}-{info['version']}").rename(base / "published" / name)
        shutil.copytree(PROJECT / "vendor" / name, base / "changed" / name)
        result = subprocess.run(["git", "diff", "--no-index", "--binary", f"published/{name}", f"changed/{name}"],
                                cwd=base, capture_output=True, text=True)
        assert result.returncode in (0, 1), result.stderr
        patch = result.stdout
        for old, new in (("a/published/", "a/"), ("b/changed/", "b/"), ("a/changed/", "a/"), ("b/published/", "b/")):
            patch = patch.replace(old, new)
        patches.append(patch)
    (PROJECT / "vendor/read-buffer-reclaim.patch").write_text("".join(patches))

before = sources()
commands = [
    ("fmt", ["cargo", "fmt", "--manifest-path", str(PROJECT / "Cargo.toml"), "--all", "--check"]),
    ("vendor", ["python3", str(PROJECT / "scripts/check-vendor.py")]),
    ("tests", ["cargo", "test", "--manifest-path", str(PROJECT / "Cargo.toml"), "--all-targets", "--locked"]),
    ("clippy", ["cargo", "clippy", "--manifest-path", str(PROJECT / "Cargo.toml"), "--all-targets", "--locked", "--", "-D", "warnings"]),
    ("buf-tests", ["cargo", "test", "--manifest-path", str(PROJECT / "crates/completion-buf/Cargo.toml")]),
    ("buf-clippy", ["cargo", "clippy", "--manifest-path", str(PROJECT / "crates/completion-buf/Cargo.toml"), "--all-targets", "--", "-D", "warnings"]),
    ("build", ["cargo", "build", "--manifest-path", str(PROJECT / "Cargo.toml"), "--release", "--bins", "--locked"]),
]
for label, command in commands:
    print(json.dumps({"started": label}), flush=True)
    with (ROOT / f"artifacts/{args.variant}-freeze-{label}.log").open("w") as output:
        subprocess.run(command, cwd=ROOT, stdout=output, stderr=output, check=True)
assert before == sources(), "source changed during validation/build"
DESTINATION.mkdir()
for name in before:
    target = DESTINATION / "sources" / name
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(PROJECT / name, target)
binaries = {}
for name in ("war-relay", "war-legacy-bridge"):
    binary = PROJECT / "target/release" / name
    shutil.copy2(binary, DESTINATION / name)
    binaries[name] = sha(binary)
record = {
    "base_revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
    "variant_name": args.variant,
    "variant": "Monoio 0.2.4 owned receive/writev, single accepted descriptor, bounded shared-envelope handoff, 14-byte idle receive, completion-owned staging/output leases. Experiment only.",
    "compiler": subprocess.check_output(["rustc", "-Vv"], text=True),
    "command": commands[-1][1], "sources": before, "binaries": binaries,
    "source_root": str((DESTINATION / "sources").relative_to(ROOT)),
}
(DESTINATION / "build.json").write_text(json.dumps(record, indent=2) + "\n")
print(json.dumps({"passed": True, "sources": len(before), "binaries": binaries}), flush=True)
