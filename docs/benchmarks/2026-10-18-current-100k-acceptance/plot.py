"""Plot equal-weight finite user maxima for private and fanout traffic."""
import csv
import gzip
import io
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
matplotlib.rcParams['svg.hashsalt'] = 'war-current-100k-acceptance'
ROOT = Path(__file__).resolve().parent
fig, axes = plt.subplots(1, 2, figsize=(12, 5.6))
for ax, suffix, count, samples, field, scale, label, color in [
    (axes[0], '.csv', 100000, 24, 'max_ms', 1, 'Private users — 24 observations each', '#16836b'),
    (axes[1], '-fanout.csv', 8192, 384, 'max_ns', 1e-6, 'Fanout recipients — 384 observations each', '#386cb0')]:
    data = gzip.decompress((ROOT / 'raw' / ('current-100k-mixed' + suffix + '.gz')).read_bytes()).decode()
    rows = list(csv.DictReader(io.StringIO(data)))
    assert len(rows) == count and all(int(row['samples']) == samples for row in rows)
    values = sorted(float(row[field]) * scale for row in rows)
    ranks = [(i+1)*100/count for i in range(count)]
    ax.plot(ranks, values, color=color, linewidth=2)
    ax.set(xlim=(0,100), ylim=(0,values[-1]*1.15), title=label,
           xlabel='User population rank (%)', ylabel='User maximum observed response (ms)')
    ax.grid(alpha=.22)
    median = values[(count-1)//2]
    ax.axhline(median, color=color, linestyle='--', linewidth=.8, alpha=.55)
    ax.text(3, median + values[-1]*.035, f'Median user: {median:.2f} ms', color=color, fontsize=10)
    ax.text(.04,.94,f'Worst user: {values[-1]:.2f} ms',transform=ax.transAxes,va='top',fontsize=10)
fig.suptitle('Selected Rust: 100,000 private users + 8,192 fanout recipients',fontsize=14)
fig.subplots_adjust(left=.07, right=.985, bottom=.22, top=.82, wspace=.22)
fig.text(.5,.025,'One six-second offered workload. Finite maxima include client/kernel scheduling; they are not latency guarantees.\nEach user contributes equally. Panels use different latency scales.',ha='center',fontsize=9)
fig.savefig(ROOT / 'population.png',dpi=180)
fig.savefig(ROOT / 'population.svg',metadata={'Date':None})
