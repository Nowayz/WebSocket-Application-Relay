"""Verify archived short relay comparisons and replay the rejected source change."""
import difflib,hashlib,json,subprocess
from pathlib import Path
root=Path(__file__).resolve().parent
for name,h in json.loads((root/'manifest.json').read_text()).items():assert hashlib.sha256((root/name).read_bytes()).hexdigest()==h,name
build=json.loads((root/'build.json').read_text());total=0
for i,label in enumerate(['control','direct','direct','control']):
 d=json.loads((root/'raw'/f'short-recv-relay-{i}-{label}.json').read_text());assert d['passed'] and d['client_exit_code']==d['server_exit_code']==0 and not d['server_forced_kill']
 end=d['events'][-1];assert end['event']=='done' and end['details']['passed']
 for k,v in end['exact_counter_checks'].items():assert end['metrics'][k]==v,k
 for k in ['war_connections','war_sessions','war_read_buffer_reserved_bytes','war_queued_bytes','war_output_queue_capacity_bytes','war_variable_bytes','war_routing_copied_bytes_total']:assert end['metrics'][k]==0,k
 assert end['metrics']['war_channels']==1
 m=d['mixed'];h=m['healthy'];r=build['runs'][i];assert h['verified']==r['verified'];total+=h['verified']
 assert m['server_cpu_seconds']/h['verified']*1e9==r['cpu_ns_per_delivery'] and h['per_second']==r['messages_per_second']
assert total==build['timed_deliveries']==28188096
c=json.loads((root/'raw/short-recv-corpus.json').read_text())['case'];assert c['passed'] and c['corpus_cases']==776 and c['scheduled_exchanges']==2311
source=subprocess.check_output(['git','show',build['source_revision']+':src/server/socket.rs'],text=True)
assert hashlib.sha256(source.encode()).hexdigest()==build['control_source_sha256']
old='        let read = self.socket.state.lock().stream.read(&mut bytes[..length])?;'
new='''        let read = {
            let state = self.socket.state.lock();
            state.stream.try_io(|| {
                (&*socket2::SockRef::from(&state.stream)).read(&mut bytes[..length])
            })?
        };'''
assert source.count(old)==1
candidate=source.replace(old,new,1);assert hashlib.sha256(candidate.encode()).hexdigest()==build['candidate_source_sha256']
patch=''.join(difflib.unified_diff(source.splitlines(keepends=True),candidate.splitlines(keepends=True),fromfile='a/src/server/socket.rs',tofile='b/src/server/socket.rs'))
assert patch==(root/'candidate.patch').read_text()
print('Verified matched source replay, four accepted relay trials, 28,188,096 deliveries, exact counters/resource retirement and 776-case input corpus.')
