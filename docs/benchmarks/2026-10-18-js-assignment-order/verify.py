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
before=json.loads(read('js-assignment-order-before-live-check.json'))
assert not before['passed'] and before['test_exit_code']==1 and before['exit_codes']==[0,0]
assert 'pass 37' in before['test_output'] and 'fail 3' in before['test_output']
for name in ['assignment acknowledgement updates identity before the next message callback','low-level assignment acknowledgements update identity without changing result types','native pipelined assignment and self-message expose the acknowledged identity']:
    assert name in before['test_output']
assert 'userId: 1n' in before['test_output'] and 'userId: 14764350148183932596n' in before['test_output']
after=json.loads(read('js-assignment-order-after-live-check.json'))
assert after['passed'] and after['test_exit_code']==0 and after['exit_codes']==[0,0]
assert 'pass 40' in after['test_output'] and 'fail 0' in after['test_output']
assert after['client_sha256']==build['source_sha256']['clients/js/relay.mjs']
assert before['server_sha256']==after['server_sha256'] and before['bridge_sha256']==after['bridge_sha256']
assert not any(after['final_resource_gauges'].values())
assert 'Done in' in read('js-assignment-order-browser.log')
print('Two deterministic and one native identity failures, 40 passing client tests, source/binary identities, browser build and resource retirement verified.')
