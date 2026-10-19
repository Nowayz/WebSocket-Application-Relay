"""Audit the saved JavaScript missing-null validation evidence."""
import argparse,gzip,hashlib,json
from pathlib import Path
parser=argparse.ArgumentParser();parser.add_argument('--check-local-bundle',action='store_true');args=parser.parse_args()
p=Path(__file__).resolve().parent
sha=lambda data:hashlib.sha256(data).hexdigest()
for name,digest in json.loads((p/'sha256.json').read_text()).items():assert sha((p/name).read_bytes())==digest,name
provenance=json.loads((p/'provenance.json').read_text());assert provenance['runtime_sources_unchanged']
for relative,hashes in provenance['files'].items():
 name=Path(relative).name
 for kind in ['base','selected']:
  assert sha(gzip.decompress((p/(kind+'-'+name+'.gz')).read_bytes()))==hashes[kind+'_sha256']
base=gzip.decompress((p/'base-relay.mjs.gz').read_bytes()).decode()
selected=gzip.decompress((p/'selected-relay.mjs.gz').read_bytes()).decode()
assert base.replace("typeof result[0] !== 'boolean'))", "typeof result[0] !== 'boolean'\n            || (result[0] === false && result[1] !== null)))")==selected
assert 'Invalid variable response' in (p/'before.log').read_text()
assert 'Missing expected rejection' in (p/'real-before.log').read_text()
local=(p/'offline.log').read_text();assert 'pass 53' in local and 'fail 0' in local
full=(p/'js.log').read_text();assert 'pass 68' in full and 'fail 0' in full and 'all six checked gauges are zero' in full
assert 'real WebSocket closes with application code 4002 for invalid missing-variable' in full
bundle=json.loads((p/'bundle.json').read_text());assert bundle['source_map_matches_selected_client'] and bundle['client_source_sha256']==sha(selected.encode())
if args.check_local_bundle:
 for label in ['bundle','map']:assert sha(Path(bundle[label+'_path']).read_bytes())==bundle[label+'_sha256']
 mapping=json.loads(Path(bundle['map_path']).read_text());assert mapping['sourcesContent'][bundle['client_source_index']]==selected
print('PASS: one validator condition, failed-before local/real peer cases, 53 local/68 integrated tests, browser source identity')
