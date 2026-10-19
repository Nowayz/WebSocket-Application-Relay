#!/usr/bin/env python3
"""Plot per-witness worst RTT without replacing prior output."""
from pathlib import Path
from analyze import departure
ROOT = Path(__file__).resolve().parent
output = ROOT / 'comparison.png'
if output.exists() or output.is_symlink():
    raise SystemExit('refusing to replace comparison.png')
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
cases = ['departure-control-first','departure-selected-first','departure-selected-repeat','departure-control-last']
fig, axes = plt.subplots(1, 2, figsize=(12, 4.5), sharey=True)
for column, ax in enumerate(axes):
    for series, name in enumerate(cases):
        values = departure(name)['events'][column+1]['per_probe_max_ms']
        ax.bar([i+(series-1.5)*.2 for i in range(8)], values, width=.19,
               color=['#b03a48','#16856a','#69b89c','#de8b94'][series],
               label=['Control first','Selected first','Selected repeat','Control last'][series])
    ax.set_title(f'Large-room departure {column+1}')
    ax.set_xticks(range(8), [str(i+1) for i in range(8)])
    ax.set_xlabel('Unrelated room / witness')
    ax.set_ylabel('Worst observed round trip (ms)')
    ax.grid(axis='y', alpha=.2)
axes[0].legend(fontsize=8)
fig.suptitle('Bounded retirement work removes the large departure stall\n100,000 resident recipients; loopback RTT includes client and kernel scheduling')
fig.tight_layout()
fig.savefig(output, dpi=150)
