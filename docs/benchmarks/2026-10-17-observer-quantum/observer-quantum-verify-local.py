from pathlib import Path
import json,os,hashlib
root=Path.cwd();p=root/'artifacts/observer-quantum-manual-acceptance.json';x=json.loads(p.read_text());state=json.loads((root/'artifacts/reactor-manual-services.json').read_text());assert x['passed'] and x['native_pid']==state['pids'][0] and x['native_command']==state['commands'][0]
pid=x['native_pid'];threads=[]
for task in Path(f'/proc/{pid}/task').iterdir():
 name=(task/'comm').read_text().strip()
 if name.startswith('war-io-'):
  index=int(name.removeprefix('war-io-'));cpus=sorted(os.sched_getaffinity(int(task.name)));assert cpus==[index];threads.append({'tid':int(task.name),'name':name,'cpus':cpus})
assert len(threads)==4
x['verified_worker_affinity']=sorted(threads,key=lambda t:t['name']);x['running_executable_sha256']=hashlib.sha256(Path(f'/proc/{pid}/exe').read_bytes()).hexdigest();assert x['running_executable_sha256']==x['native_sha256']=='f1bf8bf9fd7c7e8f189806dba4bbdc4441e11ca9f7f680d8d14efe4716cb49be'
x['runtime_source_revision']=state['source_revision'];p.write_text(json.dumps(x,indent=2)+'\n');print(json.dumps({'passed':True,'native_pid':pid,'runtime_source_revision':x['runtime_source_revision'],'native_sha256':x['native_sha256']}))
