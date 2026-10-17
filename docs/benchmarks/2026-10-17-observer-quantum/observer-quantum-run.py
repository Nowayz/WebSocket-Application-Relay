from pathlib import Path
import hashlib,json,subprocess,sys
record={'scope':'Recipient-work quantum only versus selected heartbeat-phase baseline;fixed new verifying global channel-observer client.512authenticatedglobalobservers,sourceordinaryroom,1privatepair,64idlerooms,8churnworkers,2HTTPinspectors/10ms,1KiB,16framewindows,6seconds. Native1or4workers allowedCPU0or0-3,8clientworkers8-15. Baseline/candidate/candidate/baseline,fresh sequential ordinary processes,no profiler/build/other benchmarks. Latencies are power-of-two histogram upper bounds including client work;maxima exact.','runs':[],'passed':False}
for workers in [1,4]:
 for repetition,names in [(1,['baseline','candidate']),(2,['candidate','baseline'])]:
  for name in names:
   prefix=f'artifacts/observer-quantum-{workers}w-{name}-{repetition}'
   assert not Path(prefix+'.json').exists()
   server='artifacts/candidates/'+('heartbeat-phase' if name=='baseline' else 'observer-quantum')+'/war-relay'
   cmd=[sys.executable,'scripts/inspection.py','--server',server,'--client','artifacts/candidates/observer-quantum-client/inspection','--mode','http','--rooms','64','--seconds','6','--inspectors','2','--inspection-interval-ms','10','--pairs','1','--churn-workers','8','--workers',str(workers),'--client-workers','8','--server-cpus','0' if workers==1 else '0-3','--client-cpus','8-15','--fanout-readers','512','--fanout-global-observers','--fanout-window','16','--output',prefix+'.json']
   print(json.dumps({'started':name,'workers':workers,'repeat':repetition,'command':cmd}),flush=True)
   with open(prefix+'.log','w') as log:done=subprocess.run(cmd,stdout=log,stderr=log,timeout=120)
   x=json.loads(Path(prefix+'.json').read_text());assert done.returncode==0 and x['passed'] and x['server_exit_code']==x['client_exit_code']==0
   assert x['server_sha256']==hashlib.sha256(Path(server).read_bytes()).hexdigest()
   row={'kind':name,'workers':workers,'repeat':repetition,'trace':prefix+'.json','trace_sha256':hashlib.sha256(Path(prefix+'.json').read_bytes()).hexdigest(),'mixed':x['mixed']}
   record['runs'].append(row);Path('artifacts/observer-quantum-summary.json').write_text(json.dumps(record,indent=2)+'\n');print(json.dumps(row),flush=True)
record['passed']=True
Path('artifacts/observer-quantum-summary.json').write_text(json.dumps(record,indent=2)+'\n')
