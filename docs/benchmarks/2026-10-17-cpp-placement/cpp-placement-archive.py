from pathlib import Path
from collections import Counter
import gzip
import hashlib
import json
import shutil
import statistics
import subprocess

import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

root = Path.cwd()
a = root / 'artifacts'
out = root / 'docs/benchmarks/2026-10-17-cpp-placement'
out.mkdir(exist_ok=True)
sha = lambda data: hashlib.sha256(data).hexdigest()
record = json.loads((a / 'cpp-placement-controlled.json').read_text())
assert record['passed'] and len(record['runs']) == 16
assert record['runner_sha256'] == sha((a / 'cpp-placement-controlled-runner.py').read_bytes())
client = a / 'candidates/placement-client'
build = json.loads((client / 'build.json').read_text())
assert record['client_sha256'] == build['client_sha256'] == sha((client / 'placement').read_bytes())
for name, digest in build['source_sha256'].items():
    assert sha((client / 'source' / name).read_bytes()) == digest
source_archive = root / 'docs/benchmarks/2026-10-17-cpp-single-allocation'
for label, kind in [('control', 'control'), ('array', 'candidate')]:
    server_build = json.loads((source_archive / 'source' / kind / 'build.json').read_text())
    assert record['server_sha256'][label] == server_build['binary_sha256']
summary = {'passed': True, 'scope': record['scope'], 'cases': {}, 'verified_deliveries': 0,
           'decision': 'Measurement infrastructure retained. Array prototype remains unselected; its prior mixed/population latency tradeoffs still apply.'}
for row in record['runs']:
    assert row['passed'] and row['client_exit_code'] == row['server_exit_code'] == 0
    assert [gate['event'] for gate in row['gates']] == ['connected', 'ready', 'measured', 'postcheck', 'done']
    assert row['gates'][-1]['normal_close_and_eof'] == 256
    peers = row['ownership']
    assert len(peers) == len({peer['port'] for peer in peers}) == 256
    assert len({peer['epoll_fd'] for peer in peers}) == 4
    assert len(row['plan']) == 32 and len({index for pair in row['plan'] for index in pair}) == 64
    assert all(count == 8 for count in Counter(peers[pair[0]]['epoll_fd'] for pair in row['plan']).values())
    assert all(count == 8 for count in Counter(peers[pair[1]]['epoll_fd'] for pair in row['plan']).values())
    for first, second in row['plan']:
        assert (peers[first]['epoll_fd'] == peers[second]['epoll_fd']) == (row['mode'] == 'local')
    result = row['result']
    assert result['expected_encoded_bytes'] == result['verified_deliveries'] * 1037
    assert row['measured_copied_bytes'] == (0 if row['mode'] == 'local' else result['expected_encoded_bytes'])
    assert sorted(row['worker_affinity']) == [[i] for i in range(4)]
    assert row['after']['fd_count'] == row['baseline']['fd_count']
    for key in ['war_connections', 'war_users', 'war_queued_bytes', 'war_buffered_bytes', 'war_retained_output_bytes', 'war_variable_bytes']:
        assert row['final_metrics'][key] == 0
    summary['verified_deliveries'] += result['verified_deliveries']
for mode in ['local', 'remote']:
    summary['cases'][mode] = {}
    for kind in ['control', 'array']:
        rows = [row for row in record['runs'] if row['mode'] == mode and row['kind'] == kind]
        summary['cases'][mode][kind] = {
            'median_deliveries_per_second': statistics.median(row['result']['deliveries_per_second'] for row in rows),
            'median_cpu_us_per_delivery': statistics.median(row['server_cpu_seconds'] / row['result']['verified_deliveries'] * 1e6 for row in rows),
            'p99_upper_ms': [row['result']['latency_p99_upper_ms'] for row in rows],
            'cpu_us_per_delivery': [row['server_cpu_seconds'] / row['result']['verified_deliveries'] * 1e6 for row in rows],
            'deliveries_per_second': [row['result']['deliveries_per_second'] for row in rows],
        }
    c, v = summary['cases'][mode]['control'], summary['cases'][mode]['array']
    summary['cases'][mode]['array_change_percent'] = {
        name: (v[name] / c[name] - 1) * 100 for name in ['median_deliveries_per_second', 'median_cpu_us_per_delivery']}
(a / 'cpp-placement-analysis.json').write_text(json.dumps(summary, indent=2) + '\n')
fig, axes = plt.subplots(1, 2, figsize=(12, 5), layout='constrained')
for axis, field, raw, scale, title, ylabel in [
    (axes[0], 'median_deliveries_per_second', 'deliveries_per_second', 1e6, 'Verified throughput', 'Million deliveries / second'),
    (axes[1], 'median_cpu_us_per_delivery', 'cpu_us_per_delivery', 1, 'Server CPU cost', 'CPU microseconds / delivery')]:
    for kind, offset, color, label in [('control', -.18, '#4C78A8', 'Accepted C++'), ('array', .18, '#F58518', 'Array prototype (unselected)')]:
        values = [summary['cases'][mode][kind][field] / scale for mode in ['local', 'remote']]
        bars = axis.bar([index + offset for index in range(2)], values, width=.34, label=label, color=color, alpha=.8)
        axis.bar_label(bars, fmt='%.3f', padding=4)
        for index, mode in enumerate(['local', 'remote']):
            dots = summary['cases'][mode][kind][raw]
            axis.scatter([index + offset + (n - 1.5) * .025 for n in range(4)], [value / scale for value in dots], color='black', s=17, zorder=3)
    axis.set(xticks=[0, 1], xticklabels=['Every pair on same worker', 'Every pair across workers'], title=title, ylabel=ylabel, ylim=(0, None))
    axis.margins(y=.18)
    axis.grid(axis='y', alpha=.2)
    axis.set_axisbelow(True)
axes[0].legend(fontsize=9, loc='lower left')
fig.suptitle('Controlled socket ownership · 32 private pairs · 256 connected peers · four pinned workers\n1 KiB / window16 · four fresh 8s trials per build and case · dots show every trial', fontsize=12)
for suffix in ['png', 'pdf']:
    fig.savefig(a / f'cpp-placement-comparison.{suffix}', dpi=180)
compressed = {}
for path in sorted(a.glob('cpp-placement-*')):
    if not path.is_file() or path.name == 'cpp-placement-archive.log':
        continue
    data, name = path.read_bytes(), path.name
    if path.suffix == '.json' and len(data) > 256000:
        name += '.gz'
        compressed[name] = {'sha256': sha(data), 'bytes': len(data)}
        data = gzip.compress(data, mtime=0)
    (out / name).write_bytes(data)
shutil.copyfile(root / 'cpp/placement-compare.py', out / 'placement-compare.py')
shutil.copytree(client / 'source', out / 'client-source', dirs_exist_ok=True)
shutil.copyfile(client / 'build.json', out / 'client-build.json')
shutil.copyfile(client / 'library-worktree.patch', out / 'client-library-worktree.patch')
lines = ['# Controlled worker ownership comparison', '',
         '**Outcome: retain the measurement tools; keep the accepted C++ runtime selected.**', '',
         'The earlier private comparisons could vary the retained-copy fraction between fresh SO_REUSEPORT processes. This study identifies live sockets externally and selects equivalent pair topologies without changing either relay executable.', '',
         '## Method and invariants', '',
         '- 256 real WebSocket sessions join one room. The client reports its local TCP ports and welcome IDs, then waits for the controller. All peers stay connected throughout measurement.',
         '- Linux `/proc/PID/net/tcp` maps each client port to an accepted socket inode. `/proc/PID/fd` maps the inode to a server descriptor, and each epoll fdinfo maps that descriptor to exactly one owner loop. All 256 mappings must exist, with exactly four owner groups.',
         '- Select 32 disjoint active pairs, balanced at eight senders and eight receivers per worker. In the local case each pair shares an epoll owner; in the remote case each source targets the next distinct owner. Unused peers remain idle. The mapping is checked again after measurement.',
         '- Four individually pinned server workers on CPUs0–3; one unchanged four-thread Rust load client allowed on CPUs8–15. Same 1 KiB cyclic templates and 16-frame windows in both arms. Every full encoded response, sender ID and sequence is checked before continuing.',
         '- Setup, topology discovery, template generation, postchecks and teardown are outside the CPU window. Ready/measured gates bound server-only process CPU. All four local trials per implementation have exactly zero retained-copy bytes; every remote trial has exactly 1,037 copied bytes per verified delivery. These are exact measured deltas, rather than a copy-path proxy.',
         '- After timing, every active pair routes a checked proof message. All 256 peers then complete a checked normal Close acknowledgement and raw TCP EOF. Every run retires all sessions, output/variable bytes, extra rooms and descriptors.',
         '- Four fresh eight-second trials per implementation and topology, with reversed implementation order on alternate repetitions. No builds, profiles or other workloads overlap ordinary timing.', '',
         '## Results', '',
         '| Topology | Build | Median verified deliveries/s | Median CPU us/delivery | Per-trial p99 upper ms |',
         '|---|---|---:|---:|---|']
for mode in ['local', 'remote']:
    for kind in ['control', 'array']:
        value = summary['cases'][mode][kind]
        lines.append(f"| {mode} | {kind} | {value['median_deliveries_per_second']:,.0f} | {value['median_cpu_us_per_delivery']:.3f} | {', '.join(f'{v:.3f}' for v in value['p99_upper_ms'])} |")
lines += ['', f"The 16 ordinary trials verify **{summary['verified_deliveries']:,} deliveries**. On the fully remote path, the array prototype improves median throughput by 13.00% and CPU/delivery by 9.32%. On the fully local path, throughput falls 2.36% and CPU/delivery rises 0.63%. The array wins throughput and CPU in every remote matched pair. Quantiles are client-observed histogram upper bounds, including windowing and scheduling, not isolated relay-hop times.", '',
          '![Every ordinary trial](cpp-placement-comparison.png)', '',
          'This establishes a real retained-message-path benefit under controlled topology. It does not override the rejected prototype\'s earlier mixed observer and 100,000-user latency evidence. The accepted source and executable stay unchanged. The next runtime change should preserve the cheap local path and demonstrate responsiveness under competing traffic and the full population before selection. These results do not diagnose reference-count/payload cache sharing; that hypothesis still needs separate evidence.', '',
          '## Reproduction and provenance', '',
          'Build the client with `cargo build --release --example placement`, then run:', '',
          '```sh',
          'python3 cpp/placement-compare.py \\',
          '  --server control=artifacts/candidates/cpp-modern/war-relay-cpp \\',
          '  --client target/release/examples/placement \\',
          '  --output artifacts/cpp-placement-new.json',
          '```', '',
          'Add `--server candidate=/absolute/path/to/another/war-relay-cpp` to compare another build in the same rotated workload. The frozen study executables are local artifacts, omitted from Git. Build the accepted runtime with `cpp/build.sh`; the array source and runtime patch are in the [previous ownership study](../2026-10-17-cpp-single-allocation/README.md). This archive depends explicitly on that hash-verified source archive. `client-build.json` records the exact executable, compiler, base commit, Cargo files and source hashes. Its existing library worktree diff is saved for reconstruction, and no Rust relay executable/service was changed. Strict release Clippy passes for the new client.', '',
          'The two one-second smoke topologies pass separately. An initial smoke could not launch the copied client because its executable mode had not been preserved; that setup failure and original runner are retained, excluded from all timings. Successful smoke and measured runners each match their recorded hashes. After the ordinary matrix, the reusable controller added explicit selector/pipe cleanup and rejection of empty traffic windows; both final one-second smoke topologies pass too. The exact original 16-trial runner is saved as `cpp-placement-controlled-runner.py`, separately from the reusable final `placement-compare.py`.', '',
          'Run `python3 docs/benchmarks/2026-10-17-cpp-placement/verify.py` to check this archive and its source dependency.']
(out / 'README.md').write_text('\n'.join(lines) + '\n')
verifier = '''from pathlib import Path
import gzip,hashlib,json,subprocess,sys
p=Path(__file__).resolve().parent;m=json.loads((p/'manifest.json').read_text());sha=lambda data:hashlib.sha256(data).hexdigest()
for name,want in m['files'].items():assert sha((p/name).read_bytes())==want,name
for name,r in m['compressed_originals'].items():
 data=gzip.decompress((p/name).read_bytes());assert sha(data)==r['sha256'] and len(data)==r['bytes'],name
dependency=p.parent/'2026-10-17-cpp-single-allocation'
assert sha((dependency/'manifest.json').read_bytes())==m['source_dependency_manifest_sha256']
subprocess.run([sys.executable,str(dependency/'verify.py')],check=True)
case=p/'cpp-placement-controlled.json.gz'
r=json.loads(gzip.decompress(case.read_bytes()) if case.exists() else (p/'cpp-placement-controlled.json').read_bytes())
assert r['passed'] and len(r['runs'])==16 and all(row['passed'] for row in r['runs'])
assert r['runner_sha256']==sha((p/'cpp-placement-controlled-runner.py').read_bytes())
for label,kind in [('control','control'),('array','candidate')]:assert r['server_sha256'][label]==json.loads((dependency/'source'/kind/'build.json').read_text())['binary_sha256']
final=json.loads((p/'cpp-placement-final-smoke.json').read_text());assert final['passed'] and final['runner_sha256']==sha((p/'placement-compare.py').read_bytes())
b=json.loads((p/'client-build.json').read_text());assert b['client_sha256']==r['client_sha256']
for name,want in b['source_sha256'].items():assert sha((p/'client-source'/name).read_bytes())==want,name
assert sha((p/'client-library-worktree.patch').read_bytes())==b['library_worktree_patch_sha256']
for name in ['cpp-placement-smoke','cpp-placement-smoke2']:
 s=json.loads((p/(name+'.json')).read_text());assert s['runner_sha256']==sha((p/'cpp-placement-smoke-runner.py').read_bytes())
print(json.dumps({'passed':True,'files':len(m['files']),'ordinary_trials':16,'source_dependency_verified':True}))
'''
(out / 'verify.py').write_text(verifier)
manifest = {'compressed_originals': compressed, 'source_dependency_manifest_sha256': sha((source_archive / 'manifest.json').read_bytes()),
            'files': {str(path.relative_to(out)): sha(path.read_bytes()) for path in sorted(out.rglob('*')) if path.is_file() and path.name != 'manifest.json'}}
(out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps(summary))
