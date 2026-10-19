"""Recompute selected/rejected helper sizing measurements and checksums."""
import gzip
import hashlib
import json
import statistics
from pathlib import Path
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
build = json.loads((ROOT / 'builds.json').read_text())
assert build['selected'] == 'narrow'
summary = json.loads((ROOT / 'summary.json').read_text())
assert len(summary) == 8
for item in summary:
    raw = [json.loads(line) for line in (ROOT / 'raw' / item['file']).read_text().splitlines()]
    assert len(raw) == 16
    computed = []
    for row in raw:
        assert len(row['ns_per_operation']) == 7 and all(t > 0 for t in row['ns_per_operation'])
        computed.append({**{k:v for k,v in row.items() if k != 'ns_per_operation'},
                         'median_ns': statistics.median(row['ns_per_operation'])})
        if item['phase'] == 'narrow' and item['file'].endswith('-narrow.jsonl') and not row['case'].startswith('uint'):
            assert row['capacity'] == row['encoded_len']
    assert computed == item['rows']
corpus = json.loads(gzip.decompress((ROOT / 'raw/input-corpus.json.gz').read_bytes()))['case']
assert corpus['passed'] and corpus['corpus_cases'] == 776 and corpus['scheduled_exchanges'] == 2311
assert corpus['server_sha256'] == build['native_binaries']['war-relay']
assert corpus['healthy_verified'] > 0
assert '84 passed; 0 failed' in (ROOT / 'raw/helper-sizing-library-tests.log').read_text()
assert '7 passed; 0 failed' in (ROOT / 'raw/helper-sizing-protocol-tests.log').read_text()
print('All checksums, eight process records, medians, exact capacities and corpus identity verified.')
