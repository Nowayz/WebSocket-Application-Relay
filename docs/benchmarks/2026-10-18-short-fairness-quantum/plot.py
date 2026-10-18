"""Plot finite per-user maxima; run from any directory after verify.py."""
from pathlib import Path
import csv,gzip,io,json
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(__file__).resolve().parent
fig,axes=plt.subplots(1,2,figsize=(11,4.7),layout='constrained')
for i,q in enumerate([1024,512,512,1024]):
 prefix=f'short-fairness-quantum-{i}-{q}';color='#277da8' if q==1024 else '#d35c31';style='-' if i<2 else '--'
 for ax,suffix in zip(axes,['.csv.gz','-fanout.csv.gz']):
  with gzip.open(root/'raw'/(prefix+suffix),'rt') as f:rows=list(csv.DictReader(f))
  values=sorted(float(r['max_ns'])/1e6 if 'max_ns' in r else float(r['max_ms']) for r in rows);n=len(values)
  # Audience CSV uses nanoseconds; retain the same physical milliseconds.
  if not values:raise ValueError('empty population')
  ax.plot([100*(j+1)/n for j in range(n)],values,color=color,linestyle=style,label=f'{q} recipients, trial {i+1}',linewidth=1.5)
for ax,title in zip(axes,['Private users: 24 observations each','Fanout recipients: 384 observations each']):
 ax.set(title=title,xlabel='Percentile across connected users',ylabel='Worst observed send-relative latency (ms)',xlim=(80,100),ylim=(0,None));ax.grid(alpha=.25);ax.legend(fontsize=8)
axes[1].set_ylabel('Worst observed fanout-window latency (ms)')
fig.suptitle('100,000 private users + 8,192 fanout recipients; six-second traffic phases\n512-recipient quantum rejected; finite loopback observations, not latency guarantees',fontsize=11)
fig.savefig(root/'fairness.png',dpi=160)
