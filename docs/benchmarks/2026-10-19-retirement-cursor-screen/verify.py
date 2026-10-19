#!/usr/bin/env python3
import argparse,gzip,hashlib,json,subprocess,tempfile
from pathlib import Path
from analyze import read,summary
ROOT=Path(__file__).resolve().parent

def digest(data):return hashlib.sha256(data).hexdigest()

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--check-local',action='store_true');args=parser.parse_args()
    for name,expected in json.loads((ROOT/'sha256.json').read_text()).items():
        assert digest((ROOT/name).read_bytes())==expected,name
    assert summary()==json.loads((ROOT/'summary.json').read_text())
    builds={kind:json.loads((ROOT/(kind+'-build.json')).read_text()) for kind in ['control','candidate']}
    assert all(b['passed'] and b['cargo_artifact']['fresh'] is False for b in builds.values())
    for name in ['pressure-control-first','pressure-candidate-first','pressure-1-candidate','pressure-2-control',
                 'mixed-1-control','mixed-2-candidate','mixed-3-candidate','mixed-4-control',
                 'departure-1-control','departure-2-candidate','departure-3-candidate','departure-4-control',
                 'private-1-control','private-2-candidate','private-3-candidate','private-4-control']:
        x=read(name);kind='control' if 'control' in name else 'candidate'
        actual=x.get('server_sha256') or x['sha256']['server']
        assert actual==builds[kind]['binary_sha256'],name
    changed=['src/relay.rs','src/relay/continuation_tests.rs']
    with tempfile.TemporaryDirectory() as temp:
        temp=Path(temp)
        for path in changed:
            base=gzip.decompress((ROOT/('base-'+path.replace('/','-')+'.gz')).read_bytes())
            assert digest(base)==builds['control']['source_sha256'][path]
            target=temp/path;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(base)
        subprocess.run(['git','apply',str(ROOT/'UNSELECTED-candidate.patch')],cwd=temp,check=True,capture_output=True)
        for path in changed:
            candidate=gzip.decompress((ROOT/('candidate-'+path.replace('/','-')+'.gz')).read_bytes())
            assert (temp/path).read_bytes()==candidate and digest(candidate)==builds['candidate']['source_sha256'][path]
    fixture=(ROOT/'fixture.py').read_bytes()
    default,pressure=read('fixture-default'),read('fixture-pressure')
    for x in [default,pressure]:
        assert x['fixture_sha256']==digest(fixture)
        assert x['server_sha256']==builds['control']['binary_sha256']
        assert x['server_exit']==0 and not x['server_forced_kill']
    assert default['passed'] and default['queue_budget_bytes'] is None
    assert '--total-queue-bytes' not in default['server_command']
    assert not pressure['passed'] and pressure['queue_budget_bytes']==262144
    assert int(pressure['failure_metrics']['war_global_pressure_total'])>0
    assert 'recipient.departure' in pressure['error'] and 'TimeoutError' in pressure['error']
    command=pressure['server_command']
    assert command[command.index('--total-queue-bytes')+1]=='262144'
    assert command[command.index('--queue-bytes')+1]=='262144'
    assert command[command.index('--max-message-bytes')+1]=='65536'
    assert len(json.loads((ROOT/'fixture-cli-checks.json').read_text()))==2
    if args.check_local:
        work=ROOT.parents[2]
        assert (work/'scripts/departure-fairness.py').read_bytes()==fixture
        assert (work/'src/relay/continuation_tests.rs').read_bytes()==gzip.decompress((ROOT/'selected-continuation-tests.rs.gz').read_bytes())
        # Only the additional cfg(test) module differs from the selected release
        # source snapshot. Every other build input remains byte-identical.
        for relative,expected in builds['control']['source_sha256'].items():
            if relative=='src/relay/continuation_tests.rs':continue
            assert digest((work/relative).read_bytes())==expected,relative
        for build in builds.values():
            assert digest(Path(build['binary']).read_bytes())==build['binary_sha256']
    print('PASS: immutable evidence, comparison/pressure gates, exact flags, process cleanup, patch reconstruction and unchanged selected production source')

if __name__=='__main__':main()
