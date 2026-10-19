#!/usr/bin/env python3
"""Plot finite populations; never pool trials or imply long-run per-user p99."""
import csv
import gzip
import io
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
matplotlib.rcParams['svg.hashsalt'] = 'war-single-queued-write'

ROOT = Path(__file__).resolve().parent
fig, axes = plt.subplots(1, 2, figsize=(12, 4.8), layout='constrained')
series = [('control', 'Control', '#386cb0', '-'),
          ('direct', 'Inlined, unselected', '#888888', '--'),
          ('outline', 'Out-of-line, selected', '#16836b', '-')]
for name, label, color, style in series:
    data = gzip.decompress((ROOT / 'raw' / f'single-write-100k-{name}.csv.gz').read_bytes()).decode()
    rows = list(csv.DictReader(io.StringIO(data)))
    assert len(rows) == 100000 and all(int(r['samples']) == 24 for r in rows)
    values = sorted(float(r['max_ms']) for r in rows)
    ranks = [(i + 1) * 100 / len(values) for i in range(len(values))]
    for ax in axes:
        ax.plot(ranks, values, color=color, linestyle=style, linewidth=1.8, label=label)
        ax.grid(alpha=.22)
        ax.set_xlabel('User population rank (%)')
        ax.set_ylabel('User maximum observed response (ms)')
        ax.set_ylim(0, 15)
axes[0].set_xlim(0, 100)
axes[0].set_title('100,000 private users')
axes[1].set_xlim(95, 100)
axes[1].set_title('Slowest 5% of each trial')
axes[0].legend(loc='upper left', fontsize=9)
fig.suptitle('Private-user timing: six-second trials, 24 observations per user', fontsize=14)
fig.get_layout_engine().set(rect=(0, .07, 1, .93))
fig.text(.5, .015, 'Each curve is one independent population. Observed maxima are not long-term p99 or latency guarantees.', ha='center', fontsize=9)
fig.savefig(ROOT / 'population.png', dpi=180)
fig.savefig(ROOT / 'population.svg', metadata={'Date': None})
