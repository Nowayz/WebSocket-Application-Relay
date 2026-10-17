from pathlib import Path
import gzip,hashlib,json
p=Path(__file__).resolve().parent;m=json.loads((p/'manifest.json').read_text());digest=lambda data:hashlib.sha256(data).hexdigest()
for name,want in m['files'].items():assert digest((p/name).read_bytes())==want,name
for name,record in m['compressed_originals'].items():
 data=gzip.decompress((p/name).read_bytes());assert digest(data)==record['sha256'] and len(data)==record['bytes'],name
for kind in ['control','candidate']:
 b=json.loads((p/'source'/kind/'build.json').read_text())
 for name,want in b['source_sha256'].items():assert digest((p/'source'/kind/name).read_bytes())==want,(kind,name)
print(json.dumps({'passed':True,'files':len(m['files']),'compressed_originals':len(m['compressed_originals']),'source_sets':['control','candidate']}))
