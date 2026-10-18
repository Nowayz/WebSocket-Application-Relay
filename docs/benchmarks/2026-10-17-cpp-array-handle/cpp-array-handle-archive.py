from pathlib import Path
from collections import Counter
import gzip, hashlib, json, shutil, statistics, subprocess, urllib.request

root = Path.cwd(); a = root / 'artifacts'
out = root / 'docs/benchmarks/2026-10-17-cpp-array-handle'
out.mkdir(exist_ok=True)
sha = lambda data: hashlib.sha256(data).hexdigest()
record = json.loads((a / 'cpp-array-handle-placement.json').read_text())
assert record['passed'] and len(record['runs']) == 24
assert sha((root / 'cpp/placement-compare.py').read_bytes()) == record['runner_sha256']
dependency = root / 'docs/benchmarks/2026-10-17-cpp-placement'
old = root / 'docs/benchmarks/2026-10-17-cpp-single-allocation'
builds = {}
for kind, folder in [('control', 'cpp-array-handle-control'), ('array', 'cpp-single-allocation-candidate'), ('handle', 'cpp-array-handle-candidate')]:
    source = a / 'candidates' / folder
    build = json.loads((source / 'build.json').read_text())
    assert sha((source / 'war-relay-cpp').read_bytes()) == build['binary_sha256'] == record['server_sha256'][kind]
    for name, want in build['source_sha256'].items():
        assert sha((source / name).read_bytes()) == want, (kind, name)
    builds[kind] = build
for row in record['runs']:
    assert row['passed'] and row['client_exit_code'] == row['server_exit_code'] == 0
    assert len(row['plan']) == 32 and len({index for pair in row['plan'] for index in pair}) == 64
    peers = row['ownership']; assert len(peers) == 256
    for column in [0, 1]:
        assert sorted(Counter(peers[pair[column]]['epoll_fd'] for pair in row['plan']).values()) == [8] * 4
    for first, second in row['plan']:
        assert (peers[first]['epoll_fd'] == peers[second]['epoll_fd']) == (row['mode'] == 'local')
    assert row['measured_copied_bytes'] == (0 if row['mode'] == 'local' else row['result']['verified_deliveries'] * 1037)
    assert row['after']['fd_count'] == row['baseline']['fd_count']
    assert row['gates'][-1]['normal_close_and_eof'] == 256
    assert all(row['final_metrics'][key] == 0 for key in ['war_connections', 'war_users', 'war_queued_bytes', 'war_buffered_bytes', 'war_retained_output_bytes', 'war_variable_bytes'])
corpus = json.loads((a / 'cpp-array-handle-corpus.json').read_text())
assert corpus['passed'] and corpus['cases'] == 776 and corpus['exchanges'] == 2311
layout = json.loads((a / 'cpp-array-handle-layout.json').read_text())
assert layout == {'retained_handle_bytes': 16, 'previous_wrapper_bytes': 24, 'delivery_bytes': 40, 'ownership_checked': True}
profile = json.loads((a / 'cpp-array-handle-baseline-mixed-profile.json').read_text())
assert profile['passed'] and profile['server_exit_code'] == 0 and profile['server_sha256'] == builds['control']['binary_sha256']
accepted = a / 'candidates/cpp-modern'
assert sha((accepted / 'war-relay-cpp').read_bytes()) == builds['control']['binary_sha256']
assert sha((accepted / 'libwar-relay.a').read_bytes()) == builds['control']['library_sha256']
for name, want in builds['control']['source_sha256'].items():
    assert sha((root / name).read_bytes()) == want
client_build = json.loads((a / 'candidates/placement-client/build.json').read_text())
assert record['client_sha256'] == client_build['client_sha256']
assert subprocess.check_output(['git', 'diff', '--', 'src/relay.rs', 'src/relay/listener_tests.rs']) == (a / 'candidates/placement-client/library-worktree.patch').read_bytes()
manual = json.loads((a / 'reactor-manual-services.json').read_text()); pid = manual['pids'][0]
assert sha(Path(f'/proc/{pid}/exe').read_bytes()) == sha((a / 'candidates/observer-quantum/war-relay').read_bytes())
with urllib.request.urlopen('http://127.0.0.1:1339/readyz', timeout=3) as response:
    assert response.status == 200
summary = {'passed': True, 'decision': 'Reject compact-handle refinement: no material improvement over the already-unselected array prototype. Accepted runtime restored.',
           'verified_deliveries': sum(row['result']['verified_deliveries'] for row in record['runs']),
           'trials': 24, 'cases': {}, 'layout': layout,
           'accepted_cpp_sha256': builds['control']['binary_sha256'],
           'accepted_library_sha256': builds['control']['library_sha256'],
           'rejected_candidate_sha256': builds['handle']['binary_sha256'],
           'manual_rust_ready_and_unchanged': True,
           'existing_rust_diff_unchanged': True,
           'sanitizers': 'Not rerun for rejected prototype; accepted sanitizer evidence remains separate.'}
for mode in ['local', 'remote']:
    summary['cases'][mode] = {}
    for kind in ['control', 'array', 'handle']:
        rows = [row for row in record['runs'] if row['mode'] == mode and row['kind'] == kind]
        summary['cases'][mode][kind] = {
            'median_deliveries_per_second': statistics.median(row['result']['deliveries_per_second'] for row in rows),
            'median_cpu_us_per_delivery': statistics.median(row['server_cpu_seconds'] / row['result']['verified_deliveries'] * 1e6 for row in rows),
            'p99_upper_ms': [row['result']['latency_p99_upper_ms'] for row in rows]}
(a / 'cpp-array-handle-analysis.json').write_text(json.dumps(summary, indent=2) + '\n')
compressed = {}
for path in sorted(a.glob('cpp-array-handle-*')):
    if not path.is_file() or path.name == 'cpp-array-handle-archive.log' or path.suffix not in {'.py', '.cpp', '.json', '.log', '.txt'}:
        continue
    data, name = path.read_bytes(), path.name
    if path.suffix in {'.json', '.txt'} and len(data) > 256000:
        name += '.gz'; compressed[name] = {'sha256': sha(data), 'bytes': len(data)}; data = gzip.compress(data, mtime=0)
    (out / name).write_bytes(data)
shutil.copyfile(root / 'cpp/placement-compare.py', out / 'placement-compare.py')
source = a / 'candidates/cpp-array-handle-candidate'
shutil.copytree(source / 'cpp', out / 'source/cpp', dirs_exist_ok=True)
for name in ['build.json', 'runtime.patch', 'base_revision']:
    shutil.copyfile(source / name, out / 'source' / name)
lines = ['# Compact retained-body handle trial', '',
         '**Decision: reject this refinement and restore the accepted modern C++ runtime.**', '',
         'The earlier array prototype used a 24-byte transient `OwnedMessage` wrapper containing a shared pointer and length. This version uses a 16-byte shared pointer as the transient handle; only queued deliveries store the length. The delivery remains 40 bytes. It preserves one shared ownership allocation for retained bodies, one body copy, immutable publication, full size_t lengths, quotas, FIFO and the borrowed local path. It was tested from base commit `2513b3c`.', '',
         'The hypothesis was that eliminating the redundant transient length would recover local-path performance while retaining the array allocation benefit. A native layout/ownership diagnostic verifies the size change and independence from later input mutation. The unchanged shared protocol corpus passes 776 cases / 2,311 exchanges.', '',
         '## Controlled comparison', '',
         'Twenty-four fresh eight-second trials compare accepted C++, the previous array prototype and this compact handle: four trials per build and topology, reversing build order on alternate repetitions. Four pinned workers, 32 disjoint private pairs, 256 connected peers, 1 KiB values, window16, the same frozen checked client. Exactly eight sources and eight receivers per worker. External TCP inode/fd/epoll observations verify every pair is either local or remote before and after measurement. CPU gates exclude setup, topology queries, templates, postchecks and retirement. The middle array arm remains in the middle; accepted/compact order reverses. No ordinary timing overlaps builds or profiling.', '',
         '| Topology | Build | Median deliveries/s | Median CPU us/delivery | Per-trial p99 upper ms |',
         '|---|---|---:|---:|---|']
for mode in ['local', 'remote']:
    for kind in ['control', 'array', 'handle']:
        value = summary['cases'][mode][kind]
        lines.append(f"| {mode} | {kind} | {value['median_deliveries_per_second']:,.0f} | {value['median_cpu_us_per_delivery']:.3f} | {', '.join(f'{v:.3f}' for v in value['p99_upper_ms'])} |")
lines += ['', f"The ordinary trials verify **{summary['verified_deliveries']:,} deliveries**. Local CPU is essentially unchanged across all three arms. Remote compact-handle throughput is about 1.0% lower and CPU/delivery 0.85% higher than the earlier array prototype. This does not justify selection or the hypothesis that the transient wrapper caused the earlier local gap. The accepted/array local medians are also effectively equal in this cohort, reinforcing the need to keep small timing differences qualified. All local trials record zero retained-copy bytes; all remote trials exactly 1,037 copied bytes per verified delivery. Every case completes all 256 normal Close/EOF exchanges and returns descriptors and resource gauges to baseline. Histogram latency includes client scheduling/windowing, not an isolated relay hop.", '',
          'No fresh 100,000-user or sanitizer campaign was run for this rejected refinement. Its failure to improve the already-rejected alternative did not justify promotion to those acceptance stages. Prior array population/mixed evidence and accepted sanitizer evidence remain separate.', '',
          '## Separate baseline profile and next target', '',
          'A fresh accepted-build mixed-load profile runs separately after ordinary trials: 512 authenticated global observers, one private pair, eight churn workers and two CBOR inspectors, six seconds of checked traffic. The profiler records cycles:u at 199 Hz with DWARF call stacks. The successful capture has about 3,000 samples and zero lost samples. It includes setup/retirement and does not measure off-CPU waits or isolated kernel cost.', '',
          '| Function | Share of recorded event-cycle samples |',
          '|---|---:|',
          '| Worker::write | 24.75% |',
          '| Worker::submit | 13.04% |',
          '| Worker::drain | 11.18% |',
          '| Worker::reconcile_buffer | 9.63% |',
          '| pthread_mutex_lock | 3.48% |', '',
          'Annotations concentrate samples on instructions immediately following locked updates to the retained-output budget and buffered-byte counters. Sampling skid prevents interpreting this as precise instruction timing. Some addresses remain unresolved/kernel-looking even in the cycles:u capture; the profile is not a complete causal accounting of wall time. The result motivates measuring global output-accounting cache-line placement and contention, not asserting a diagnosed false-sharing cause. Earlier zero-byte guards and per-worker-counter experiments remain rejected; any new accounting change must be isolated, preserve exact quota/retirement behavior, and demonstrate competing-traffic and population responsiveness.', '',
          'The first attempted diagnostic lacked the server fixture auth password, so the checked client rejected authentication. That failed setup and driver are preserved explicitly and excluded from profile conclusions. The corrected successful capture is separate. Raw perf data stays local outside Git; build IDs, text report, instruction annotations, commands, exact binaries and client hashes are preserved here.', '',
          '## Restoration and provenance', '',
          f"- Accepted executable restored: `{builds['control']['binary_sha256']}`.",
          f"- Accepted static library restored: `{builds['control']['library_sha256']}`.",
          f"- Rejected compact-handle executable: `{builds['handle']['binary_sha256']}`.",
          '- Restored build-source hashes are checked. The selected manual Rust executable/readiness and existing Rust experiment diff are unchanged. No push or deployment occurs.',
          '- Complete compact-handle sources, patch and build metadata are under source/. Accepted and older-array sources are in the prior ownership study; the frozen client and controller provenance are in the controlled-placement study. This archive verifies that explicit dependency chain.',
          '- Each case retains its ownership census, exact pairing plan, measured copy counts, gates, resource counters and complete client logs. Large JSON/text originals have separate uncompressed hashes.', '',
          'Verify with `python3 docs/benchmarks/2026-10-17-cpp-array-handle/verify.py`.']
(out / 'README.md').write_text('\n'.join(lines) + '\n')
verifier = '''from pathlib import Path
import gzip,hashlib,json,subprocess,sys
p=Path(__file__).resolve().parent;m=json.loads((p/'manifest.json').read_text());sha=lambda data:hashlib.sha256(data).hexdigest()
for name,want in m['files'].items():assert sha((p/name).read_bytes())==want,name
for name,r in m['compressed_originals'].items():
 data=gzip.decompress((p/name).read_bytes());assert sha(data)==r['sha256'] and len(data)==r['bytes'],name
d=p.parent/'2026-10-17-cpp-placement';assert sha((d/'manifest.json').read_bytes())==m['dependency_manifest_sha256']
subprocess.run([sys.executable,str(d/'verify.py')],check=True)
f=p/'cpp-array-handle-placement.json.gz';r=json.loads(gzip.decompress(f.read_bytes()) if f.exists() else (p/'cpp-array-handle-placement.json').read_bytes())
assert r['passed'] and len(r['runs'])==24 and all(row['passed'] and row['server_exit_code']==0 for row in r['runs'])
assert r['runner_sha256']==sha((p/'placement-compare.py').read_bytes())
b=json.loads((p/'source/build.json').read_text());assert b['binary_sha256']==r['server_sha256']['handle']
for name,want in b['source_sha256'].items():assert sha((p/'source'/name).read_bytes())==want,name
assert r['client_sha256']==json.loads((d/'client-build.json').read_text())['client_sha256']
print(json.dumps({'passed':True,'files':len(m['files']),'ordinary_trials':24,'dependency_verified':True}))
'''
(out / 'verify.py').write_text(verifier)
manifest = {'compressed_originals': compressed, 'dependency_manifest_sha256': sha((dependency / 'manifest.json').read_bytes()),
            'files': {str(path.relative_to(out)): sha(path.read_bytes()) for path in sorted(out.rglob('*')) if path.is_file() and path.name != 'manifest.json'}}
(out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps(summary))
