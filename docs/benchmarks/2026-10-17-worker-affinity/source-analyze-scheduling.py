#!/usr/bin/env python3
"""Summarize scoped perf context switches; off-CPU duration is not wakeup latency.

Input: perf script --show-switch-events --show-lost-events --ns
       -F comm,pid,tid,cpu,time,event
Cohort alignment estimates client start as controller continuation + 250 ms.
"""
import argparse
from collections import defaultdict
import json
from pathlib import Path
import re

SWITCH = re.compile(r'^\s*(.*?)\s+(\d+)/(\d+)\s+\[(\d+)\]\s+(\d+)\.(\d{9}):\s+PERF_RECORD_SWITCH\s+(IN|OUT)\s*(preempt)?\s*$')


def quantiles(values):
    ordered = sorted(values)
    def q(percent):
        return ordered[(len(ordered) * percent + 99) // 100 - 1] / 1e6 if ordered else 0.0
    return {'count':len(ordered), 'p50_ms':q(50), 'p95_ms':q(95), 'p99_ms':q(99), 'max_ms':q(100)}


def analyze(run, lines):
    origin = round((run['measurement_continue_monotonic_seconds'] + .250) * 1e9)
    seconds = run['args']['seconds']
    end = origin + seconds * 1_000_000_000
    pids = {pid:role for role,pid in run['profile_pids'].items()}
    pending_out, pending_in, previous_cpu, last_direction = {}, {}, {}, {}
    intervals = defaultdict(lambda:{'off':[], 'preempted':[], 'running':[], 'migrations':0})
    windows = [defaultdict(lambda:{'running_ns':0, 'off':[], 'preempted':[], 'migrations':0}) for _ in range(seconds)]
    names = {}
    counts = defaultdict(int)
    losses = []
    for line in lines:
        if 'PERF_RECORD_LOST' in line:
            losses.append(line.strip())
        if 'PERF_RECORD_SWITCH' not in line:
            continue
        match = SWITCH.match(line)
        if not match:
            raise ValueError('Unrecognized switch record: ' + line[:300])
        comm,pid,tid,cpu,whole,fraction,direction,preempted = match.groups()
        pid,tid,cpu = int(pid),int(tid),int(cpu)
        stamp = int(whole)*1_000_000_000 + int(fraction)
        role = pids.get(pid, 'unknown')
        if role == 'server' and comm.startswith('war-io-'):
            group = 'server_io'
        elif role == 'client' and comm == 'tokio-rt-worker':
            group = 'client_io'
        else:
            group = role
        names[tid] = {'pid':pid, 'comm':comm, 'group':group}
        stats = intervals[tid]
        counts['switch_records'] += 1
        counts[direction.lower()] += 1
        if last_direction.get(tid) == direction:
            counts['duplicate_directions'] += 1
        last_direction[tid] = direction
        if direction == 'OUT':
            if tid in pending_out:
                counts['overwritten_out'] += 1
            pending_out[tid] = (stamp, bool(preempted), group)
            began = pending_in.pop(tid, None)
            if began is not None:
                first,last = max(began,origin),min(stamp,end)
                if last > first:
                    stats['running'].append(last-first)
                    cursor = first
                    while cursor < last:
                        second = (cursor-origin)//1_000_000_000
                        boundary = min(last,origin+(second+1)*1_000_000_000)
                        windows[second][group]['running_ns'] += boundary-cursor
                        cursor = boundary
        else:
            pending_in[tid] = stamp
            previous = previous_cpu.get(tid)
            if previous is not None and previous != cpu and origin <= stamp < end:
                stats['migrations'] += 1
                windows[(stamp-origin)//1_000_000_000][group]['migrations'] += 1
            previous_cpu[tid] = cpu
            out = pending_out.pop(tid, None)
            if out is None:
                counts['unpaired_in'] += 1
                continue
            began,preempted,out_group = out
            if stamp < began:
                raise ValueError('Switch timestamps go backwards within a thread')
            if began < origin or began >= end or stamp > end:
                counts['boundary_off_intervals'] += 1
                continue
            duration = stamp-began
            stats['off'].append(duration)
            window = windows[(began-origin)//1_000_000_000][out_group]
            window['off'].append(duration)
            if preempted:
                stats['preempted'].append(duration)
                window['preempted'].append(duration)
    def report(stats):
        return {'off_cpu':quantiles(stats['off']), 'preempted_off_cpu':quantiles(stats['preempted']),
                'running_ms':sum(stats.get('running',[]))/1e6 + stats.get('running_ns',0)/1e6,
                'migrations':stats['migrations']}
    groups = defaultdict(lambda:{'off':[], 'preempted':[], 'running':[], 'migrations':0})
    for tid,stats in intervals.items():
        target = groups[names[tid]['group']]
        for key in ['off','preempted','running']:
            target[key].extend(stats[key])
        target['migrations'] += stats['migrations']
    return {'scope':'Diagnostic paired switch-out/in durations. Ordinary off-CPU includes voluntary I/O waits. Preempted off-CPU is not directly measured wakeup/run-queue latency. Origin approximates client Instant start from controller continuation + 250ms; cohort-edge alignment is uncertain. Tracing overhead excluded from ordinary performance claims.',
            'estimated_origin_monotonic_ns':origin, 'counts':dict(counts), 'lost_records':losses,
            'groups':{group:report(stats) for group,stats in groups.items()},
            'threads':{str(tid):{**names[tid],**report(stats)} for tid,stats in intervals.items()},
            'windows':[{'second':second, 'groups':{group:report(stats) for group,stats in window.items()}} for second,window in enumerate(windows)]}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--events', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    with args.events.open() as stream:
        result = analyze(json.loads(args.run.read_text()), stream)
    args.output.write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps({'counts':result['counts'],'groups':result['groups'],'lost_records':result['lost_records']},indent=2))


if __name__ == '__main__':
    main()
