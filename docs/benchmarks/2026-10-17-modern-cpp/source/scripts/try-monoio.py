#!/usr/bin/env python3
"""Rebuild one archived Monoio experiment without changing the default relay."""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "docs/benchmarks/2026-10-17-monoio/sources.json"
COMPLETION_MANIFEST = ROOT / "docs/benchmarks/2026-10-17-monoio-completion/sources.json"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def prepare(variant, output, manifest, manifest_path=MANIFEST):
    if output.exists():
        raise FileExistsError(f"Use a new output directory: {output}")
    record = manifest["variants"][variant]
    patch = ROOT / "docs" / record["patch"]["path"]
    if digest(patch) != record["patch"]["sha256"]:
        raise ValueError("Archived experiment patch checksum differs")
    archive = subprocess.check_output([
        "git", "archive", "--format=tar", manifest["base_revision"], "--",
        "Cargo.toml", "Cargo.lock", "src", "vendor", "tests", "examples",
    ], cwd=ROOT)
    # Apply outside the repository so git's current-directory prefix cannot
    # redirect patch paths into the main checkout. Extraction rejects traversal.
    with tempfile.TemporaryDirectory(prefix="war-monoio-", dir="/tmp") as temporary:
        project = Path(temporary)
        with tarfile.open(fileobj=io.BytesIO(archive)) as contents:
            contents.extractall(project, filter="data")
        subprocess.run(["git", "apply", str(patch)], cwd=project, check=True)
        for name, expected in record["sources"].items():
            if digest(project / name) != expected:
                raise ValueError(f"Experiment source differs: {name}")
        output.parent.mkdir(parents=True, exist_ok=True)
        # move() can merge an existing directory; reserve the destination first
        # and move individual entries so a racing creator is never overwritten.
        output.mkdir()
        for entry in project.iterdir():
            shutil.move(str(entry), output / entry.name)
    return {
        "variant": variant,
        "base_revision": manifest["base_revision"],
        "manifest_sha256": digest(manifest_path),
        "patch_sha256": record["patch"]["sha256"],
        "runner_sha256": digest(Path(__file__)),
        "source_hashes_verified": len(record["sources"]),
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("variant", choices=["readiness", "persistent-unfixed", "persistent", "completion-initial", "completion"],
                        help="experimental backend; completion-initial is the historical failed-overload version")
    parser.add_argument("--output", type=Path, required=True,
                        help="New local Cargo project directory")
    parser.add_argument("--prepare-only", action="store_true")
    args = parser.parse_args()
    manifest_path = COMPLETION_MANIFEST if args.variant.startswith("completion") else MANIFEST
    manifest = json.loads(manifest_path.read_text())
    output = args.output.resolve()
    result = prepare(args.variant, output, manifest, manifest_path)
    record = output / "experiment-build.json"
    record.write_text(json.dumps(result, indent=2) + "\n")
    if not args.prepare_only:
        command = ["cargo", "build", "--manifest-path", str(output / "Cargo.toml"),
                   "--target-dir", str(output / "target"), "--release", "--bins", "--locked"]
        subprocess.run(command, cwd=ROOT, check=True)
        binary = output / "target/release/war-relay"
        result.update(command=command, compiler=subprocess.check_output(
            ["rustc", "-Vv"], text=True), binary=str(binary), binary_sha256=digest(binary))
        record.write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))


if __name__ == "__main__":
    main()
