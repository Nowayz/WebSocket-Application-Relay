import gzip, hashlib, json
from pathlib import Path
root=Path(__file__).resolve().parent
for line in (root/'SHA256SUMS').read_text().splitlines():
    expected,name=line.split('  ',1)
    assert hashlib.sha256((root/name).read_bytes()).hexdigest()==expected,name
def read(name):
    return gzip.decompress((root/'raw'/(name+'.gz')).read_bytes()).decode()
build=json.loads((root/'build.json').read_text())
for name,digest in build['source_sha256'].items():
    assert hashlib.sha256((root/'raw'/Path(name).name).read_bytes()).hexdigest()==digest
before=json.loads(read('js-channel-listing-before-live-check.json'))
assert not before['passed'] and before['test_exit_code']==1 and before['exit_codes']==[0,0]
assert 'pass 36' in before['test_output'] and 'fail 1' in before['test_output']
assert 'Missing expected rejection' in before['test_output']
assert 'native channel listing returns sorted byte names and current populations' in before['test_output']
after=json.loads(read('js-channel-listing-after-live-check.json'))
assert after['passed'] and after['test_exit_code']==0 and after['exit_codes']==[0,0]
assert 'pass 37' in after['test_output'] and 'fail 0' in after['test_output']
assert after['client_sha256']==build['source_sha256']['clients/js/relay.mjs']
assert before['server_sha256']==after['server_sha256'] and before['bridge_sha256']==after['bridge_sha256']
assert not any(after['final_resource_gauges'].values())
assert 'Done in' in read('js-channel-listing-browser.log')
print('Native format baseline, original malformed-result failure, 37 passing client tests, source/binary identities, browser build and retirement verified.')
