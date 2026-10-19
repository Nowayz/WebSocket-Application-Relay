import gzip,json
from pathlib import Path
p=Path(__file__).resolve().parent
r=json.loads(gzip.decompress((p/'profile-receipt.json.gz').read_bytes()))
assert r['passed'] and r['profile_diagnostic'] and r['client_exit_code']==r['server_exit_code']==0 and r['perf_exit_code']==-2 and not r['server_forced_kill']
assert 'Total Lost Samples: 0' in (p/'profile.txt').read_text()
s=json.loads((p/'scratch-recovery.json').read_text());assert s['verified_reconstructed_files']==108 and s['abandoned_source_files_verified']==112
assert 'lock incq' in (p/'send-annotation.txt').read_text() and 'lock sub' in (p/'batch-annotation.txt').read_text()
print('Verified accepted diagnostic profile, atomic annotations and recorded scratch recovery.')
