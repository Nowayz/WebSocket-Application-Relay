"""Exact heap-header accounting with executable-address attribution."""
import hashlib
import json
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]
result = {'scope': 'Requested live and cumulative server heap under gperftools replacement allocator, not default-allocator RSS or ordinary timing. Allocation rows sum to each header. Native return addresses use the captured executable mapping and subtract one before symbolization. Generic allocator frames are retained without claiming their callers.', 'cases': {}}
pattern = re.compile(r'\s*(\d+):\s*(\d+)\s*\[\s*(\d+):\s*(\d+)\]\s*@\s*(.*)')
for kind in ['control', 'candidate']:
    receipt_path = ROOT / f'artifacts/connection-phase-lease-20261019-heap-{kind}.json'
    receipt = json.loads(receipt_path.read_text())
    assert receipt['passed'] and receipt['profile_diagnostic']
    command = receipt['server_command']
    binary = Path(next(part for part in command if part.endswith('/war-relay') or part.endswith('/war-relay-cpp')))
    assert hashlib.sha256(binary.read_bytes()).hexdigest() == receipt['sha256']['server']
    captures = []
    addresses = set()
    for capture in receipt['heap_profiler']['captures']:
        data = Path(capture['path']).read_bytes()
        assert hashlib.sha256(data).hexdigest() == capture['sha256']
        lines = data.decode().splitlines()
        mappings = [line.split() for line in lines if str(binary) in line]
        base = int(next(fields[0].split('-')[0] for fields in mappings if int(fields[2], 16) == 0), 16)
        segments = [tuple(int(a, 16) for a in fields[0].split('-')) for fields in mappings]
        rows = []
        for line in lines[1:]:
            match = pattern.fullmatch(line)
            if not match:
                continue
            offsets = [int(a, 16) - base - 1 for a in match[5].split()
                       if any(lo <= int(a, 16) < hi for lo, hi in segments)]
            addresses.update(offsets)
            rows.append({'live_objects': int(match[1]), 'live_bytes': int(match[2]),
                         'allocated_objects': int(match[3]), 'allocated_bytes': int(match[4]),
                         'offsets': [hex(a) for a in offsets]})
        header = pattern.fullmatch(lines[0].removeprefix('heap profile:'))
        assert header is not None
        keys = ['live_objects', 'live_bytes', 'allocated_objects', 'allocated_bytes']
        totals = dict(zip(keys, [int(header[i]) for i in range(1, 5)]))
        assert totals == {k: sum(row[k] for row in rows) for k in keys}
        captures.append({'phase': capture['phase'], 'heap_sha256': capture['sha256'],
                         'base': hex(base), 'totals': totals, 'rows': rows})
    run = subprocess.run(['addr2line', '-a', '-f', '-C', '-i', '-e', str(binary)],
                         input=''.join(hex(a) + '\n' for a in sorted(addresses)),
                         capture_output=True, text=True, check=True)
    symbols = {}
    current = None
    for line in run.stdout.splitlines():
        if re.fullmatch(r'0x[0-9a-fA-F]+', line):
            current = hex(int(line, 16)); symbols[current] = []
        elif current is not None:
            symbols[current].append(line)
    for capture in captures:
        groups = {}
        for row in capture['rows']:
            names = [s for address in row['offsets'] for s in symbols.get(address, [])]
            function = next((s for s in names if s != '??' and ':?' not in s and ':0' not in s), 'outside executable / unresolved')
            group = groups.setdefault(function, dict.fromkeys(keys, 0))
            for k in keys:
                group[k] += row[k]
        capture['groups'] = dict(sorted(groups.items(), key=lambda item: item[1]['live_bytes'], reverse=True))
    result['cases'][kind] = {'binary': str(binary), 'binary_sha256': receipt['sha256']['server'],
                            'receipt_sha256': hashlib.sha256(receipt_path.read_bytes()).hexdigest(),
                            'symbols': symbols, 'captures': captures}
output = ROOT / 'artifacts/connection-phase-lease-20261019-heap-analysis.json'
output.write_text(json.dumps(result, indent=2) + '\n')
for kind, case in result['cases'].items():
    print(kind)
    for capture in case['captures']:
        print(capture['phase'], capture['totals'])
        if capture['phase'] == 'population_ready':
            for name, counts in list(capture['groups'].items())[:8]:
                print(name, counts)
