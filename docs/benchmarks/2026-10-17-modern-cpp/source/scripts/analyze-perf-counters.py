#!/usr/bin/env python3
"""Summarize scoped per-thread perf stat CSV, retaining unavailable counts explicitly."""
import argparse
import csv
import json
import math
from pathlib import Path
import re

EVENTS = ('task-clock', 'cycles', 'instructions', 'cache-misses', 'context-switches', 'cpu-migrations')


def analyze(run, lines):
    threads = {t['tid']: dict(t, events={}) for t in run['profile_threads']}
    for row in csv.reader(lines, delimiter=';'):
        if not row or row[0].startswith('#'):
            continue
        if len(row) < 6:
            raise ValueError(f'incomplete counter row: {row}')
        match = re.search(r'-(\d+)$', row[0])
        if match is None or int(match[1]) not in threads:
            raise ValueError(f'unattributed thread: {row[0]}')
        tid = int(match[1])
        event = row[3]
        if event not in EVENTS or event in threads[tid]['events']:
            raise ValueError(f'unknown or duplicate event for {tid}: {event}')
        value = None if row[1].startswith('<') else float(row[1])
        coverage = float(row[5])
        if value is not None and (not math.isfinite(value) or value < 0):
            raise ValueError('invalid counter value')
        if not math.isfinite(coverage) or not 0 <= coverage <= 100:
            raise ValueError('invalid counter running percentage')
        if event == 'task-clock' and row[2] != 'msec':
            raise ValueError('expected task-clock in milliseconds')
        threads[tid]['events'][event] = {'value': value, 'status': row[1] if value is None else 'counted',
                                       'running_percent': coverage}
    groups = {}
    for thread in threads.values():
        if set(thread['events']) != set(EVENTS):
            raise ValueError(f'missing event rows for thread {thread["tid"]}')
        group = groups.setdefault(thread['role'], {'counted_totals': dict.fromkeys(EVENTS, 0.0),
                                                  'counted_threads_per_event': dict.fromkeys(EVENTS, 0),
                                                  'unavailable': [], 'running_percent_min': 100.0})
        for event, entry in thread['events'].items():
            if entry['value'] is None:
                group['unavailable'].append({'tid': thread['tid'], 'event': event, 'status': entry['status']})
            else:
                group['counted_totals'][event] += entry['value']
                group['counted_threads_per_event'][event] += 1
                group['running_percent_min'] = min(group['running_percent_min'], entry['running_percent'])
    for group in groups.values():
        totals = group['counted_totals']
        if not any(group['counted_threads_per_event'].values()):
            group['running_percent_min'] = None
        for event, count in group['counted_threads_per_event'].items():
            if count == 0:
                totals[event] = None
        def ratio(a, b):
            return a / b if a is not None and b else None
        # These ratios describe counted observations only, never impute absent values.
        group['observed_ipc'] = ratio(totals['instructions'], totals['cycles'])
        group['observed_cycles_per_task_clock_ghz'] = ratio(totals['cycles'], totals['task-clock'] * 1e6 if totals['task-clock'] is not None else None)
        group['observed_cache_misses_per_1000_instructions'] = ratio(totals['cache-misses'] * 1000 if totals['cache-misses'] is not None else None, totals['instructions'])
        group['ratios_complete_for_all_threads'] = not group['unavailable']
    accounting = None
    if 'counter_cpu_start' in run:
        start, end = run['counter_cpu_start'], run['counter_cpu_end']
        hz = start['ticks_per_second']
        if hz <= 0 or hz != end['ticks_per_second']:
            raise ValueError('inconsistent CPU tick frequency')
        accounting = {}
        for role, mask in run['counter_cpu_masks'].items():
            delta = [sum(end['cpus'][f'cpu{cpu}'][i] - start['cpus'][f'cpu{cpu}'][i]
                         for cpu in mask) / hz for i in range(8)]
            if any(value < 0 for value in delta):
                raise ValueError('CPU accounting counters decreased')
            clock = groups[role]['counted_totals']['task-clock']
            process = run['measurement'][f'{role}_cpu_seconds']
            accounting[role] = {'cpus': mask,
                'process_cpu_gate_seconds': process,
                'perf_task_clock_seconds': clock / 1000 if clock is not None else None,
                'cpu_set_user_system_seconds': sum(delta[i] for i in [0, 1, 2]),
                'cpu_set_irq_seconds': delta[5], 'cpu_set_softirq_seconds': delta[6],
                'cpu_set_busy_seconds': sum(delta[i] for i in [0, 1, 2, 5, 6]),
                'scope': 'CPU-set totals include other tasks. Perf and process gate boundaries differ. IRQ totals are not exclusive process attribution.'}
    return {'scope': 'Ready-time threads only; kernel/user included. Counted totals and ratios omit unavailable observations, which remain listed. Counter collection is diagnostic and not an ordinary timing comparison. No causal attribution from counters alone.',
            'groups': groups, 'threads': list(threads.values()), 'cpu_accounting': accounting}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--run', type=Path, required=True)
    parser.add_argument('--counters', type=Path, help='Defaults to recorded profile_path')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    run = json.loads(args.run.read_text())
    with (args.counters or Path(run['profile_path'])).open() as stream:
        result = analyze(run, stream)
    args.output.write_text(json.dumps(result, indent=2) + '\n')


if __name__ == '__main__':
    main()
