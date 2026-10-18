"""Check the archived rejected-parser screen without starting services."""
from pathlib import Path
import hashlib,json,statistics
root=Path(__file__).resolve().parent
for p,h in json.loads((root/'manifest.json').read_text()).items():assert hashlib.sha256((root/p).read_bytes()).hexdigest()==h,p
summary=json.loads((root/'summary.json').read_text());total=0
for i,label in enumerate(['control','direct','direct','control']):
 d=json.loads((root/'raw'/f'short-bytes-relay-{i}-{label}.json').read_text());assert d['passed'] and d['server_exit_code']==d['client_exit_code']==0 and not d['server_forced_kill']
 end=d['events'][-1];assert end['event']=='done' and end['details']['passed']
 for k,v in end['exact_counter_checks'].items():assert end['metrics'][k]==v,k
 for k in ['war_connections','war_sessions','war_queued_bytes','war_variable_bytes','war_read_buffer_reserved_bytes','war_output_queue_capacity_bytes','war_routing_copied_bytes_total']:assert end['metrics'][k]==0,k
 assert end['metrics']['war_channels']==1
 m=d['mixed'];h=m['healthy'];r=summary['runs'][i];assert h['verified']==r['checked_deliveries'];total+=h['verified']
 assert m['server_cpu_seconds']/h['verified']*1e9==r['server_ns_per_delivery']
 assert h['per_second']==r['deliveries_per_second'] and h['p99_upper_ms']==r['p99_upper_ms']
 batches=[json.loads(line) for line in (root/'raw'/f'short-bytes-parse-{i}-{label}.jsonl').read_text().splitlines()]
 assert [b['payload_bytes'] for b in batches]==[0,16,1024,65536]
 assert all(len(b['ns_per_parse_batches'])==9 and all(x>0 for x in b['ns_per_parse_batches']) for b in batches)
assert total==summary['verified_timed_deliveries']==28011216
corpus=json.loads((root/'raw/short-bytes-corpus.json').read_text());assert corpus['case']['passed'] and corpus['case']['scheduled_exchanges']==103
control=(root/'source/protocol-control.rs').read_text();direct=(root/'source/protocol-direct.rs').read_text()
old='    let start = d.position();\n    validate_value(&mut d)?;'
new='''    let start = d.position();
    // A definite byte string has no children to walk or nesting stack to maintain.
    if matches!(bytes.get(start), Some(0x40..=0x5b)) {
        d.bytes().map_err(bad)?;
    } else {
        validate_value(&mut d)?;
    }'''
assert control.replace(old,new,1)==direct
print('Verified four matched parser/relay cases, 28,011,216 timed deliveries, exact counters/resource retirement, smoke corpus, and the sole source change.')
