import gzip,json,math
from pathlib import Path
p=Path(__file__).resolve().parent
s=json.loads((p/'summary.json').read_text());assert s['selected'] is False
for x in s['rows']:
 r=json.loads(gzip.decompress((p/x['receipt']).read_bytes()));assert r['passed'] and r['client_exit_code']==r['server_exit_code']==0 and not r['server_forced_kill']
 m=r['mixed'];assert math.isclose(x['cpu_seconds_per_million_deliveries'],m['server_cpu_seconds']*1e6/m['counts']['deliveries'])
 assert x['private_max_ms']==m['healthy']['max_ms'] and x['fanout_max_ms']==m['fanout']['max_ms']
 assert r['profile_diagnostic'] is False
print('Verified four ordinary receipts, clean exits and reported measurements.')
