import csv,json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np
ROOT=Path(__file__).resolve().parents[1]
d=json.loads((ROOT/'artifacts/rust-owner-fanout-explicit-summary.json').read_text())
colors=['#2878b8','#d48828','#21865b'];kinds=['rust','modern','control'];labels=['Previous Rust','Current C++','New Rust']
fig,axs=plt.subplots(1,2,figsize=(11,4.5),layout='constrained')
for ax,metric,scale,label in [(axs[0],'rate',1e-6,'Million deliveries/sec — higher is better'),(axs[1],'cpu_us',1,'CPU µs/delivery — lower is better')]:
 for i,k in enumerate(kinds):
  v=d['final']['fanout/'+k][metric];ax.bar(i,v['median']*scale,color=colors[i],width=.6);ax.scatter(i+np.linspace(-.1,.1,len(v['all'])),np.array(v['all'])*scale,color='#17212b',s=25,zorder=3)
 ax.set_xticks(range(3),labels);ax.set_ylabel(label);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True);ax.spines[['top','right']].set_visible(False)
fig.suptitle('Rust fanout: publish once, send on recipient workers',fontsize=15)
fig.supxlabel('Final build · 4 server cores · 63 readers · 1 KiB · 8,192 idle rooms · 16-message window\nBars: median of two fresh six-second repeats. Dots: every repeat. Exact payload/order checked.',fontsize=9)
for ext in ['png','pdf']:fig.savefig(ROOT/f'artifacts/rust-owner-fanout-throughput.{ext}',dpi=180)
fig,axs=plt.subplots(1,2,figsize=(11.5,4.8),layout='constrained');maximum=0
for row in d['capacity']:
 path=ROOT/row['csv']
 with path.open() as f:values=np.sort([float(r['p99_ms']) for r in csv.DictReader(f)])
 assert len(values)==100000;maximum=max(maximum,float(values[-1]));y=np.arange(1,len(values)+1)*100/len(values)
 newer=row['kind']=='rust-owner';name=('New Rust' if newer else 'Previous Rust')+f' trial {row["repeat"]}'
 for ax in axs:ax.plot(values,y,color='#21865b' if newer else '#2878b8',linestyle='-' if row['repeat']==1 else '--',linewidth=1.5,label=name)
for ax in axs:
 ax.set_xlabel('Each user’s p99 send-to-receive latency (ms)');ax.set_xlim(0,maximum*1.03);ax.grid(alpha=.2);ax.spines[['top','right']].set_visible(False)
axs[0].set_ylim(0,100);axs[0].set_ylabel('Users at or below latency (%)');axs[0].set_title('Every connected user');axs[0].legend(loc='lower right',fontsize=8)
axs[1].set_ylim(99,100);axs[1].set_title('Slowest 1% of users');axs[1].set_ylabel('Users at or below latency (%)')
fig.suptitle('100,000-user latency fairness after the fanout change',fontsize=15)
fig.supxlabel('400,000 private probes/sec · 30 seconds/run · 120 samples per user · 2 fresh runs/build\nIncludes client scheduling and room batching. Every user is retained; this is not a population-wide packet percentile.',fontsize=9)
for ext in ['png','pdf']:fig.savefig(ROOT/f'artifacts/rust-owner-fanout-user-fairness.{ext}',dpi=180)
