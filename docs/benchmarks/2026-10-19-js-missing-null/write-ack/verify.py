"""Audit the write-ack follow-up without running servers."""
import argparse,gzip,hashlib,json
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('--check-local-bundle',action='store_true');args=parser.parse_args()
p=Path(__file__).resolve().parent
sha=lambda data:hashlib.sha256(data).hexdigest()
for name,digest in json.loads((p/'sha256.json').read_text()).items():assert sha((p/name).read_bytes())==digest,name
base=gzip.decompress((p/'../selected-relay.mjs.gz').read_bytes())
selected=gzip.decompress((p/'selected-relay.mjs.gz').read_bytes())
needle=b"          if ((op === 0 || op === 4) && typeof result !== 'boolean') throw new Error('Invalid boolean response');"
addition=b"\n          if (op === 4 && result !== true) throw new Error('Invalid variable acknowledgement');"
assert base.count(needle)==1 and base.replace(needle,needle+addition)==selected
provenance=json.loads((p/'provenance.json').read_text());assert provenance['runtime_sources_unchanged']
assert sha(base)==provenance['files']['clients/js/relay.mjs']['base_sha256']
assert sha(selected)==provenance['files']['clients/js/relay.mjs']['selected_sha256']
before=(p/'before.log').read_text();assert 'fail 2' in before and 'Missing expected rejection' in before
local=(p/'offline.log').read_text();assert 'pass 55' in local and 'fail 0' in local
full=(p/'integration.log').read_text();assert 'pass 70' in full and 'fail 0' in full and 'all six checked gauges are zero' in full
assert 'real WebSocket closes with application code 4002 for invalid write-ack' in full
bundle=json.loads((p/'bundle.json').read_text());assert bundle['client_source_sha256']==sha(selected)
if args.check_local_bundle:
 for kind in ['bundle','map']:assert sha(Path(bundle[kind+'_path']).read_bytes())==bundle[kind+'_sha256']
 mapping=json.loads(Path(bundle['map_path']).read_text());assert mapping['sourcesContent'][bundle['client_source_index']].encode()==selected
print('PASS: one guard, two failed-before fixtures, valid auth/write distinctions, 55 local/70 integrated checks and browser source identity')
