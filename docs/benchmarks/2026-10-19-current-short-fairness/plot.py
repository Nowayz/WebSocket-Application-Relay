"""Render the original local per-user CSV named in receipt.json."""
import csv
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root = Path(__file__).resolve().parent
receipt = json.loads((root / 'receipt.json').read_text())
command = receipt['client_command']
source = Path(command[command.index('--user-latency-output') + 1])
rows = list(csv.DictReader(source.open()))
assert len(rows) == 100000 and all(int(row['samples']) == 8 for row in rows)
fig, ax = plt.subplots(figsize=(9, 5.6))
fig.subplots_adjust(left=.09, right=.98, top=.88, bottom=.23)
y = [100 * (i + 1) / len(rows) for i in range(len(rows))]
for column, label in [('max_ms', 'From actual send to verified reply'),
                      ('schedule_max_ms', 'From scheduled send to verified reply')]:
    ax.plot(sorted(float(row[column]) for row in rows), y, label=label, linewidth=2)
ax.set(xlabel='Worst observed round trip per user (ms)',
       ylabel='Users at or below this latency (%)', ylim=(0, 100), xlim=(0, 22))
fig.suptitle('100,000 users: each user’s worst observed latency', fontsize=14)
ax.grid(alpha=.25)
ax.legend(loc='lower right')
fig.text(.5, .035, '2-second mixed load · 8 observations/user · 8,192 fanout recipients · 256 churn cycles\n'
         'Loopback fixture; finite observations, not a sustained latency guarantee', ha='center', fontsize=9)
fig.savefig(root / 'per-user-latency.png', dpi=150)
