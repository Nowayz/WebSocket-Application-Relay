"""Bind this measured checkpoint to sources, frozen executables and raw evidence."""
import datetime,hashlib,json,re,shutil,statistics,subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[1]
source=root/'artifacts'
archive=root/'docs/benchmarks/2026-10-17-c10m'
archive.mkdir(parents=True,exist_ok=True)
allowed={'.json','.txt','.log','.py','.patch','.rs'}
for path in sorted(source.glob('c10m-*')):
    if path.is_file() and path.suffix in allowed:
        shutil.copy2(path,archive/path.name)
    elif path.is_dir() and path.name.endswith('-traces'):
        shutil.copytree(path,archive/path.name,dirs_exist_ok=True)
for name in ['capacity-final','c10m-batch','c10m-sendmsg','c10m-queued','c10m-bounded','c10m-whole']:
    directory=source/'baselines'/name
    for path in directory.rglob('*'):
        if path.is_file() and path.suffix in allowed:
            dest=archive/'snapshots'/name/path.relative_to(directory)
            dest.parent.mkdir(parents=True,exist_ok=True)
            shutil.copy2(path,dest)
for name in ['capacity.py','capacity_pressure.py','benchmark.py','compare.py','stability.py','reference.py']:
    dest=archive/'harness'/name
    dest.parent.mkdir(parents=True,exist_ok=True)
    shutil.copy2(root/'scripts'/name,dest)
def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
def reference(path):
    return str(path.relative_to(root/'docs/benchmarks'))
def load(name):
    return json.loads((source/name).read_text())
def overload(name):
    result={}
    for label in ['control',name]:
        paths=sorted(source.glob(f'c10m-{name}-overload-*-{label}.json'))
        data=[json.loads(path.read_text()) for path in paths]
        assert len(data)==3 and all(d['passed'] for d in data)
        result[label]={'traces':[reference(archive/path.name) for path in paths],
          'median_completions_per_second':statistics.median(d['measurements'][0]['stable_deliveries_per_second'] for d in data),
          'median_cpu_ns_per_completion':statistics.median(d['measurements'][0]['server_cpu_seconds']*1e9/5000000 for d in data)}
    return result
selected=source/'baselines/c10m-sendmsg/war-relay'
control=source/'baselines/capacity-final/war-relay'
assert digest(selected)==digest(root/'target/release/war-relay')
assert (root/'src/server/socket.rs').read_bytes()==(source/'baselines/c10m-sendmsg/src/server/socket.rs').read_bytes()
for name in ['c10m-final-dense.json','c10m-final-fanout.json','c10m-final-mixed.json','c10m-queued-fanout.json','c10m-bounded-fanout.json','c10m-whole-fanout.json']:
    assert all(run['passed'] for run in load(name)['runs'])
for name in ['c10m-final-large.json','c10m-final-400k-control.json','c10m-final-400k-selected.json','c10m-final-100k-mixed.json','c10m-repeat-mixed-control.json','c10m-repeat-mixed-selected.json','c10m-before-kernel.json','c10m-selected-kernel.json','c10m-manual-acceptance.json']:
    assert load(name)['passed']
manifest={
 'date':'2026-10-17','generated_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),
 'title':'C10M guidance: measured socket send interface and rejected prototypes',
 'base_revision':'104effdf00c06c388a7d9cc5655679b03611d155',
 'compiler':subprocess.check_output(['rustc','--version'],text=True).strip(),
 'sources':[
  'https://highscalability.com/the-secret-to-10-million-concurrent-connections-the-kernel-i/',
  'https://doc.dpdk.org/guides/prog_guide/writing_efficient_code.html',
  'https://docs.kernel.org/networking/scaling.html',
  'https://docs.rs/socket2/0.6.5/socket2/struct.Socket.html#method.send_vectored',
  'https://docs.rs/mio/1.2.4/mio/net/struct.TcpStream.html#method.try_io'],
 'executables':{label:{'local_path':str(path.relative_to(root)),'sha256':digest(path)} for label,path in [('control',control),('selected',selected),('capacity_client',source/'baselines/capacity-final/capacity'),('stress_client',source/'baselines/capacity-final/stress')]},
 'source_association':'Frozen build metadata, complete runtime source snapshots and patches under snapshots/. Selected release rebuild is byte-identical to the timed selected executable. Base Git revision plus selected runtime patch identifies the accepted source.',
 'ordinary_overload':overload('sendmsg'),
 'ordinary_workloads':{name:load(f'c10m-final-{name}.json')['summary'] for name in ['dense','fanout','mixed']},
 'population_acceptance':[reference(archive/name) for name in ['c10m-final-100k-mixed.json','c10m-repeat-mixed-control.json','c10m-repeat-mixed-selected.json']],
 'rejected_prototypes':{
   'placement':'One exploratory frozen-binary case, c10m-placement-pinned-1.json; no measured gain sufficient for a production affinity setting.',
   'cumulative_batches':overload('batch'),
   'queued_frames':{'trace':'c10m-queued-fanout.json','outcome':'Throughput +18.65%, CPU approximately -4.97%, but approximately 42 ms RTT maxima in every candidate run with a 4 KiB send-buffer request. Removed.'},
   'queued_bytes_4096':{'trace':'c10m-bounded-fanout.json','outcome':'Spikes removed; gain too small and inconsistent for added code. Removed.'},
   'queued_complete_frames_4096':{'trace':'c10m-whole-fanout.json','outcome':'No consistent speed/CPU improvement. Removed.'}},
 'diagnostics':{},
 'verification':{'rust_tests_passed':sum(map(int,re.findall(r'test result: ok\. (\d+) passed', (source/'c10m-final-tests.log').read_text()))),
    'clippy':'--locked --all-targets -- -D warnings; passed','format':'cargo fmt --check; passed','profile_cli_guard':'--profile-kernel without --profile returns parser exit 2 before starting a server',
    'manual_service_acceptance':reference(archive/'c10m-manual-acceptance.json')},
 'limits':[
   'Plaintext loopback. Separate server/client physical CPU sets. Process RSS excludes kernel TCP memory.',
   'Latency quantiles are histogram upper bounds; RTT and client scheduling backlog are distinct.',
   'Overload offered rate is not achieved throughput. One million/s still falls behind on four workers.',
   'Dense throughput median is 0.96% lower with overlapping ranges, while server CPU per delivery is lower.',
   'Initial selected full-population run has a 20.906 ms RTT outlier; repeat is 3.174 ms. No isolated causal or long-term tail claim.',
   'Population pressure sidecars vary with flow placement and are not matched throughput/CPU comparisons.',
   'Expected unread-peer capacity closes and partial-frame TCP cancellations are distinguished from normal stable/churn closes.',
   'Private-only traffic checks exact bytes, counters and lifecycle. Pressure output includes explicit bounds for intentionally cancelled/unobserved output.',
   'Application-body reuse is not kernel zero-copy. No new bypass driver, user-space TCP stack or huge-page pool.',
   'No remote NIC, WAN/TLS edge, multi-hour, all-to-all or ten-million-user capacity acceptance.'],
}
for label in ['before','selected']:
    path=source/f'c10m-{label}-kernel.perf.data'
    data=load(f'c10m-{label}-kernel.json')
    manifest['diagnostics'][label]={'trace':reference(archive/f'c10m-{label}-kernel.json'),'local_perf_path':str(path.relative_to(root)),'perf_sha256':digest(path),'perf_bytes':path.stat().st_size,'profile_command':data['profile_command'],'perf_log':data['perf_log'],'lost_samples':0,'lost_samples_evidence':reference(archive / f"c10m-{label}-kernel-{'self' if label == 'before' else 'flat'}.txt")}
manifest['files']=[{'path':reference(path),'sha256':digest(path),'bytes':path.stat().st_size} for path in sorted(archive.rglob('*')) if path.is_file()]
assert manifest['verification']['rust_tests_passed']==141
out=root/'docs/benchmarks/2026-10-17-c10m.json'
out.write_text(json.dumps(manifest,indent=2)+'\n')
print(json.dumps({'manifest':str(out.relative_to(root)),'archived_files':len(manifest['files']),'archived_bytes':sum(row['bytes'] for row in manifest['files']),'accepted_native_sha256':manifest['executables']['selected']['sha256'],'rust_tests':141}))
