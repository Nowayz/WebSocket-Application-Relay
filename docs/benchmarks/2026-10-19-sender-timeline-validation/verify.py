#!/usr/bin/env python3
"""Replay timing summaries and verify their evidence without starting a relay."""
import gzip
import hashlib
import importlib.util
import json
import math
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def main():
    for line in (ROOT / 'SHA256SUMS').read_text().splitlines():
        expected, name = line.split('  ', 1)
        assert hashlib.sha256((ROOT / name).read_bytes()).hexdigest() == expected, name
    old = load('old_mixed', ROOT / 'original-mixed-capacity.py')
    new = load('new_mixed', ROOT / 'mixed-capacity.py')
    before = (ROOT / 'original-mixed-capacity.py').read_text()
    after = (ROOT / 'mixed-capacity.py').read_text()
    assert before[before.index('def main():'):] == after[after.index('def main():'):]
    fixture = {'windows': [{'intended_second': 0, 'pre_send_delay': {'max_ms': float('nan')}}]}
    bad = old.generator_lag(fixture, 4)
    assert math.isnan(bad['maximum_pre_send_ms']) and bad['available'] and not bad['exceeded_one_period']
    try:
        new.generator_lag(fixture, 4)
    except ValueError:
        pass
    else:
        raise AssertionError('invalid evidence was accepted')
    records = json.loads((ROOT / 'generator-lag-replay.json').read_text())
    assert len(records) == 9
    for case in records:
        values = {}
        for key in ['receipt', 'timeline']:
            raw = gzip.decompress((ROOT / 'raw' / (Path(case[key]).name + '.gz')).read_bytes())
            assert hashlib.sha256(raw).hexdigest() == case[key + '_sha256']
            values[key] = json.loads(raw)
        assert values['receipt']['passed'] and values['receipt']['correctness_passed']
        assert values['receipt']['args']['users'] == 100000
        assert old.generator_lag(values['timeline'], 4) == new.generator_lag(values['timeline'], 4) == case['summary'] == values['receipt']['generator_lag']
    assert b'failures=15, errors=3' in (ROOT / 'generator-lag-before-tests.log').read_bytes()
    live = (ROOT / 'generator-lag-load-tests.log').read_text()
    outcomes = [json.loads(line) for line in live.splitlines() if line.startswith('{')]
    assert [row['rate'] for row in outcomes] == [1, 1000000, 0]
    assert [row['passed'] for row in outcomes] == [True, False, True]
    assert [row['exit_code'] for row in outcomes] == [0, 1, 0]
    assert all(row['correctness_passed'] for row in outcomes)
    for test in ['test-generator-lag.py', 'test-fanout-window-timing.py']:
        subprocess.run([sys.executable, str(ROOT / test)], check=True, capture_output=True)
    print('PASS: bad-evidence rejection, unchanged measured main, nine exact summary replays and recorded offered-load outcomes')


if __name__ == '__main__':
    main()
