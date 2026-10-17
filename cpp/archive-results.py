#!/usr/bin/env python3
"""Archive accepted traces and replay the five Rust build-source records."""
import gzip, hashlib, io, json, pathlib, platform, shutil, subprocess, tarfile, tempfile
ROOT = pathlib.Path(__file__).resolve().parents[1]
A = ROOT / 'artifacts'
OUT = ROOT / 'docs/benchmarks/2026-10-17-modern-cpp'
OUT.mkdir(parents=True, exist_ok=True)

def sha(data):
    return hashlib.sha256(data).hexdigest()

def put(source, dest):
    dest = OUT / dest
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, dest)
build = json.loads((A / 'candidates/cpp-modern/build.json').read_text())
for name, want in build['source_sha256'].items():
    assert sha((ROOT / name).read_bytes()) == want, name
for p in (ROOT / 'cpp').rglob('*'):
    if p.is_file() and '__pycache__' not in p.parts:
        put(p, 'source/' + str(p.relative_to(ROOT)))
for p in (ROOT / 'scripts').glob('*.py'):
    put(p, 'source/scripts/' + p.name)
put(A / 'candidates/cpp-modern/build.json', 'cpp-build.json')
compressed = {}
patterns = ['cpp-modern-final-comparison*', 'cpp-modern-final-mixed*', 'cpp-modern-final-aocl*', 'cpp-modern-accepted-100k*', 'cpp-modern-counter-ablation*', 'cpp-modern-batch-ablation*', 'cpp-modern-private-unlock-ablation*', 'cpp-modern-*corpus*', 'cpp-modern-*runtime*', 'cpp-modern-*pressure*', 'cpp-modern-*transport*', 'cpp-modern-*build.log', 'cpp-modern-*profile*.txt', 'cpp-modern-write-annotate.txt', 'cpp-modern-analysis.json', 'cpp-modern-replay-check.json', 'cpp-modern-machine.json', 'cpp-modern-plot.log', 'cpp-modern-*ctest*', 'cpp-asan-accepted*', 'cpp-tsan-*', 'cpp-cmake-*']
files = set()
for pattern in patterns:
    files.update((p for p in A.glob(pattern) if p.is_file()))
for p in sorted(files):
    if '.perf.data' in p.name:
        continue
    data = p.read_bytes()
    name = 'raw/' + p.name
    if p.suffix == '.csv' or len(data) > 256000:
        target = name + '.gz'
        (OUT / target).parent.mkdir(parents=True, exist_ok=True)
        (OUT / target).write_bytes(gzip.compress(data, mtime=0))
        compressed[target] = {'original_name': name, 'sha256': sha(data), 'bytes': len(data)}
    else:
        put(p, name)
for p in A.glob('cpp-modern-*.png'):
    put(p, p.name)
for p in A.glob('cpp-modern-*.pdf'):
    put(p, p.name)
put(A / 'cpp-modern-stock-transport.json', 'raw/cpp-modern-stock-transport.json')
put(A / 'cpp-modern-existing-service-check.json', 'raw/cpp-modern-existing-service-check.json')
put(A / 'aocl-build.json', 'aocl-build.json')
for label, folder in [('v0', 'cpp-modern-v0'), ('v1', 'cpp-modern-v1'), ('v2', 'cpp-modern-v2'), ('before-physical-close', 'cpp-before-physical-close')]:
    for p in (A / 'candidates' / folder).iterdir():
        if p.is_file() and p.suffix in ('.cpp', '.hpp', '.sh', '.json', '.patch'):
            put(p, 'experiments/' + label + '/' + p.name)
sets = [('rust-selected', A / 'candidates/observer-quantum', 'runtime.patch', 'sources'), ('rust-observer-index', A / 'candidates/observer-index', 'runtime.patch', 'sources'), ('observer-client', A / 'candidates/observer-quantum-client', 'change.patch', 'sources'), ('capacity-client', A / 'candidates/latency-timeline-client', 'change.patch', None), ('stress-client', A / 'baselines/capacity-final', 'runtime.patch', 'files')]
replays = {}
for label, folder, patch, key in sets:
    record = json.loads((folder / 'build.json').read_text())
    put(folder / 'build.json', 'provenance/' + label + '-build.json')
    put(folder / patch, 'provenance/' + label + '-' + patch)
    names = set(subprocess.check_output(['git', 'ls-tree', '--name-only', record['base_revision']], cwd=ROOT, text=True).splitlines())
    paths = [n for n in ['src', 'tests', 'vendor', 'examples', 'scripts', 'deploy', 'Cargo.toml', 'Cargo.lock'] if n in names]
    with tempfile.TemporaryDirectory(prefix='war-cpp-source-') as temporary:
        stage = pathlib.Path(temporary)
        data = subprocess.check_output(['git', 'archive', record['base_revision'], *paths], cwd=ROOT)
        with tarfile.open(fileobj=io.BytesIO(data)) as archive:
            archive.extractall(stage, filter='data')
        subprocess.run(['git', 'apply', str((folder / patch).resolve())], cwd=stage, check=True)
        overlays = {}
        if label == 'stress-client':
            for name in ['examples/capacity.rs', 'scripts/capacity.py', 'scripts/capacity_pressure.py']:
                target = stage / name
                assert not target.exists()
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(folder / name, target)
                overlays[name] = sha(target.read_bytes())
        checks = record[key] if key else {'examples/capacity.rs': record['source_sha256'], 'Cargo.lock': record['cargo_lock_sha256']}
        checks = {name: digest for name, digest in checks.items() if key != 'files' or name.endswith(('.rs', '.py', '.toml', '.lock'))}
        for name, digest in checks.items():
            assert sha((stage / name).read_bytes()) == digest, (label, name)
        tarname = 'provenance/' + label + '-source.tar.gz'
        with tarfile.open(OUT / tarname, 'w:gz') as archive:
            for p in sorted(stage.rglob('*')):
                if p.is_file():
                    archive.add(p, arcname=str(p.relative_to(stage)), recursive=False)
        replays[label] = {'tar': tarname, 'base_revision': record['base_revision'], 'source_sha256': checks, 'original_untracked_overlays': overlays}
host = {'snapshot': 'after ordinary runs', 'platform': platform.platform(), 'libc': platform.libc_ver(), 'python': platform.python_version(), 'revision': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip()}
(OUT / 'host.json').write_text(json.dumps(host, indent=2) + '\n')
readme = '# Modern C++ relay comparison archive\n\nSee [the report](../../../cpp/BENCHMARKS.md) for the results and their limits.\n\nThe accepted study contains 36 native CBOR saturation trials, six competing\nobserver trials, eight AOCL trials, and four 100,000-user trials with 48 million\nverified timed probes. All runtime processes are fresh and sequential. Per-user\nCSV files and large JSON receipts are gzip-compressed; `manifest.json` retains\nboth stored-file hashes and original uncompressed hashes.\n\n`source/cpp` is the accepted C++ source snapshot, including its pinned setup,\ntransport patch, build, tests and comparison/analysis tools. `cpp-build.json`\nbinds runtime source hashes to the executable and static library. The independent\nnative replay reproduced both hashes exactly. The compared C++ executable is the\none in that build record. Five Rust source archives are reconstructed from their\nsaved base revisions and patches and checked against their build-source hashes;\nthese include the three original then-untracked stress-client build inputs.\nNo runtime binaries are committed in this archive.\n\nThe normal matrix used the exact original runner saved under\n`raw/cpp-modern-final-comparison-runner.py`; its working-tree counterpart was\nsubsequently formatted and adds a runner hash to future traces. Other accepted\nrunners record their source hash in their JSON. The test fixtures used for wire\nverification are also saved under `source/scripts`.\n\nThe earlier tuning trials inherit the raw key `rust` for a **C++ control**:\nfor counter and batch experiments it means C++ v0; for guard-release it means\nC++ v2. Use the executable hashes, not that inherited label. Available original\ncontrol snapshots are under `experiments/`. The intermediate batch candidate\nwas overwritten before a separate source freeze, so its short tuning result is\nexplanatory evidence, not a claim of an independently replayable source build.\nThe final retained batching implementation is fully source-bound and retested.\nEarlier preliminary 100k CSVs remain only in the working `artifacts` directory;\nthe complete committed census is the accepted four-trial study.\n\nVerification from the repository root:\n\n```sh\npython3 cpp/verify-archive.py\n```\n\nFor a fresh native build, run `cpp/setup.sh` and `cpp/build.sh`; LLVM/Clang,\nllvm-ar, lld, a C compiler, make and Linux pthread/epoll support are required.\nThe CMake route and library embedding example are in `cpp/README.md`. To rerun\nthe original comparisons, rebuild the recorded Rust sources/clients with their\nrecorded compiler/profile and place them at the frozen paths used by the saved\nrunner commands. Do not substitute a new client build while attributing its\nresults to an old binary hash. The AOCL trial additionally requires the dispatcher\nbuild recorded in `aocl-build.json`; it was not adopted.\n\nPopulation memory excludes kernel TCP and client memory. The C++ operational\nfeature gaps remain explicit in the report. No existing manual relay service was\nreplaced, and these measurements are not a production deployment or acceptance.\n'
(OUT / 'README.md').write_text(readme)
manifest = {'scope': 'Accepted C++ source, checked results and verified Rust build-source replays', 'cpp_executable_sha256': build['binary_sha256'], 'cpp_library_sha256': build['library_sha256'], 'source_replays': replays, 'compressed_originals': compressed, 'files': {str(p.relative_to(OUT)): sha(p.read_bytes()) for p in sorted(OUT.rglob('*')) if p.is_file() and p.name != 'manifest.json'}}
(OUT / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps({'archive': str(OUT), 'files': len(manifest['files']), 'source_replays': list(replays)}))
