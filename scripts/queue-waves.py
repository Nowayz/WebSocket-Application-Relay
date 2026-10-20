#!/usr/bin/env python3
"""Verify output burst recovery and observe retained queue/receive storage."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import tempfile
import time

import benchmark as runtime
import inspection

ROOT = Path(__file__).resolve().parents[1]
sha = lambda p: hashlib.sha256(Path(p).read_bytes()).hexdigest()


def run(args):
    port, admin = runtime.free_port(), runtime.free_port()
    while port == admin:
        admin = runtime.free_port()
    env = {k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
    if args.heap_prefix:
        prefix = args.heap_prefix.resolve(); prefix.parent.mkdir(parents=True,exist_ok=True)
        assert not list(prefix.parent.glob(prefix.name+'*.heap')), 'use a fresh prefix'
        env.update(LD_PRELOAD=str(Path('/usr/lib/libtcmalloc.so').resolve()),
                   HEAPPROFILE=str(prefix),HEAPPROFILESIGNAL=str(signal.SIGUSR2),
                   HEAP_PROFILE_ALLOCATION_INTERVAL=str(100*1024**3),HEAP_PROFILE_INUSE_INTERVAL=str(100*1024**3))
    server_command = ['taskset','-c','0-3',str(args.server.resolve()),'--bind',f'127.0.0.1:{port}',
        '--admin-bind',f'127.0.0.1:{admin}','--workers','4','--messages-per-second','0','--bytes-per-second','0',
        '--ping-interval-secs','0','--write-timeout-secs','60','--queue-messages','64',
        '--max-message-bytes','16777216','--queue-bytes','33554432','--total-queue-bytes','1073741824']
    client_command = ['taskset','-c','4-7',str(args.client.resolve()),'--url',f'ws://127.0.0.1:{port}',
        '--receivers',str(args.receivers),'--messages','48','--waves',str(args.waves),'--payload-bytes','8388608']
    record = {'scope':'Gated fresh-process burst/recovery test; one publisher and persistent receivers. Each wave broadcasts a changing8MiB value and48 changing scalar values, then uses a private publication marker before receivers drain every full encoded frame in parallel. Every departure, normal Close and TCP EOF is checked. Default per-peer64-message/32MiB limits, enlarged1GiB global byte allowance for this workload. Server CPUs0-3/client4-7. Heap profiling replaces only the server allocator and is diagnostic; client intervals exclude gate pauses while server CPU snapshots include any work between gates. RSS snapshots are not continuous peaks.',
        'server_command':server_command,'client_command':client_command,'server_sha256':sha(args.server),
        'client_sha256':sha(args.client),'client_source_sha256':sha(ROOT/'examples/queue_waves.rs'),
        'runner_sha256':sha(__file__),'profile_diagnostic':bool(args.heap_prefix),'events':[],'passed':False}
    if args.heap_prefix:
        library=Path(env['LD_PRELOAD']);record['allocator']={'path':str(library),'sha256':sha(library)}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    client = None
    with tempfile.TemporaryFile() as logs, tempfile.TemporaryFile() as errors:
        server = subprocess.Popen(server_command,env=env,stdout=logs,stderr=logs)
        try:
            deadline=time.monotonic()+5
            while True:
                assert server.poll() is None
                try:runtime.metrics(admin);break
                except OSError:
                    assert time.monotonic()<deadline;time.sleep(.02)
            client_env={k:v for k,v in os.environ.items() if not k.startswith('WAR_')}
            client=subprocess.Popen(client_command,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=errors,text=True,env=client_env)
            started=time.monotonic()
            while True:
                assert time.monotonic()-started<180, 'outer deadline'
                assert select.select([client.stdout],[],[],60)[0], 'client gate deadline'
                line=client.stdout.readline();assert line, 'client ended before final gate'
                event=json.loads(line);counts=event['counts'];done=event['event']=='done';queued=event['event']=='queued'
                expected={'war_accepted_total':counts['joins'],'war_joined_total':counts['joins'],
                    'war_disconnected_total':counts['normal_closes'],'war_incoming_messages_total':counts['incoming_messages'],
                    'war_incoming_bytes_total':counts['incoming_bytes'],'war_connections':0 if done else args.receivers+1,
                    'war_sessions':0 if done else args.receivers+1,'war_channels':1 if done else 2,'war_variable_bytes':0}
                if not queued:
                    expected.update(war_queued_bytes=0,war_deliveries_total=counts['deliveries'],war_delivered_bytes_total=counts['delivered_bytes'])
                deadline=time.monotonic()+5
                while True:
                    metrics={k:int(v) for k,v in runtime.metrics(admin).items()}
                    if all(metrics[k]==value for k,value in expected.items()):break
                    assert time.monotonic()<deadline,{k:(metrics[k],v) for k,v in expected.items() if metrics[k]!=v}
                    time.sleep(.01)
                assert metrics['war_routing_reused_bytes_total']+metrics['war_routing_copied_bytes_total']==counts['value_bytes']
                for key in ['war_write_timeouts_total','war_write_errors_total','war_aborted_writes_total','war_close_failures_total',
                            'war_transport_disconnects_total','war_slow_consumers_total','war_protocol_errors_total',
                            'war_global_pressure_total','war_read_buffer_reservations_rejected_total']:
                    assert metrics[key]==0,(event['event'],key,metrics[key])
                if done:assert metrics['war_read_buffer_reserved_bytes']==0
                event.update(metrics=metrics,process=runtime.snapshot(server.pid),exact_counter_checks=expected)
                if args.heap_prefix:
                    time.sleep(.05)
                    event['process']['heap']=inspection.dump_heap(server.pid,prefix)
                record['events'].append(event)
                client.stdin.write('continue\n');client.stdin.flush()
                if done:break
            assert client.wait(timeout=5)==0
            record['client_exit_code']=client.returncode
            server.send_signal(signal.SIGINT);assert server.wait(timeout=5)==0
            record['server_exit_code']=server.returncode
            assert any(e['event']=='queued' and e['metrics']['war_queued_bytes']>0 for e in record['events']), 'backpressure was not observed'
            assert len(record['events'])==2*args.waves+3
            record['passed']=True
        finally:
            if client is not None and client.poll() is None:client.kill();client.wait(timeout=5)
            if server.poll() is None:server.send_signal(signal.SIGINT);server.wait(timeout=5)
            errors.seek(0);logs.seek(0)
            record.update(client_stderr=errors.read().decode(errors='replace'),server_log=logs.read().decode(errors='replace'))
            args.output.write_text(json.dumps(record,indent=2)+'\n')
    print(json.dumps({'passed':record['passed'],'events':len(record['events']),'output':str(args.output)}))


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server',type=Path,default=runtime.SERVER)
    parser.add_argument('--client',type=Path,default=ROOT/'target/release/examples/queue_waves')
    parser.add_argument('--receivers',type=int,default=32)
    parser.add_argument('--waves',type=int,default=3)
    parser.add_argument('--heap-prefix',type=Path)
    parser.add_argument('--output',type=Path,required=True)
    run(parser.parse_args())
