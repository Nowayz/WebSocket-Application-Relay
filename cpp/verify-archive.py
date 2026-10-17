#!/usr/bin/env python3
"""Verify stored files, decompressed evidence, and every recorded Rust source set."""
import gzip, hashlib, json, pathlib, tarfile
ROOT = pathlib.Path(__file__).resolve().parents[1]
archive = ROOT / 'docs/benchmarks/2026-10-17-modern-cpp'
m = json.loads((archive / 'manifest.json').read_text())

def digest(data):
    return hashlib.sha256(data).hexdigest()
for name, want in m['files'].items():
    assert digest((archive / name).read_bytes()) == want, name
for name, record in m['compressed_originals'].items():
    data = gzip.decompress((archive / name).read_bytes())
    assert digest(data) == record['sha256'] and len(data) == record['bytes'], name
for label, record in m['source_replays'].items():
    with tarfile.open(archive / record['tar']) as source:
        for name, want in record['source_sha256'].items():
            assert digest(source.extractfile(name).read()) == want, (label, name)
build = json.loads((archive / 'cpp-build.json').read_text())
for name, want in build['source_sha256'].items():
    assert digest((archive / 'source' / name).read_bytes()) == want, name
result = {'passed': True, 'files': len(m['files']), 'compressed_evidence': len(m['compressed_originals']), 'rust_source_replays': list(m['source_replays']), 'cpp_binary_sha256': m['cpp_executable_sha256'], 'cpp_library_sha256': m['cpp_library_sha256']}
print(json.dumps(result, indent=2))
