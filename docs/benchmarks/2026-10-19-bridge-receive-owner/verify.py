import hashlib, json, re
from pathlib import Path
root = Path(__file__).resolve().parent
receive = json.loads((root / 'receive.json').read_text())
assert receive['passed'] and receive['client_exit_code'] == 0
assert [x['payload_bytes'] for x in receive['observations']] == [0, 1, 24, 1024, 4096, 65536]
assert all(not x['receive_unique'] and not x['payload_reused'] and x['prefix_bytes'] >= 8 for x in receive['observations'])
cases = []
for name in ['control', 'early-drop']:
    receipt = json.loads((root / f'{name}.json').read_text())
    assert receipt['passed'] and receipt['verified_probes'] == 2048
    totals = []
    for capture in receipt['heap_profiler']['captures']:
        data = (root / f"{name}-{capture['phase']}.heap").read_bytes()
        assert hashlib.sha256(data).hexdigest() == capture['sha256']
        header = re.match(rb'heap profile:\s*(\d+):\s*(\d+) \[\s*(\d+):\s*(\d+)\]', data)
        values = tuple(map(int, header.groups()))
        rows = [tuple(map(int, m)) for m in re.findall(rb'^\s*(\d+):\s*(\d+) \[\s*(\d+):\s*(\d+)\] @', data, re.M)]
        assert tuple(map(sum, zip(*rows))) == values
        totals.append(values)
    cases.append(totals)
assert [x[:2] for x in cases[0]] == [(174, 96957), (3760, 3873293), (3764, 4401149), (180, 98045)]
assert [x[:2] for x in cases[0]] == [x[:2] for x in cases[1]]
for a, b in zip(cases[0], cases[1]):
    assert a[2] == b[2] and b[3] - a[3] == 3
print('PASS: six shared receive views; identical live and baseline-adjusted cumulative heap')
