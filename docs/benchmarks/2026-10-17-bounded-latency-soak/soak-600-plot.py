from pathlib import Path
import csv,json
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
run=json.loads(Path('artifacts/soak-600-bounded.json').read_text());rows=list(csv.DictReader(Path('artifacts/soak-600-bounded-users.csv').open()))
fig,axes=plt.subplots(1,2,figsize=(13,5),layout='constrained')
for role in ['server','client']:
 series=run['management_samples'];t0=run['events'][1]['seconds'];axes[0].plot([(s['seconds']-t0)/60 for s in series],[s[role+'_rss_kib']/1024 for s in series],label=role.title())
axes[0].set(xlabel='Minutes after ready gate',ylabel='Process RSS (MiB)',title='Process memory during the ten-minute run');axes[0].legend()
for field,label in [('p99_upper_ms','From actual send'),('schedule_p99_upper_ms','From intended send')]:
 values=sorted(float(r[field]) for r in rows);axes[1].step(values,[100*(i+1)/len(values) for i in range(len(values))],where='post',label=label)
axes[1].set(xlabel="Upper bound of each user's p99 (ms)",ylabel='Users at or below bound (%)',title='Per-user histogram bounds; not exact percentiles',ylim=(0,100));axes[1].legend()
for ax in axes:ax.grid(alpha=.2)
fig.suptitle('100,000 users · 400k application messages/s · 600 seconds · selected relay unchanged')
fig.savefig('artifacts/soak-600-results.png',dpi=180);fig.savefig('artifacts/soak-600-results.pdf')
