from pathlib import Path
import json,csv,statistics
runs=[]
for case in json.loads(Path('artifacts/client-event-campaign.json').read_text())['runs']:
 if case['exit_code'] != 0:continue
 p=Path(case['receipt']);x=json.loads(p.read_text());t=json.loads(p.with_suffix('.windows.json').read_text())
 with p.with_suffix('.csv').open() as file:rows=list(csv.DictReader(file))
 intended=[float(r['schedule_max_ms']) for r in rows]
 runs.append({'label':case['label'],'repeat':case['repeat'],'passed':x['passed'],'client_cpu_seconds':x['client_after']['capacity']['cpu_seconds']-x['client_before']['capacity']['cpu_seconds'],'server_cpu_seconds':x['server_cpu_seconds'],'fanout_windows':x['fanout_windows'],'private_worst_distribution_ms':x['latency']['schedule_max_ms'],'users_with_every_probe_under_5ms':sum(v<=5 for v in intended),'users_with_every_probe_under_10ms':sum(v<=10 for v in intended),'stages_p99_ms':{stage:{'min':min(w['client_stages'][stage]['p99_ms'] for w in t['windows']),'median':statistics.median(w['client_stages'][stage]['p99_ms'] for w in t['windows']),'max':max(w['client_stages'][stage]['p99_ms'] for w in t['windows'])} for stage in t['stage_order']}})
Path('artifacts/client-event-summary.json').write_text(json.dumps(runs,indent=2)+'\n')
for r in runs:print(r['label'],r['repeat'],'cpu',round(r['client_cpu_seconds'],2),'postsend',r['stages_p99_ms']['after_send'],'intendedworst',r['private_worst_distribution_ms'],'allunder5ms',r['users_with_every_probe_under_5ms'])
