#!/usr/bin/env python3
"""Render the retained repeat without launching traffic; preserve existing PNG."""
import json
from pathlib import Path
ROOT = Path(__file__).resolve().parent
output = ROOT / 'latency.png'
if output.exists() or output.is_symlink():
    raise SystemExit('refusing to replace latency.png')
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
x = json.loads((ROOT / '100k-repeat.json').read_text())
fig, axes = plt.subplots(2, 2, figsize=(12, 7), sharey=True)
for ax, e in zip(axes.flat, x['events']):
    for i in range(8):
        s = [s for s in x['samples'] if s['probe'] == i and e['start_ms'] <= s['sent_ms'] <= e['end_ms']]
        ax.plot([s['sent_ms']-e['close_sent_ms'] for s in s],
                [s['received_ms']-s['sent_ms'] for s in s], linewidth=.8, label=f'Probe {i+1}')
    ax.axvspan(0, e['close_ack_ms']-e['close_sent_ms'], color='gray', alpha=.18)
    ax.axvline(0, color='black', linewidth=.6)
    ax.set_title(f"{e['label']}: {e['recipients']:,} notification recipients")
    ax.set_xlabel('Request sent relative to Close (ms)')
    ax.set_ylabel('Round-trip latency (ms)')
    ax.grid(alpha=.2)
axes.flat[0].legend(ncol=4, fontsize=7)
fig.suptitle('One large-room departure stalls eight unrelated rooms\n100,000 resident recipients; ordinary native server; loopback RTT includes client scheduling')
fig.tight_layout()
fig.savefig(output, dpi=150)
