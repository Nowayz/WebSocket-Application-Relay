"""Audit invalid-clock rejection and valid retained-data compatibility offline."""
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path
import sys
import tempfile

sys.dont_write_bytecode = True
ROOT = Path(__file__).resolve().parent
for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
build = json.loads((ROOT / 'build.json').read_text())
for name, expected in build['source_sha256'].items():
    assert hashlib.sha256((ROOT / 'raw' / Path(name).name).read_bytes()).hexdigest() == expected


def decoded(name):
    return gzip.decompress((ROOT / 'raw' / (name + '.gz')).read_bytes()).decode()


before = decoded('latency-clock-before.log')
assert 'failures=4' in before and 'ValueError not raised' in before
assert 'Ran 9 tests' in decoded('latency-clock-after.log') and 'OK' in decoded('latency-clock-after.log')
cli = json.loads(decoded('latency-clock-cli.json'))
assert cli['passed'] and cli['exit_code'] == 2 and not cli['outputs_created']
assert 'intended-send latency precedes actual-send latency' in cli['stderr']


def module(name, source):
    spec = importlib.util.spec_from_file_location(name, ROOT / 'raw' / source)
    result = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(result)
    return result


old = module('before', 'previous-plotter.py')
new = module('after', 'plot-user-latency-comparison.py')
receipt = json.loads(decoded('latency-clock-valid.json'))
assert receipt['passed'] and len(receipt['cases']) == 16
with tempfile.TemporaryDirectory() as directory:
    invalid = Path(directory) / 'invalid.csv'
    invalid.write_text(decoded('latency-clock-invalid.csv'))
    try:
        new.load(invalid, statistic='max')
    except ValueError:
        pass
    else:
        raise AssertionError('inverted fixture accepted')
    for case in receipt['cases']:
        name = Path(case['csv']).name
        archive = ('2026-10-18-simd-text-validation' if name.startswith('simd-text-')
                   else '2026-10-18-window-timeout-screen')
        raw = gzip.decompress((ROOT.parent / archive / 'raw' / (name + '.gz')).read_bytes())
        assert hashlib.sha256(raw).hexdigest() == case['csv_sha256']
        path = Path(directory) / 'users.csv'
        path.write_bytes(raw)
        a = old.load(path, statistic=case['statistic'])
        b = new.load(path, statistic=case['statistic'])
        assert a == b and len(b[1]) == case['users'] and case['unchanged']
        assert old.summarize(*a, [5, 50], statistic=case['statistic']) == new.summarize(*b, [5, 50], statistic=case['statistic'])
print('Checksums, original failures, nine focused tests, CLI rejection and all 16 retained-data equivalence cases verified.')
