#!/usr/bin/env python3
"""Plot checked timing traces after every ordinary benchmark has stopped."""
import csv, json, pathlib, statistics
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np
ROOT = pathlib.Path(__file__).resolve().parents[1]
OUT = ROOT / 'artifacts'
colors = {'rust': '#2166ac', 'rust-observer-index': '#6a51a3', 'cpp': '#d95f02'}
labels = {'rust': 'Selected Rust', 'rust-observer-index': 'Rust observer candidate', 'cpp': 'Modern C++'}
capacity = json.loads((OUT / 'cpp-modern-accepted-100k.json').read_text())
assert capacity['passed']
fig, axes = plt.subplots(1, 2, figsize=(13.5, 5.3), layout='constrained')
for row in capacity['runs']:
    data = list(csv.DictReader((ROOT / row['csv']).open()))
    for ax, key in zip(axes, ['p99_ms', 'schedule_p99_ms']):
        values = np.sort([float(r[key]) for r in data])
        y = np.arange(1, len(values) + 1) / len(values) * 100
        ax.plot(values, y, color=colors[row['kind']], linestyle='-' if row['repeat'] == 1 else '--', label=f"{labels[row['kind']]} trial {row['repeat']}", linewidth=1.8)
for ax, title in zip(axes, ['Send to receive', 'Intended send to receive']):
    ax.set(xlabel='Per-user p99 latency (ms)', ylabel='Connected users at or below latency (%)', title=title, ylim=(0, 100), xlim=(0, None))
    ax.grid(alpha=0.22)
    ax.axhline(99, color='#777', linewidth=0.7, linestyle=':')
    ax.legend(loc='lower right', fontsize=8)
fig.suptitle('100,000 active users · 400,000 verified probes/s · 1 KiB · 8 server workers\n30 s per trial; 120 samples per user; exact per-user percentiles, two fresh trials each', fontsize=12)
fig.savefig(OUT / 'cpp-modern-user-fairness.png', dpi=180)
fig.savefig(OUT / 'cpp-modern-user-fairness.pdf')
plt.close(fig)
result = json.loads((OUT / 'cpp-modern-final-comparison.json').read_text())
assert result['passed']
fig, axes = plt.subplots(2, 3, figsize=(14, 8), layout='constrained')
for col, mode in enumerate(['traffic', 'fanout', 'churn']):
    for offset, kind in enumerate(['rust', 'rust-observer-index', 'cpp']):
        for wi, w in enumerate([1, 4]):
            rows = [r for r in result['runs'] if r['mode'] == mode and r['kind'] == kind and (r['workers'] == w)]
            throughput = [r['result']['deliveries_per_second'] / 1000000.0 for r in rows]
            cpu = [r['server_cpu_seconds'] / r['result']['verified_deliveries'] * 1000000.0 for r in rows]
            for ax, values in [(axes[0, col], throughput), (axes[1, col], cpu)]:
                middle = statistics.median(values)
                x = wi + (offset - 1) * 0.23
                ax.bar(x, middle, width=0.22, color=colors[kind], label=labels[kind] if wi == 0 else None)
                ax.errorbar(x, middle, yerr=[[middle - min(values)], [max(values) - middle]], color='#333', capsize=3, linewidth=1)
    for row in range(2):
        axes[row, col].set_xticks([0, 1], ['1 worker', '4 workers'])
        axes[row, col].grid(axis='y', alpha=0.22)
    axes[0, col].set_title({'traffic': '32 private pairs', 'fanout': '1 publisher → 63 readers', 'churn': '16 connection-lifecycle workers'}[mode])
    axes[0, col].set_ylabel('Verified operations / s (millions)')
    axes[1, col].set_ylabel('Server CPU µs / verified operation')
axes[0, 0].legend(fontsize=8)
fig.suptitle('Native CBOR saturation workloads · 1 KiB / 16-frame windows\nMedians and full ranges of two 6 s trials; CPU includes setup and cleanup; no profiler or concurrent benchmarks', fontsize=12)
fig.savefig(OUT / 'cpp-modern-throughput.png', dpi=180)
fig.savefig(OUT / 'cpp-modern-throughput.pdf')
plt.close(fig)
mixed = json.loads((OUT / 'cpp-modern-final-mixed.json').read_text())
assert mixed['passed']
fig, axes = plt.subplots(1, 3, figsize=(13.5, 4.8), layout='constrained')
for ax, metric, title in zip(axes, ['healthy', 'churn', 'inspection'], ['Private traffic', 'Connection churn', 'CBOR inspection']):
    for x, kind in enumerate(['rust', 'rust-observer-index', 'cpp']):
        rows = [r for r in mixed['runs'] if r['kind'] == kind]
        values = [r['result'][metric]['p99_upper_ms'] for r in rows]
        median = statistics.median(values)
        ax.bar(x, median, color=colors[kind])
        ax.scatter([x] * len(values), values, color='#222', s=15, zorder=3)
    ax.set_xticks(range(3), ['Rust selected', 'Rust observer\ncandidate', 'C++'])
    ax.set_title(title)
    ax.set_ylabel('p99 upper bound (ms)')
    ax.grid(axis='y', alpha=0.22)
fig.suptitle('Latency while 512 global observers compete with private traffic, churn and inspection\n4 pinned server workers; two 6 s trials; histogram bounds include client windowing\nBars: median of two trials · dots: individual p99 upper bounds', fontsize=12)
fig.savefig(OUT / 'cpp-modern-mixed-latency.png', dpi=180)
fig.savefig(OUT / 'cpp-modern-mixed-latency.pdf')
plt.close(fig)
print('Created fairness, throughput and mixed-load figures')
