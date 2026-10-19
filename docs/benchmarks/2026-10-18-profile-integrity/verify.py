"""Audit the saved failure reproduction and successful real profile offline."""
import gzip
import hashlib
import json
from pathlib import Path

root = Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected, name = line.split('  ', 1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest() == expected, name


def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes())


def rows(name):
    # unittest prefixes a failure marker before the second diagnostic JSON row.
    result = []
    for line in read(name).decode().splitlines():
        if line.startswith('{') or line.startswith('F{'):
            row = json.loads(line[line.index('{'):])
            if 'runner_exit' in row or 'rate' in row:
                result.append(row)
    return result


before = rows('profile-early-exit-before.log')
assert len(before) == 2
assert all(r['runner_exit'] == 0 and r['passed'] and not r['profile_created'] for r in before)
assert all(r['target_reaped'] for r in before)
after = rows('profile-failure-four-cases.log')
assert [r['mode'] for r in after] == ['early', 'during', 'missing', 'empty']
assert all(r['runner_exit'] != 0 and not r['passed'] and r['target_reaped'] for r in after)
assert all(r['profile']['exit_code'] == 0 for r in after)
assert all(len(r['child_exit_codes']) == 3 and all(type(c) is int for c in r['child_exit_codes']) for r in after)
assert all('profiler exited before' in r['error'] for r in after[:2])
assert all('profiler produced no data' in r['error'] for r in after[2:])
x = json.loads(read('profile-integrity-real.json'))
assert x['passed'] and x['correctness_passed'] and x['exit_codes'] == [0, 0, 0]
assert x['fanout_windows'] == {'completed': 2, 'offered': 2}
assert x['profile']['target'] == 'server' and x['profile']['exit_code'] == -2
profile = read('profile-integrity-real.perf.data')
assert x['profile']['data_bytes'] == len(profile) == 504052
assert x['profile']['data_sha256'] == hashlib.sha256(profile).hexdigest()
assert x['sha256']['runner'] == hashlib.sha256(read('selected-runner.py')).hexdigest()
assert x['csv_sha256'] == hashlib.sha256(read('profile-integrity-real.csv')).hexdigest()
for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
    assert x['final_metrics'][key] == 0, key
assert x['final_metrics']['war_channels'] == 1
report = read('profile-integrity-real-report.txt').decode()
assert 'Total Lost Samples: 0' in report and 'Samples: 53 ' in report and 'war-relay' in report
for name in ['profile-failure-four-cases.log', 'profile-integrity-nonprofile-gates.log', 'profile-integrity-output-tests.log', 'profile-integrity-timing-tests.log']:
    assert 'OK' in read(name).decode()
gates = rows('profile-integrity-nonprofile-gates.log')
assert [r['passed'] for r in gates] == [True, False, True]
assert all(r['correctness_passed'] for r in gates)
print('Verified two original false successes, four rejected profile failures with reaped targets, real perf data/hash/success and unchanged ordinary workload gates.')
