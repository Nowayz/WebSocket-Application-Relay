"""Validate archived receipts without starting services."""
import hashlib,json,math
from pathlib import Path
p=Path(__file__).resolve().parent
manifest=json.loads((p/'sha256.json').read_text())
for name,digest in manifest.items():
 assert hashlib.sha256((p/name).read_bytes()).hexdigest()==digest,name
x=json.loads((p/'receipt.json').read_text());c=json.loads((p/'context.json').read_text())
assert x['passed'] and x['requests_per_trial']==50000 and x['warmup_requests']==32
assert [r['label'] for r in x['runs']]==['control-1','candidate-1','candidate-2','control-2']
assert x['server_cpu']==0 and x['client_cpu']==4
assert c['clock_ticks_per_second']==100 and not any(c['profiler_environment_present'].values())
body=(p/'../http-ready.prom').read_bytes();digest=hashlib.sha256(body).hexdigest()
assert len(body)==7557 and digest=='dd57b293550ed4eab801c0d6b653beb11ff34ed15f4571b8ca5f7c13ef376388'
expected={'control':'ebc79736832a11bda4d30c9922ddb5d77f0f1b2e95d031586dd3be7533c67365','candidate':'20336238cf6a27cdf4defe021a1dd5dd27bf1b9610e4f7955075967129b3003c'}
for r in x['runs']:
 assert r['passed'] and r['completed']==50000 and r['exit_code']==0 and not r.get('forced_kill')
 assert r['body_sha256']==digest and r['response_bytes']==len(body) and r['binary_sha256']==expected[r['kind']]
 assert 0<r['elapsed_seconds']<8 and r['server_cpu_seconds']>0
 assert 0<r['latency_ms']['median']<=r['latency_ms']['p95']<=r['latency_ms']['p99']<=r['latency_ms']['max']
 log=(p/(r['label']+'.log')).read_text()
 assert 'Starting tracking the heap' not in log and 'Profiling enabled' not in log
assert math.isclose(c['timed_seconds'],sum(r['elapsed_seconds'] for r in x['runs']))
print('PASS: four fixed-work trials, 200000 verified responses, archived hashes and scope')
