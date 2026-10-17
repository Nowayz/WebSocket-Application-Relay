from pathlib import Path
import json,time,urllib.request
server=797241;client=797250
assert Path(f'/proc/{server}/exe').resolve()==Path('artifacts/candidates/heartbeat-phase/war-relay').resolve()
parts=Path(f'/proc/{server}/cmdline').read_bytes().decode().strip('\0').split('\0');admin=parts[parts.index('--admin-bind')+1]
credential=next(item.split(b'=',1)[1].decode() for item in Path(f'/proc/{server}/environ').read_bytes().split(b'\0') if item.startswith(b'WAR_ADMIN_TOKEN='))
request=urllib.request.Request('http://'+admin+'/metrics',headers={'Authorization':'Bearer '+credential})
with urllib.request.urlopen(request,timeout=5) as response:
 metrics={line.split()[0]:int(line.split()[1]) for line in response.read().decode().splitlines() if line.startswith('war_')}
def rss(pid):
 return int(next(line.split()[1] for line in Path(f'/proc/{pid}/status').read_text().splitlines() if line.startswith('VmRSS:')))
record={'monotonic_seconds':time.monotonic(),'server_pid':server,'client_pid':client,'server_rss_mib':rss(server)/1024,'client_rss_mib':rss(client)/1024,'metrics':{name:metrics[name] for name in ['war_connections','war_sessions','war_incoming_messages_total','war_queued_bytes','war_read_buffer_reserved_bytes','war_write_errors_total']}}
with Path('artifacts/soak-600-live-observations.jsonl').open('a') as out:out.write(json.dumps(record)+'\n')
print(json.dumps(record))
