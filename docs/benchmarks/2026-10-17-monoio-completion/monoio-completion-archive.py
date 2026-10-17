"""Archive both completion prototypes, reproducible sources and raw evidence."""
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import io

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "docs/benchmarks/2026-10-17-monoio-completion"
sha = lambda path: hashlib.sha256(path.read_bytes()).hexdigest()
OUT.mkdir(exist_ok=True)
base = json.loads((ROOT / "artifacts/candidates/monoio-completion/build.json").read_text())["base_revision"]
index = {"base_revision": base, "variants": {}}
for label, candidate in (("completion-initial", "monoio-completion"), ("completion", "monoio-completion-bounded")):
    directory = ROOT / "artifacts/candidates" / candidate
    record = json.loads((directory / "build.json").read_text())
    for name, expected in record["sources"].items():
        assert sha(directory / "sources" / name) == expected, name
    with tempfile.TemporaryDirectory(prefix="war-completion-patch-") as temporary:
        tree = Path(temporary)
        archive = subprocess.check_output(["git", "archive", "--format=tar", base, "--",
            "Cargo.toml", "Cargo.lock", "src", "vendor", "tests", "examples"], cwd=ROOT)
        (tree / "base").mkdir()
        with tarfile.open(fileobj=io.BytesIO(archive)) as contents:
            contents.extractall(tree / "base", filter="data")
        shutil.copytree(directory / "sources", tree / "candidate")
        compared = subprocess.run(["git", "diff", "--no-index", "--binary", "base", "candidate"],
                                  cwd=tree, capture_output=True, text=True)
        assert compared.returncode == 1, compared.stderr
        patch = compared.stdout
        for old, new in (("a/base/", "a/"), ("b/candidate/", "b/"), ("a/candidate/", "a/"), ("b/base/", "b/")):
            patch = patch.replace(old, new)
        path = OUT / "sources" / f"{label}.patch"
        path.parent.mkdir(exist_ok=True)
        path.write_text(patch)
        subprocess.run(["git", "apply", str(path)], cwd=tree / "base", check=True)
        replayed = {str(p.relative_to(tree / "base")): sha(p) for p in (tree / "base").rglob("*") if p.is_file()}
        assert replayed == record["sources"], (replayed.keys() ^ record["sources"].keys())
    target = OUT / "sources" / f"{label}-build.json"
    shutil.copy2(directory / "build.json", target)
    index["variants"][label] = {**record,
        "patch": {"path": str(path.relative_to(ROOT / "docs")), "sha256": sha(path)},
        "note": "Historical failed-overload prototype; diagnostic only" if label.endswith("initial") else "Functional experiment; measured slower than selected Mio; not adopted"}
(OUT / "sources.json").write_text(json.dumps(index, indent=2) + "\n")

for path in sorted((ROOT / "artifacts").glob("monoio-completion*")):
    if path.is_file() and not path.name.endswith(".perf.data"):
        shutil.copy2(path, OUT / path.name)
for name in ("scripts/inspection.py", "scripts/capacity.py", "scripts/capacity_pressure.py", "scripts/benchmark.py", "scripts/try-monoio.py"):
    target = OUT / "scripts" / Path(name).name
    target.parent.mkdir(exist_ok=True)
    shutil.copy2(ROOT / name, target)

# Preserve the exact capacity helper used before the additional failed-run
# snapshot was added. Historical trace hashes must remain reproducible.
historical = (ROOT / "scripts/capacity.py").read_text()
historical = historical.replace('''                        # Preserve the full numeric snapshot before a live
                        # assertion so a failed trial can identify its cause.
                        result["last_monitoring_metrics"] = current
                        assert args.users <= current["war_sessions"] <= args.users + args.churn_workers * args.room_size + extra, (
                            "live sessions", current["war_sessions"], args.users,
                            args.users + args.churn_workers * args.room_size + extra)
''', '''                        assert args.users <= current["war_sessions"] <= args.users + args.churn_workers * args.room_size + extra
''')
target = OUT / "scripts/capacity-before-diagnostic.py"
target.write_text(historical)
expected = json.loads((ROOT / "artifacts/monoio-completion-capacity-completion-1.json").read_text())["runner_sources_sha256"]["scripts/capacity.py"]
assert sha(target) == expected

cohorts = {}
for name in ("monoio-completion-private", "monoio-completion-capacity", "monoio-completion-bounded-private", "monoio-completion-bounded-capacity"):
    data = json.loads((ROOT / "artifacts" / f"{name}.json").read_text())
    cohorts[name] = {"passed": data["passed"], "runs": len(data["runs"]), "summary": data.get("summary")}
manifest = {"scope": "Owned Monoio completion-I/O study; both prototypes and failed trials retained. Default native runtime remains the selected Mio input-batch binary.",
    "selected_native_sha256": "35d46da18a5eb4e945d5eacb7962581e520b7e613e9006d89db559264e63db6e",
    "base_revision": base, "cohorts": cohorts, "source_index_sha256": sha(OUT / "sources.json"),
    "files": {str(p.relative_to(OUT)): sha(p) for p in sorted(OUT.rglob("*")) if p.is_file()}}
(OUT.with_suffix(".json")).write_text(json.dumps(manifest, indent=2) + "\n")
print(json.dumps({"passed": True, "archived_files": len(manifest["files"]), "source_index_sha256": manifest["source_index_sha256"]}), flush=True)
