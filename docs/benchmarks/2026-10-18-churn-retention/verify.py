"""Verify finite repeated connection retirement and allocator-memory evidence."""
import gzip,hashlib,json,subprocess
from pathlib import Path
root=Path(__file__).resolve().parent
for name,h in json.loads((root/'manifest.json').read_text()).items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==h,name
summary=json.loads((root/'summary.json').read_text())
for row in summary['runs']:
 d=json.loads(gzip.decompress((root/'raw'/(row['name']+'.json.gz')).read_bytes()));assert d['passed'] and d['client_exit_code']==d['server_exit_code']==0
 assert not d.get('forced_kill_pids') and len(d['events'])==20
 assert [e['event'] for e in d['events']]==['connected','ready','measured','postcheck','done']*4
 base=d['baseline']['server'];done=[e for e in d['events'] if e['event']=='done'];assert base['fd_count']==26
 for wave,e in enumerate(done,1):
  m=e['metrics'];assert e['wave']==wave and e['server']['fd_count']==26 and e['established_server_sockets']==0
  assert all(m[k]==0 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']) and m['war_channels']==1
  for key,value in e['exact_counter_checks'].items():assert m[key]==value,key
  assert m['war_accepted_total']==m['war_joined_total']==m['war_disconnected_total']==wave*16384
 assert done[-1]['counts']['joins']==done[-1]['counts']['normal_closes']==65536
 assert done[-1]['counts']['routing_checks']==157312 and done[-1]['counts']['departure_checks']==32768
 assert row['retired_rss_mib']==[e['server']['VmRSS']/1024 for e in done]
 assert row['connected_rss_mib']==[e['server']['VmRSS']/1024 for e in d['events'] if e['event']=='connected']
 assert row['server_cpu_seconds']==done[-1]['server']['cpu_seconds']-base['cpu_seconds']
 if row['allocator']=='jemalloc':assert d['server_preload']['sha256']==summary['jemalloc_sha256']
 else:assert d['server_preload'] is None
 for file,h in d['runner_sources_sha256'].items():assert hashlib.sha256(subprocess.check_output(['git','show',summary['revision']+':'+file])).hexdigest()==h,file
print('Verified four fresh cases, sixteen retired gates, 262,144 normal connection lifecycles, exact counters, descriptor/resource retirement, allocator identity and runner source provenance.')
