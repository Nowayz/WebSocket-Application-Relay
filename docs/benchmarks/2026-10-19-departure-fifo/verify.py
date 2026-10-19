#!/usr/bin/env python3
"""Offline provenance, data and patch checks; --check-local adds binary/source checks."""
import argparse
import gzip
import hashlib
import importlib.util
import json
from pathlib import Path
import subprocess
import tempfile
from analyze import read, summary

ROOT = Path(__file__).resolve().parent
CHANGED = ['src/relay.rs','src/relay/continuation_tests.rs','src/server.rs',
           'src/server/fairness_tests.rs','src/server/socket.rs']


def digest(data):
    return hashlib.sha256(data).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check-local', action='store_true')
    args = parser.parse_args()
    for name, expected in json.loads((ROOT/'sha256.json').read_text()).items():
        assert digest((ROOT/name).read_bytes()) == expected, name
    assert summary() == json.loads((ROOT/'summary.json').read_text())
    builds = {name: json.loads((ROOT/(name+'-build.json')).read_text())
              for name in ['control','measured','selected','prototype']}
    assert all(b['passed'] and b['cargo_artifact']['fresh'] is False for b in builds.values())
    for label in ['departure-control-first','departure-control-last','mixed-control-first','mixed-control-last']:
        receipt = read(label)
        identity = receipt.get('server_sha256') or receipt['sha256']['server']
        assert identity == builds['control']['binary_sha256']
    for label in ['departure-selected-first','departure-selected-repeat','eof-selected','mixed-selected-first','mixed-selected-repeat']:
        receipt = read(label)
        identity = receipt.get('server_sha256') or receipt['sha256']['server']
        assert identity == builds['measured']['binary_sha256']
    fixture = gzip.decompress((ROOT/'departure-fairness.py.gz').read_bytes())
    for name in ['departure-control-first','departure-control-last','departure-selected-first','departure-selected-repeat','eof-selected']:
        x = read(name)
        assert x['fixture_sha256'] == digest(fixture)
        for key in ['war_protocol_errors_total','war_write_errors_total','war_write_timeouts_total',
                    'war_close_failures_total','war_read_buffer_reservations_rejected_total',
                    'war_handshake_errors_total','war_slow_consumers_total','war_global_pressure_total']:
            assert int(x['after'][key]) == int(x['before'][key]), (name, key)
        delta = int(x['after']['war_transport_disconnects_total'])-int(x['before']['war_transport_disconnects_total'])
        assert delta == (4 if name == 'eof-selected' else 0)
    eof = read('eof-selected')
    assert eof['passed'] and eof['server_exit'] == 0 and eof['observed_eof_disconnects'] == 4
    assert eof['variant_sha256'] == digest((ROOT/'eof-check.py').read_bytes())
    assert [e['recipients'] for e in eof['events']] == [0,100001,100000,0]
    assert int(eof['after']['war_sessions']) == 100000 and int(eof['after']['war_queued_bytes']) == 0
    assert int(eof['after']['war_routing_copied_bytes_total']) == 0
    rejected = read('rejected-local-mixed')
    assert not rejected['passed']
    assert rejected['sha256']['server'] == builds['prototype']['binary_sha256']
    assert 'fanout departure changed' in (ROOT/'mixed-low-2-candidate-sidecar.err').read_text()
    overload = read('rejected-overload-control')
    assert not overload['passed'] and overload['correctness_passed']
    assert overload['fanout_windows'] == {'completed':40,'offered':64}
    with tempfile.TemporaryDirectory() as temp:
        temp = Path(temp)
        for path in CHANGED:
            base = gzip.decompress((ROOT/('base-'+path.replace('/','-')+'.gz')).read_bytes())
            target = temp/path
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(base)
            if path in ['src/relay.rs','src/server.rs','src/server/socket.rs']:
                assert digest(base) == builds['control']['source_sha256'][path]
        subprocess.run(['git','apply',str(ROOT/'changes.patch')],cwd=temp,check=True,capture_output=True)
        for path in CHANGED:
            selected = gzip.decompress((ROOT/('selected-'+path.replace('/','-')+'.gz')).read_bytes())
            assert (temp/path).read_bytes() == selected
            assert digest(selected) == builds['selected']['source_sha256'][path]
            if path != 'src/relay.rs':
                assert digest(selected) == builds['measured']['source_sha256'][path]
    old = gzip.decompress((ROOT/'measured-relay.rs.gz').read_bytes())
    assert digest(old) == builds['measured']['source_sha256']['src/relay.rs']
    assert old.replace(b'/// One immutable membership snapshot and one charged shared body. The box is\n/// allocated only on a publisher\'s first oversized room multicast and reused.\n',
                       b'/// One immutable membership snapshot and one charged shared body. Publisher\n/// slots reuse their box; retirement jobs belong to the shared work queue.\n') == gzip.decompress((ROOT/'selected-src-relay.rs.gz').read_bytes())
    equivalence = json.loads((ROOT/'elf-equivalence.json').read_text())
    assert equivalence['passed']
    assert equivalence['measured_sha256'] == builds['measured']['binary_sha256']
    assert equivalence['selected_sha256'] == builds['selected']['binary_sha256']
    assert equivalence['measured'].keys() == equivalence['selected'].keys()
    for name, a in equivalence['measured'].items():
        for key, value in a.items():
            if name != '.note.gnu.build-id' or key != 'sha256':
                assert value == equivalence['selected'][name][key], (name, key)
    if args.check_local:
        for name in ['control','measured','selected']:
            assert digest(Path(builds[name]['binary']).read_bytes()) == builds[name]['binary_sha256']
        work = ROOT.parents[2]
        for path in CHANGED:
            assert digest((work/path).read_bytes()) == builds['selected']['source_sha256'][path]
        spec = importlib.util.spec_from_file_location('elf_equivalence',ROOT/'elf-equivalence.py')
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        assert module.compare(Path(builds['measured']['binary']),Path(builds['selected']['binary'])) == equivalence
    print('PASS: hashes, raw comparisons, notification/resource gates, rejected experiments, patch reconstruction, build/source identities, ELF equivalence')


if __name__ == '__main__':
    main()
