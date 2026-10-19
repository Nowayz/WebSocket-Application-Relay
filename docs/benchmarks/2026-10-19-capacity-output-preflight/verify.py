import csv
import gzip
import hashlib
import io
import json
from pathlib import Path

root = Path(__file__).resolve().parent
meta = json.loads((root / 'provenance.json').read_text())
assert meta['runtime_changed'] is False
for name, digest in meta['files_sha256'].items():
    assert hashlib.sha256((root / name).read_bytes()).hexdigest() == digest, name


def raw(name):
    return gzip.decompress((root / (name + '.gz')).read_bytes())


def decoded(name):
    return json.loads(raw(name))

source = (root / 'capacity.py').read_text()
assert source.index('_preflight_outputs(args)\n    assert resource') < source.index('server = subprocess.Popen')
assert 'os.path.lexists(path)' in source and 'identity = path.resolve()' in source
assert 'output paths overlap' in source and "name.rpartition('.')" in source
assert 'FAILED (failures=16)' in (root / 'before.log').read_text()
assert 'Ran 10 tests' in (root / 'tests.log').read_text() and '\nOK\n' in (root / 'tests.log').read_text()
old = decoded('counterfactual.json')
assert old['passed'] and old['server_exit_code'] == old['client_exit_code'] == 0
assert old['runner_sha256'] == hashlib.sha256(raw('original-capacity.py')).hexdigest()
assert raw('counterfactual-users.csv') != meta['original_sentinels']['csv'].encode()
assert raw('counterfactual-windows.json') != meta['original_sentinels']['timeline'].encode()
assert raw('counterfactual-users.csv').startswith(b'room,member,id,samples,')
assert len(decoded('counterfactual-windows.json')['windows']) == 1
build = decoded('client-build.json')
assert build['passed'] and build['cargo_artifact']['fresh'] is False
for mode in ['exact', 'hist']:
    x = decoded(mode + '.json')
    assert x['passed'] and x['server_exit_code'] == x['client_exit_code'] == 0
    assert not x.get('server_forced_kill') and not x.get('client_forced_kill')
    assert x['runner_sha256'] == meta['files_sha256']['capacity.py']
    assert x['client_sha256'] == build['binary_sha256']
    assert x['measurement']['application_deliveries'] == 64
    audience = list(csv.DictReader(io.StringIO(raw(mode + '-users.csv').decode())))
    assert len(audience) == len({r['id'] for r in audience}) == 16
    assert all(int(r['samples']) == 4 for r in audience)
    for event in x['events']:
        for key, value in event['exact_counter_checks'].items():
            assert event['metrics'][key] == value
    done = next(e for e in x['events'] if e['event'] == 'done')
    for key in ['war_connections', 'war_sessions', 'war_queued_bytes', 'war_variable_bytes', 'war_read_buffer_reserved_bytes', 'war_output_queue_capacity_bytes', 'war_routing_copied_bytes_total']:
        assert done['metrics'][key] == 0
    assert done['server']['fd_count'] == x['baseline']['server']['fd_count']
    if mode == 'exact':
        assert x['args']['user_latency_output'].endswith('users.')
        assert len(decoded('exact-windows.json')['windows']) == 1
    print(f'{mode}: 16 distinct users, four samples each, 64 deliveries, clean exits and checked retirement')
print('Capacity output preservation evidence verified; demonstrated old overwrite and valid selected outputs.')
