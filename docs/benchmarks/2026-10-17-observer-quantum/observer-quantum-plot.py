from pathlib import Path
import json,subprocess,sys
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
summary=json.loads(Path('artifacts/observer-quantum-summary.json').read_text());assert summary['passed']
fig,axes=plt.subplots(2,2,figsize=(13,8),layout='constrained')
labels=['1 worker / pair 1','1 worker / pair 2','4 workers / pair 1','4 workers / pair 2']
for ax,(group,field,title,ylabel,scale) in zip(axes.flat,[('healthy','p99_upper_ms','Independent private traffic','Reported p99 upper bound (ms)',1),('churn','p99_upper_ms','Join / self-route / Close / EOF','Reported p99 upper bound (ms)',1),('fanout','per_second','Observer fanout throughput','Verified observer deliveries/s (thousands)',1000),('inspection','p99_upper_ms','Authenticated HTTP inspection','Reported p99 upper bound (ms)',1)]):
 for kind,offset,color,label in [('baseline',-.18,'#4C78A8','Baseline'),('candidate',.18,'#F58518','Recipient-work quantum')]:
  values=[next(r['mixed'][group][field] for r in summary['runs'] if r['kind']==kind and r['workers']==workers and r['repeat']==rep)/scale for workers in [1,4] for rep in [1,2]]
  bars=ax.bar([i+offset for i in range(4)],values,width=.34,color=color,label=label)
  ax.bar_label(bars,fmt='%.2f',fontsize=8,padding=3)
 ax.set(xticks=range(4),xticklabels=labels,ylabel=ylabel,title=title,ylim=(0,None));ax.margins(y=.18);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
axes[0,0].legend(fontsize=8)
fig.suptitle('512 global channel observers · one private pair · eight churn workers · two HTTP inspectors\nSix-second matched runs; quantiles are histogram bounds, not hard latency guarantees',fontsize=13)
for ext in ['png','pdf']:fig.savefig('artifacts/observer-quantum-fairness.'+ext,dpi=180)
cmd=[sys.executable,'scripts/plot-user-latency-comparison.py','--subtitle','100,000-user private guard · 400k messages/s · 30 seconds · same exact client']
for kind,rep in [('baseline',1),('candidate',1),('candidate',2),('baseline',2)]:cmd+=['--case',f'{kind.title()} {rep}=artifacts/observer-quantum-400k-{kind}-{rep}.csv']
cmd+=['--output','artifacts/observer-quantum-400k-user-latency'];subprocess.run(cmd,check=True)
