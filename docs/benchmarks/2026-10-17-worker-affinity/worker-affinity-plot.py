from pathlib import Path
import csv
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
root=Path(__file__).resolve().parent
fig,ax=plt.subplots(figsize=(10,5.5),layout='constrained')
for kind,rep,color,style in [('none',1,'#ba4d35','-'),('none',2,'#ba4d35','--'),('native',1,'#127b82','-'),('native',2,'#127b82','--')]:
    with (root/f'worker-affinity-latency-{kind}-{rep}.csv').open() as f:
        rows=list(csv.DictReader(f))
    assert len(rows)==100000 and all(int(r['samples'])==120 for r in rows)
    values=sorted(float(r['p99_ms']) for r in rows)
    ax.plot(values,[(i+1)/1000 for i in range(len(values))],label=f"{'Pinned' if kind=='native' else 'Unpinned'} run {rep}",color=color,linestyle=style,linewidth=2)
ax.set(title='CPU placement improves latency across 100,000 users',xlabel="Each user's p99 send-to-receive latency (ms) — lower is better",ylabel='Users at or below latency (%)',ylim=(0,100),xlim=(0,None))
ax.grid(alpha=.2);ax.legend(loc='lower right')
fig.suptitle('400,000 messages/s · 1 KiB · 30 seconds · 120 samples/user · heartbeat 15s',fontsize=10)
fig.savefig(root/'worker-affinity-user-latency.png',dpi=180)
fig.savefig(root/'worker-affinity-user-latency.pdf')
