import json,subprocess,sys
from pathlib import Path
root=Path('artifacts/join-output-20261019-cli');root.mkdir()
rows=[]
for mode in ['missing','permission','interpreter']:
 server=(root/(mode+'-server')).resolve()
 if mode=='permission': server.write_text('#!/bin/sh\nexit 99\n');server.chmod(0o600)
 if mode=='interpreter': server.write_text('#!/missing-war-fixture-interpreter\n');server.chmod(0o700)
 output=root/(mode+'.json');cmd=[sys.executable,'scripts/join-pressure.py','--server',str(server),'--connections','1','--waves','1','--output',str(output)]
 done=subprocess.run(cmd,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True,timeout=2)
 receipt=json.loads(output.read_text());assert done.returncode==1 and not receipt['passed'] and receipt['server_exit_code'] is None and 'error' in receipt
 assert output.with_suffix('.server.log').read_bytes()==b''
 rows.append({'mode':mode,'command':cmd,'cli_exit_code':done.returncode,'receipt':receipt})
Path('artifacts/join-output-20261019-cli.json').write_text(json.dumps({'passed':True,'cases':rows},indent=2)+'\n')
