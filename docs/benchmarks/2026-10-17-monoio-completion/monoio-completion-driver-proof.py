"""Independent kernel-FD proof; excluded from ordinary performance measurements."""
import hashlib,json,os,socket,struct,subprocess,time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
results=[]
for kind,path in [('mio','artifacts/candidates/input-batch/war-relay'),('monoio','artifacts/candidates/monoio-completion-bounded/war-relay')]:
    with socket.socket() as spare:
        spare.bind(('127.0.0.1',0));port=spare.getsockname()[1]
    env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
    command=[str(ROOT/path),'--bind',f'127.0.0.1:{port}','--no-admin','--workers','1']
    server=subprocess.Popen(command,env=env,stdout=subprocess.PIPE,stderr=subprocess.STDOUT,text=True)
    client=None
    try:
        for _ in range(200):
            if server.poll() is not None:raise RuntimeError(server.stdout.read())
            try:client=socket.create_connection(('127.0.0.1',port),timeout=2);break
            except ConnectionRefusedError:time.sleep(.01)
        assert client is not None
        client.sendall(b'GET / HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Protocol: war.cbor.v1\r\n\r\n')
        response=b''
        while not response.endswith(b'\r\n\r\n'):response+=client.recv(1)
        assert response.startswith(b'HTTP/1.1 101 ') and b'war.cbor.v1' in response
        time.sleep(.05)
        targets=[];rings=[]
        for fd in Path(f'/proc/{server.pid}/fd').iterdir():
            try:
                target=os.readlink(fd);targets.append(target)
                if 'io_uring' in target:rings.append(Path(f'/proc/{server.pid}/fdinfo/{fd.name}').read_text())
            except FileNotFoundError:pass
        assert len(rings)==(1 if kind=='monoio' else 0)
        body=struct.pack('>H',1000);mask=b'abcd'
        client.sendall(bytes([0x88,0x82])+mask+bytes(x^mask[i%4] for i,x in enumerate(body)))
        ack=b''
        while len(ack)<4:ack+=client.recv(4-len(ack))
        assert ack==b'\x88\x02\x03\xe8' and client.recv(1)==b''
        results.append({'kind':kind,'binary_sha256':hashlib.sha256((ROOT/path).read_bytes()).hexdigest(),'command':command,'io_uring_descriptors':len(rings),'socket_descriptors':sum(t.startswith('socket:[') for t in targets),'fd_count':len(targets),'ring_fdinfo':rings,'websocket_upgrade_verified':True,'exact_close_ack_and_eof':True})
    finally:
        if client is not None:client.close()
        server.terminate();logs=server.communicate(timeout=10)[0]
        assert server.returncode==0,logs
        if results and results[-1]['kind']==kind:results[-1]['server_exit_code']=server.returncode
output={'passed':True,'diagnostic_only':True,'scope':'One fresh one-worker server and one upgraded TCP/WebSocket peer at a time. Kernel descriptor inspection proves an io_uring ring only for the Monoio completion variant. No benchmark timings taken.','runs':results}
(ROOT/'artifacts/monoio-completion-driver-proof.json').write_text(json.dumps(output,indent=2)+'\n')
print(json.dumps({'passed':True,'rings':{r['kind']:r['io_uring_descriptors'] for r in results}}))
