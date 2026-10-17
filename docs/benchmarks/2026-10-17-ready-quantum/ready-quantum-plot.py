from pathlib import Path
import csv
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(__file__).resolve().parent
fig,ax=plt.subplots(figsize=(10,5.5),layout='constrained')
for kind,rep,color,style in [('baseline',1,'#ba4d35','-'),('baseline',2,'#ba4d35','--'),('quantum',1,'#127b82','-'),('quantum',2,'#127b82','--')]:
    with (root/f'ready-quantum-latency-{kind}-{rep}.csv').open() as f:
        rows=list(csv.DictReader(f))
    assert len(rows)==100000 and all(int(r['samples'])==120 for r in rows)
    values=sorted(float(r['p99_ms']) for r in rows)
    ax.plot(values,[(i+1)/1000 for i in range(len(values))],label=f"{'64 sockets/batch' if kind=='quantum' else '1024 sockets/batch'} run {rep}",color=color,linestyle=style,linewidth=2)
ax.set(title='Ready-socket batch experiment across 100,000 users',xlabel="Each user's p99 send-to-receive latency (ms) — lower is better",ylabel='Users at or below latency (%)',ylim=(0,100),xlim=(0,None))
ax.grid(alpha=.2);ax.legend(loc='lower right')
fig.suptitle('400,000 messages/s · 1 KiB · 30 seconds · 120 samples/user · heartbeat 15s',fontsize=10)
fig.savefig(root/'ready-quantum-user-latency.png',dpi=180)
fig.savefig(root/'ready-quantum-user-latency.pdf')
