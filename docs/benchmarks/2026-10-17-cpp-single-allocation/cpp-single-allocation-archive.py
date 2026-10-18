from pathlib import Path
import csv
import gzip
import hashlib
import json
import os
import shutil
import statistics
import subprocess
import urllib.request

root = Path.cwd()
artifacts = root / 'artifacts'
out = root / 'docs/benchmarks/2026-10-17-cpp-single-allocation'
out.mkdir(parents=True, exist_ok=True)
sha = lambda data: hashlib.sha256(data).hexdigest()
names = ['first', 'one-worker', 'small', 'large', 'mixed', 'capacity', 'capacity-confirmation']
records = {name: json.loads((artifacts / f'cpp-single-allocation-{name}.json').read_text()) for name in names}
assert all(record['passed'] for record in records.values())
builds = {}
for kind in ['control', 'candidate']:
    source = artifacts / 'candidates' / f'cpp-single-allocation-{kind}'
    build = json.loads((source / 'build.json').read_text())
    assert sha((source / 'war-relay-cpp').read_bytes()) == build['binary_sha256']
    assert sha((source / 'libwar-relay.a').read_bytes()) == build['library_sha256']
    for name, digest in build['source_sha256'].items():
        assert sha((source / name).read_bytes()) == digest, (kind, name)
    builds[kind] = build
for record in records.values():
    assert record['server_sha256'] == {kind: build['binary_sha256'] for kind, build in builds.items()}
    for row in record['runs']:
        assert row['passed'] and row['server_exit_code'] == 0
        for key in ['war_connections', 'war_users', 'war_queued_bytes', 'war_variable_bytes', 'war_buffered_bytes', 'war_retained_output_bytes']:
            assert row['final_metrics'][key] == 0, (row['kind'], key)
        assert row['final_metrics']['war_channels'] == 1
population = records['capacity']['runs'] + records['capacity-confirmation']['runs']
assert len(population) == 8
for row in population:
    data = (root / row['csv']).read_bytes()
    assert sha(data) == row['csv_sha256']
    users = list(csv.DictReader(data.decode().splitlines()))
    assert len(users) == len({user['id'] for user in users}) == 100000
    assert all(int(user['samples']) == 120 for user in users)
    phases = {gate['event']: gate for gate in row['gates']}
    counts = phases['measured']['details']['counts']
    assert counts['sent_messages'] == counts['deliveries'] == 12000000
    assert phases['done']['counts']['normal_closes'] == 100000
    assert phases['done']['counts']['departure_checks'] == 750000
    assert row['after']['fd_count'] == row['baseline']['fd_count']
corpus = json.loads((artifacts / 'cpp-single-allocation-corpus.json').read_text())
assert corpus['passed'] and corpus['cases'] == 776 and corpus['exchanges'] == 2311 and corpus['server_exit_code'] == 0
diagnostic = json.loads((artifacts / 'cpp-single-allocation-diagnostic.json').read_text())
assert all(row['candidate_allocations'] == 1 for row in diagnostic)
assert all(row['control_allocations'] == (2 if row['body_bytes'] > 15 else 1) for row in diagnostic)
accepted = artifacts / 'candidates/cpp-modern'
assert sha((accepted / 'war-relay-cpp').read_bytes()) == builds['control']['binary_sha256']
assert sha((accepted / 'libwar-relay.a').read_bytes()) == builds['control']['library_sha256']
for name, digest in builds['control']['source_sha256'].items():
    assert sha((root / name).read_bytes()) == digest, name
small = {}
for name in ['first', 'one-worker', 'small', 'large']:
    for mode in sorted({row['mode'] for row in records[name]['runs']}):
        key = f'{name}/{mode}'
        small[key] = {}
        for kind in ['control', 'candidate']:
            rows = [row for row in records[name]['runs'] if row['kind'] == kind and row['mode'] == mode]
            small[key][kind] = {
                'trials': len(rows),
                'median_deliveries_per_second': statistics.median(row['result']['deliveries_per_second'] for row in rows),
                'median_cpu_us_per_delivery': statistics.median(row['server_cpu_seconds'] / row['result']['verified_deliveries'] * 1e6 for row in rows),
                'p99_upper_ms': [row['result']['latency_p99_upper_ms'] for row in rows],
                'retained_copy_proxy': [row.get('cpp_retained_copy_proxy') for row in rows],
            }
cpu = {kind: sum(row['server_cpu_seconds'] for row in population if row['kind'] == kind) for kind in ['control', 'candidate']}
summary = {
    'decision': 'Rejected. Allocation count improves, but modest mixed CPU results and competing-traffic latency do not justify replacing the accepted ownership representation.',
    'passed': True, 'timed_population_probes': 96000000,
    'ordinary_small_population_trials': sum(len(records[name]['runs']) for name in ['first', 'one-worker', 'small', 'large', 'mixed']),
    'population_cpu_seconds': cpu,
    'population_cpu_change_percent': (cpu['candidate'] / cpu['control'] - 1) * 100,
    'small_population': small,
    'corpus': {'cases': 776, 'exchanges': 2311},
    'allocation_diagnostic': diagnostic,
    'restored_binary_sha256': builds['control']['binary_sha256'],
    'restored_library_sha256': builds['control']['library_sha256'],
    'candidate_binary_sha256': builds['candidate']['binary_sha256'],
    'sanitizers': 'Not rerun for this rejected prototype; no runtime change is being selected. Prior accepted-build sanitizer evidence remains separate.',
}
(artifacts / 'cpp-single-allocation-analysis.json').write_text(json.dumps(summary, indent=2) + '\n')
cmd = ['python3', 'artifacts/cpp-single-allocation-plot.py', '--subtitle', 'Queued-body allocation experiment · 100,000 users · 400k probes/s · 30s per trial · prototype rejected']
for study in ['capacity', 'capacity-confirmation']:
    for row in records[study]['runs']:
        label = f"{'Initial' if study == 'capacity' else 'Confirm'} {row['kind']} {row['repeat']}"
        cmd += ['--case', label + '=' + row['csv']]
cmd += ['--output', 'artifacts/cpp-single-allocation-user-latency']
subprocess.run(cmd, check=True)
manual = json.loads((artifacts / 'reactor-manual-services.json').read_text())
native_pid = manual['pids'][0]
native_exe = Path(f'/proc/{native_pid}/exe')
selected = artifacts / 'candidates/observer-quantum/war-relay'
assert sha(native_exe.read_bytes()) == sha(selected.read_bytes())
with urllib.request.urlopen('http://127.0.0.1:1339/readyz', timeout=3) as response:
    assert response.status == 200
machine = {'uname': list(os.uname()), 'manual_native_pid': native_pid,
           'manual_native_sha256': sha(native_exe.read_bytes()), 'manual_native_ready': True,
           'compiler': builds['candidate']['compiler'], 'stdlib_header': '/usr/include/c++/16/bits/shared_ptr_base.h',
           'restoration_checked': True}
(artifacts / 'cpp-single-allocation-machine.json').write_text(json.dumps(machine, indent=2) + '\n')
compressed = {}
for path in sorted(artifacts.glob('cpp-single-allocation-*')):
    if path.name == 'cpp-single-allocation-archive.log':
        continue
    if not path.is_file() or path.suffix not in {'.py', '.cpp', '.json', '.md', '.log', '.csv', '.png', '.pdf'}:
        continue
    data = path.read_bytes()
    name = path.name
    if path.suffix == '.csv' or (path.suffix in {'.json', '.log'} and len(data) > 256000):
        name += '.gz'
        compressed[name] = {'sha256': sha(data), 'bytes': len(data)}
        data = gzip.compress(data, mtime=0)
    (out / name).write_bytes(data)
for kind in ['control', 'candidate']:
    source = artifacts / 'candidates' / f'cpp-single-allocation-{kind}'
    destination = out / 'source' / kind
    destination.mkdir(parents=True, exist_ok=True)
    shutil.copytree(source / 'cpp', destination / 'cpp', dirs_exist_ok=True)
    for name in ['build.json', 'runtime.patch', 'base_revision']:
        if (source / name).exists():
            shutil.copyfile(source / name, destination / name)
shutil.copyfile(root / 'scripts/plot-user-latency-comparison.py', out / 'plot-user-latency-comparison.py')
lines = ['# Single-allocation queued-body experiment', '',
         '**Decision: reject the prototype and restore the accepted modern C++ runtime.**', '',
         'The candidate replaces `shared_ptr<const string>` with `make_shared_for_overwrite<char[]>`, copies a queued body once, and shares it across recipients. A full `size_t` body length occupies a union with close metadata, retaining the 40-byte delivery record. Borrowed local output, quotas, recipient ordering and accounting are unchanged. Sources derive from commit `02aea99`; both complete source trees, build records and the candidate patch are saved below.', '',
         '## Allocation evidence', '',
         'The separate allocation diagnostic escapes construction through noinline functions, keeps assertions enabled in the optimized build, and checks exact binary bytes plus continued shared ownership after the first owner retires. At a 1,037-byte encoded body, allocation count falls from two to one; requested bytes fall only from 1,086 to 1,084. Bodies of 15 bytes or less already used one string allocation, and some consume more requested bytes in the candidate. This diagnostic establishes allocation behavior, not end-to-end latency or allocator-wide memory use.', '',
         'An initial diagnostic had assertions compiled out and allowed candidate construction to be eliminated. Its observations are explicitly marked invalid in `cpp-single-allocation-diagnostic-elided-not-valid.json`; they are not used as evidence.', '',
         '## Ordinary guards', '',
         'All trials are fresh, sequential and rotated. No build, profiler, sanitizer or other benchmark overlaps ordinary timing. Four-worker 1 KiB private/fanout cases use four eight-second trials per implementation. One-worker, 8-byte private, churn, and 64 KiB private guards use two six-second trials per implementation. Churn has its fixed 16-byte application payload even in the small-message matrix. CPU in these guards includes setup/cleanup. Histogram p99s include client windowing.', '',
         '| Workload | Control deliveries/s | Candidate deliveries/s | Control CPU us/delivery | Candidate CPU us/delivery |',
         '|---|---:|---:|---:|---:|']
for key, value in small.items():
    c, v = value['control'], value['candidate']
    lines.append(f"| {key} | {c['median_deliveries_per_second']:,.0f} | {v['median_deliveries_per_second']:,.0f} | {c['median_cpu_us_per_delivery']:.3f} | {v['median_cpu_us_per_delivery']:.3f} |")
lines += ['',
          'The initial private medians improve, but connection placement changes the retained-copy fraction between processes. The recorded copy proxy includes prefix fallback, local queued work and teardown notifications; it is not a socket-ownership census or a directly comparable Rust metric. One-worker private traffic has zero retained-copy bytes and essentially unchanged CPU cost.', '',
          '## 100,000-user latency and CPU', '',
          'Two independently named studies each run control/candidate then candidate/control. Eight fresh processes verify **96 million timed probes**: 100,000 unique users, 120 probes per user, 1 KiB payloads, 16-member rooms, 400k probes/s for 30 seconds, eight pinned server workers. CPU is measured between ready/measured gates, excluding setup, CSV work and retirement. RSS is sampled process memory with all users present, excluding client and kernel TCP memory.', '',
          '| Study/pair | Control CPU-s | Candidate CPU-s | CPU change | Control/candidate p99 of user p99 ms | Control/candidate worst user p99 ms | Control/candidate worst packet ms |',
          '|---|---:|---:|---:|---|---|---|']
for study in ['capacity', 'capacity-confirmation']:
    for repeat in [1, 2]:
        c = next(row for row in records[study]['runs'] if row['kind'] == 'control' and row['repeat'] == repeat)
        v = next(row for row in records[study]['runs'] if row['kind'] == 'candidate' and row['repeat'] == repeat)
        lines.append(f"| {study}/{repeat} | {c['server_cpu_seconds']:.2f} | {v['server_cpu_seconds']:.2f} | {(v['server_cpu_seconds']/c['server_cpu_seconds']-1)*100:+.2f}% | {c['p99_ms']['p99']:.3f} / {v['p99_ms']['p99']:.3f} | {c['p99_ms']['max']:.3f} / {v['p99_ms']['max']:.3f} | {c['max_ms']['max']:.3f} / {v['max_ms']['max']:.3f} |")
lines += ['', f"Aggregate measured CPU is {cpu['control']:.2f} versus {cpu['candidate']:.2f} seconds ({summary['population_cpu_change_percent']:+.2f}%). One pair reverses both CPU and latency results. The last candidate run has a 19.405 ms worst packet versus 6.628 ms for its paired control. These finite runs do not establish a consistent responsiveness gain; neither isolated maxima nor the earlier favorable cohort are treated as a universal result.", '',
          '![All eight per-user distributions](cpp-single-allocation-user-latency.png)', '',
          '## Competing traffic', '',
          'Four-worker, two six-second trials per implementation: 512 authenticated global observers, one private pair, eight churn workers, two native-CBOR inspectors every 10 ms, 64 idle rooms and 1 KiB fanout. The single private pair is placement-sensitive. Percentiles below are histogram upper bounds.', '',
          '| Kind/trial | Observer deliveries/s | Private p99 ms | Churn p99 ms | Inspection p99 ms | Fanout p99 ms |',
          '|---|---:|---:|---:|---:|---:|']
for row in records['mixed']['runs']:
    r = row['result']
    lines.append(f"| {row['kind']} {row['repeat']} | {r['fanout']['per_second']:,.0f} | {r['healthy']['p99_upper_ms']:.3f} | {r['churn']['p99_upper_ms']:.3f} | {r['inspection']['p99_upper_ms']:.3f} | {r['fanout']['p99_upper_ms']:.3f} |")
lines += ['',
          'Candidate private throughput is lower in both trials; fanout p99 worsens by one bucket in one trial, and inspection p99 worsens by one bucket in the other. Placement and short-run variability prevent attributing each difference solely to the representation, but this is insufficient evidence to select a latency-sensitive replacement. Halving one allocation count did not establish a broadly better relay.', '',
          '## Verification, restoration and next investigation', '',
          '- 36 small-population ordinary trials pass, plus eight complete population trials. Corpus: 776 cases / 2,311 exchanges. All final connection/user/output/variable gauges are zero; every population run returns file descriptors to its exact baseline and verifies 100,000 normal closes plus 750,000 departure checks.',
          '- The candidate was rejected before a new sanitizer campaign; the accepted build\'s existing ASan/UBSan/TSan evidence remains separate. No rejected runtime change is shipped or selected.',
          f"- Accepted executable restored and verified: `{builds['control']['binary_sha256']}`.",
          f"- Accepted static library restored and verified: `{builds['control']['library_sha256']}`.",
          f"- Rejected executable: `{builds['candidate']['binary_sha256']}`.",
          '- Build-source hashes are checked against restored working sources. The selected manual Rust executable and readiness endpoint are unchanged. Existing Rust experiment edits are preserved; no push, deployment or manual service replacement occurs.',
          '- The local libstdc++16 implementation places array shared-ownership metadata after the array. Sharing a cache line between reference counts and payload tail is a possible follow-up hypothesis, not a diagnosed cause of these timings. Investigate cache placement and obtain a controlled worker-ownership workload before proposing another allocation/layout change.',
          '- Full sources, all raw trials, compressed user census CSVs, allocation diagnostic, build provenance, plot helper and this archive creator are retained. Binaries remain local outside Git. Compressed originals have independent uncompressed SHA-256 hashes.', '',
          'Verify: `python3 docs/benchmarks/2026-10-17-cpp-single-allocation/verify.py`.']
(out / 'README.md').write_text('\n'.join(lines) + '\n')
verifier = '''from pathlib import Path
import gzip,hashlib,json
p=Path(__file__).resolve().parent;m=json.loads((p/'manifest.json').read_text());digest=lambda data:hashlib.sha256(data).hexdigest()
for name,want in m['files'].items():assert digest((p/name).read_bytes())==want,name
for name,record in m['compressed_originals'].items():
 data=gzip.decompress((p/name).read_bytes());assert digest(data)==record['sha256'] and len(data)==record['bytes'],name
for kind in ['control','candidate']:
 b=json.loads((p/'source'/kind/'build.json').read_text())
 for name,want in b['source_sha256'].items():assert digest((p/'source'/kind/name).read_bytes())==want,(kind,name)
for name in ['first','one-worker','small','large','mixed','capacity','capacity-confirmation']:
 r=json.loads((p/('cpp-single-allocation-'+name+'.json')).read_text());assert r['passed']
 assert all(row['passed'] and row['server_exit_code']==0 for row in r['runs'])
 for kind,b in r['server_sha256'].items():assert b==json.loads((p/'source'/kind/'build.json').read_text())['binary_sha256']
print(json.dumps({'passed':True,'files':len(m['files']),'compressed_originals':len(m['compressed_originals']),'source_sets':['control','candidate']}))
'''
(out / 'verify.py').write_text(verifier)
manifest = {'decision': summary['decision'], 'compressed_originals': compressed,
            'files': {str(path.relative_to(out)): sha(path.read_bytes()) for path in sorted(out.rglob('*')) if path.is_file() and path.name != 'manifest.json'}}
(out / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')
print(json.dumps({key: value for key, value in summary.items() if key not in ['small_population', 'allocation_diagnostic']}))
