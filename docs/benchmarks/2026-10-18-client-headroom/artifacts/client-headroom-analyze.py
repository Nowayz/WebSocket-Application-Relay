from pathlib import Path
import json,csv,statistics
runs=[]
for name in ['event','workers','placement']:
 campaign=json.loads(Path(f'artifacts/client-{name}-campaign.json').read_text())
 for case in campaign['runs']:
  if case['exit_code']!=0:continue
  p=Path(case['receipt']);x=json.loads(p.read_text());w=json.loads(p.with_suffix('.windows.json').read_text())['windows']
  with p.with_suffix('.csv').open() as source:rows=list(csv.DictReader(source))
  columns={}
  for column in ['max_ms','schedule_max_ms']:
   values=sorted(float(row[column]) for row in rows)
   columns[column]={key:values[(len(values)-1)*q//100] for key,q in [('median',50),('p95',95),('p99',99),('max',100)]}
  cpu=x['measurement']['client_cpu_seconds'] if name=='placement' else x['client_after']['capacity']['cpu_seconds']-x['client_before']['capacity']['cpu_seconds']
  m={'campaign':name,'label':case.get('label',str(case.get('workers'))),'repeat':case['repeat'],'prefix':str(p.with_suffix('')),'passed':x['passed'],'client_cpu_seconds':cpu,'user_worst_ms':columns,'pre_send_max_ms':max(r['pre_send_delay']['max_ms'] for r in w),'window_send_p99_ms':[r['send_to_receive']['p99_ms'] for r in w]}
  if name=='placement':m['measurement']=x['measurement']
  runs.append(m)
Path('artifacts/client-headroom-summary.json').write_text(json.dumps(runs,indent=2)+'\n')
for x in runs:print(x['campaign'],x['label'],x['repeat'],'CPU',round(x['client_cpu_seconds'],2),'worst-user send p99',round(x['user_worst_ms']['max_ms']['p99'],3),'intended p99',round(x['user_worst_ms']['schedule_max_ms']['p99'],3),'max',round(x['user_worst_ms']['schedule_max_ms']['max'],3),'pre_send_max',round(x['pre_send_max_ms'],3))
