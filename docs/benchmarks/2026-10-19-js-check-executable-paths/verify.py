import gzip,json
from pathlib import Path
p=Path(__file__).resolve().parent
r=json.loads((p/'evidence.json').read_text())
assert r['before_exit']==r['live_resource_leak_exit']==1 and r['after_exit']==0 and r['default_snapshot_binary_absent']
before=gzip.decompress((p/'before.log.gz').read_bytes()).decode();assert 'target/debug/war-relay: No such file or directory' in before
after=gzip.decompress((p/'after.log.gz').read_bytes()).decode();assert 'tests 61' in after and 'pass 61' in after and 'fail 0' in after and 'skip' in after and 'all six checked gauges are zero' in after
leak=gzip.decompress((p/'leak.log.gz').read_bytes()).decode();assert 'pass 61' in leak and 'war_connections 1' in leak and 'war_sessions 1' in leak and 'resources did not retire' in leak
assert len(r['test_ports_after_failure'].splitlines())==1
print('Verified custom-target reproduction, suite acceptance, actual live-client rejection and released ports.')
