from pathlib import Path
import gzip,hashlib,json,shutil,subprocess
root=Path.cwd();out=root/'docs/benchmarks/2026-10-17-client-placement';out.mkdir(exist_ok=True)
def included(name):
 return name.startswith(('client-placement-400k-','client-placement-600k-','client-placement-counter','client-placement-pinned-counter')) or name in {'client-placement-analysis.json','client-placement-analysis.log','client-placement-analyze.py','client-placement-archive.py','client-placement-plot.py','client-placement-plots.log','client-placement-topology.json'}
for stale in out.glob('client-placement-*'):
 if not included(stale.name): stale.unlink()
analysis=json.loads((root/'artifacts/client-placement-analysis.json').read_text());assert analysis['ordinary_verified_messages']==120000000 and analysis['diagnostic_verified_messages']==36000000
for p in sorted((root/'artifacts').glob('client-placement-*')):
 if not included(p.name) or not p.is_file() or p.suffix not in ['.json','.log','.csv','.py','.png','.pdf']:continue
 target=out/p.name
 if p.suffix=='.csv':target=target.with_suffix('.csv.gz');target.write_bytes(gzip.compress(p.read_bytes(),mtime=0))
 else:shutil.copyfile(p,target)
for folder,prefix in [('heartbeat-phase','server'),('latency-timeline-client','client')]:
 for p in (root/'artifacts/candidates'/folder).iterdir():
  if p.suffix in ['.json','.rs','.patch']:shutil.copyfile(p,out/(prefix+'-'+p.name))
for name in ['capacity.py','benchmark.py','capacity_pressure.py','analyze-perf-counters.py','test-perf-counters.py','plot-user-latency-comparison.py']:
 shutil.copyfile(root/'scripts'/name,out/('source-'+name))
shutil.copyfile(root/'examples/capacity.rs',out/'source-capacity.rs')
server=json.loads((out/'server-build.json').read_text());client=json.loads((out/'client-build.json').read_text())
assert hashlib.sha256((out/'client-capacity.rs').read_bytes()).hexdigest()==client['source_sha256']
for rate in ['400k','600k']:
 summary=json.loads((out/f'client-placement-{rate}-summary.json').read_text());assert summary['passed'] and len(summary['runs'])==4
 for row in summary['runs']:
  path=root/row['trace'];assert hashlib.sha256(path.read_bytes()).hexdigest()==row['trace_sha256']
for p in sorted(out.glob('client-placement-*.json')):
 run=json.loads(p.read_text())
 if 'server_sha256' not in run:continue
 assert run['passed'] and run['server_exit_code']==run['client_exit_code']==0
 assert run['server_sha256']==server['binary_sha256'] and run['client_sha256']==client['binary_sha256']
 assert run['client_source_sha256']==hashlib.sha256((out/'source-capacity.rs').read_bytes()).hexdigest()
 assert run['runner_sha256']==hashlib.sha256((out/'source-capacity.py').read_bytes()).hexdigest()
 for path,want in run['runner_sources_sha256'].items():assert hashlib.sha256((out/('source-'+Path(path).name)).read_bytes()).hexdigest()==want
assert not subprocess.check_output(['git','diff','--','src','examples','scripts','Cargo.toml','Cargo.lock'])
manifest={'base_revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'relay_runtime_changed':False,'prior_study':'docs/benchmarks/2026-10-17-borrowed-submission/manifest.json; older d505a4b7 client-placement ablation had mixed600k results. Current study refreshes selected heartbeat-phase c9a6c6da runtime and adds400k controls/scoped migrations.','server_binary_sha256':server['binary_sha256'],'client_binary_sha256':client['binary_sha256'],'frozen_client_source_sha256':client['source_sha256'],'workspace_client_source_sha256':hashlib.sha256((out/'source-capacity.rs').read_bytes()).hexdigest(),'source_attribution':'Raw client_source_sha256 is explicitly a workspace snapshot. Actual frozen client source is associated through its original client-build.json and patch, preserved separately.','scope':'Eight fresh ordinary client-placement controls,120m verified application messages;two separate scoped counter diagnostics,36m messages. Selected relay unchanged. Same-host plaintext loopback;not a server-code speedup or WAN/TLS capacity upgrade. Perf task-clock and process CPU/whole CPU-set IRQ scopes remain distinct.','files':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(out.iterdir()) if p.name!='manifest.json'}}
(out/'manifest.json').write_text(json.dumps(manifest,indent=2)+'\n');print('Archived',len(manifest['files']),'files')
