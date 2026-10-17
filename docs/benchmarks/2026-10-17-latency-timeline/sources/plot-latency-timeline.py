#!/usr/bin/env python3
"""Plot post-measurement capacity latency cohorts and observed Ping concentration."""
import argparse
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--case', action='append', required=True, metavar='LABEL=WINDOWS_JSON')
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
cases = []
for item in args.case:
    label, separator, path = item.partition('=')
    if not separator:
        parser.error('--case requires LABEL=WINDOWS_JSON')
    cases.append((label, json.loads(Path(path).read_text())['windows']))
columns = min(2, len(cases))
rows = (len(cases) + columns - 1) // columns
fig, axes = plt.subplots(rows, columns, figsize=(14, rows * 4.1), layout='constrained', squeeze=False)
maximum = max(w['intended_to_receive']['p99_ms'] for _, windows in cases for w in windows)
summary = {}
for ax, (label, windows) in zip(axes.flat, cases):
    seconds = [w['intended_second'] + .5 for w in windows]
    for field, text, color, style in [('send_to_receive','Send → receive p99','#2374ab','-'),('intended_to_receive','Intended → receive p99','#dc7a25','-'),('pre_send_delay','Intended → actual send p99','#666666','--')]:
        ax.plot(seconds, [w[field]['p99_ms'] for w in windows], label=text, color=color, ls=style, lw=1.8)
    twin = ax.twinx()
    twin.bar(seconds, [w['pings_encountered'] for w in windows], width=.85, color='#3c9d65', alpha=.18, label='Pings encountered')
    twin.set_ylim(0, 110000)
    twin.set_ylabel('Pings encountered in cohort', color='#28804c', fontsize=9)
    twin.tick_params(axis='y', labelsize=8, colors='#28804c')
    ax.set(title=label, xlabel='Intended-send cohort, seconds after measurement start', ylabel='Exact cohort p99 latency (ms)', ylim=(0, maximum * 1.12), xlim=(0, len(windows)))
    ax.grid(alpha=.2)
    if ax is axes.flat[0]:
        handles, labels = ax.get_legend_handles_labels()
        other, other_labels = twin.get_legend_handles_labels()
        ax.legend(handles + other, labels + other_labels, fontsize=8, loc='upper left')
    peak = max(windows, key=lambda w:w['send_to_receive']['p99_ms'])
    summary[label] = {'pings_encountered':sum(w['pings_encountered'] for w in windows), 'worst_cohort_p99':peak, 'ping_cohorts':[w['intended_second'] for w in windows if w['pings_encountered']]}
for ax in list(axes.flat)[len(cases):]:
    ax.set_visible(False)
fig.suptitle('Latency over time · 100,000 users · 400,000 offered packets/s · unchanged Mio server', fontsize=16, fontweight='bold')
fig.supxlabel('1 KiB private traffic, four server workers / eight client cores. Each panel: 30 s, 12 million verified packets.\nOne-second intended-send cohorts; Pings counted while reading those probes. Includes client scheduling and room batching; no isolated server-time claim.', fontsize=9)
args.output.parent.mkdir(parents=True, exist_ok=True)
fig.savefig(args.output.with_suffix('.png'), dpi=160)
fig.savefig(args.output.with_suffix('.pdf'))
args.output.with_suffix('.json').write_text(json.dumps(summary, indent=2) + '\n')
