#!/usr/bin/env python3
"""Verify that the local codecs equal pinned published archives plus our patch."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tarfile
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def files(root):
    return {path.relative_to(root).as_posix(): digest(path)
            for path in root.rglob("*") if path.is_file()}


def main():
    provenance = json.loads((ROOT / "vendor/provenance.json").read_text())
    cache = ROOT / "target/vendor-source-cache"
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="war-vendor-") as directory:
        prepared = Path(directory)
        published = {}
        for name, info in provenance.items():
            filename = f"{name}-{info['version']}.crate"
            archive = cache / filename
            if not archive.exists():
                url = f"https://static.crates.io/crates/{name}/{filename}"
                with urllib.request.urlopen(url, timeout=60) as response:
                    temporary = archive.with_suffix(".download")
                    temporary.write_bytes(response.read())
                    temporary.replace(archive)
            if digest(archive) != info["package_sha256"]:
                raise RuntimeError(f"published archive checksum mismatch: {name}")
            with tarfile.open(archive) as contents:
                contents.extractall(prepared, filter="data")
            (prepared / f"{name}-{info['version']}").rename(prepared / name)
            published[name] = files(prepared / name)
        # Apply in a separate, disposable filesystem tree. Ambient Git settings
        # must not redirect this verification into the calling worktree.
        environment = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
        subprocess.run(["git", "apply", str(ROOT / "vendor/read-buffer-reclaim.patch")],
                       cwd=prepared, env=environment, check=True)
        total = 0
        for name in provenance:
            expected, actual = files(prepared / name), files(ROOT / "vendor" / name)
            changed = sorted(k for k in published[name].keys() | expected.keys()
                             if published[name].get(k) != expected.get(k))
            if changed != sorted(provenance[name]["modified_files"]):
                raise RuntimeError(f"{name} patch scope differs from its provenance: {changed}")
            if expected != actual:
                changed = sorted(k for k in expected.keys() | actual.keys()
                                 if expected.get(k) != actual.get(k))
                raise RuntimeError(f"{name} differs from its published archive plus patch: {changed}")
            total += len(actual)
    print(f"Verified {total} vendored files against two pinned published archives plus the declared patch.")


if __name__ == "__main__":
    main()
