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
    native={kind:json.loads((ROOT/(kind+'-build.json')).read_text()) for kind in ['control','selected']}
    assert all(b['passed'] and not b['cargo_artifact']['fresh'] for b in native.values())
    for name in ['mixed-1-control','mixed-2-candidate','mixed-3-candidate','mixed-4-control',
                 'mixed-observers-candidate','mixed-observers-control','departure-candidate']:
        x=read(name);kind='control' if 'control' in name else 'selected'
        assert (x.get('server_sha256') or x['sha256']['server'])==native[kind]['binary_sha256']
    changed=['src/relay.rs','src/relay/continuation_tests.rs']
    with tempfile.TemporaryDirectory() as temp:
        temp=Path(temp)
        for path in changed:
            base=gzip.decompress((ROOT/('base-'+path.replace('/','-')+'.gz')).read_bytes())
            target=temp/path;target.parent.mkdir(parents=True,exist_ok=True);target.write_bytes(base)
            if path=='src/relay.rs':assert digest(base)==native['control']['source_sha256'][path]
        subprocess.run(['git','apply',str(ROOT/'changes.patch')],cwd=temp,check=True,capture_output=True)
        for path in changed:
            selected=gzip.decompress((ROOT/('selected-'+path.replace('/','-')+'.gz')).read_bytes())
            assert (temp/path).read_bytes()==selected
            assert digest(selected)==native['selected']['source_sha256'][path]
    component={}
    for kind in ['control','candidate']:
        build=json.loads((ROOT/(kind+'-component-build.json')).read_text())
        sources=json.loads((ROOT/(kind+'-component-sources.json')).read_text())
        assert build['reason']=='compiler-artifact' and build['fresh'] is False
        assert build['profile']['test'] and str(build['profile']['opt_level'])=='3'
        tests=gzip.decompress((ROOT/(kind+'-component-tests.rs.gz')).read_bytes())
        assert digest(tests)==sources['src/relay/continuation_tests.rs']
        assert b'fn retirement_wake_census()' in tests
        assert sources['src/relay.rs']==native['control' if kind=='control' else 'selected']['source_sha256']['src/relay.rs']
        component[kind]=json.loads((ROOT/(kind+'-component-binary.json')).read_text())
    selection=json.loads((ROOT/'selection.json').read_text())
    assert selection['passed'] and selection['sha256']==native['selected']['binary_sha256']
    assert selection['measured_binary']==native['selected']['binary']
    selected_tests=gzip.decompress((ROOT/'selected-src-relay-continuation_tests.rs.gz').read_bytes())
    assert b'fn retirement_publications_share_one_wake_until_the_queue_drains()' in selected_tests
    assert b'fn retirement_wake_census()' not in selected_tests
    if args.check_local:
        work=ROOT.parents[2]
        for path in changed:
            assert digest((work/path).read_bytes())==native['selected']['source_sha256'][path],path
        for b in native.values():assert digest(Path(b['binary']).read_bytes())==b['binary_sha256']
        assert digest(Path(selection['selected_binary']).read_bytes())==selection['sha256']
        for b in component.values():assert digest(Path(b['binary']).read_bytes())==b['sha256']
    print('PASS: hashes, release component counters/timing, ordinary delivery/resource gates, exact source patch and executable identities')

if __name__=='__main__':main()
