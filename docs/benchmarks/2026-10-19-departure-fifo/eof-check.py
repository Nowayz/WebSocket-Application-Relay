"""Correctness-only EOF variant; event ACK fields mean client shutdown return."""
import argparse, hashlib, importlib.util, json, socket
from pathlib import Path
root=Path('/home/phil/dev/war_next')
spec=importlib.util.spec_from_file_location('departure',root/'scripts/departure-fairness.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
base=fixture.Peer
class EOFPeer(base):
    def __init__(self,port,channel,index):
        super().__init__(port,channel,index)
        self.eof_source=channel==b'dp-big' or channel.startswith(b'dp-control-')
    def close(self):
        if self.eof_source:
            self.socket.shutdown(socket.SHUT_RDWR)
            self.socket.close()
        else:
            super().close()
fixture.Peer=EOFPeer
fixture.wire.ZERO_ERRORS=tuple(x for x in fixture.wire.ZERO_ERRORS if x!='war_transport_disconnects_total')
args=argparse.Namespace(server=root/'artifacts/candidates/departure-fifo-20261019/war-relay',audience=100000,server_cpus='0-7')
output=root/'artifacts/departure-quantum-20261019-runs/eof-selected.json'
with output.open('x') as f:
    result=fixture.run(args)
    result['variant']='EOF correctness only; close_ack_ms is client shutdown return, not server ACK'
    result['variant_sha256']=hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    if result['passed']:
        delta=int(result['after']['war_transport_disconnects_total'])-int(result['before']['war_transport_disconnects_total'])
        result['expected_eof_disconnects']=4
        result['observed_eof_disconnects']=delta
        result['passed']=delta==4
    json.dump(result,f,indent=2);f.write('\n')
print(json.dumps({k:result.get(k) for k in ['passed','error','observed_eof_disconnects']}))
raise SystemExit(0 if result['passed'] else 1)
