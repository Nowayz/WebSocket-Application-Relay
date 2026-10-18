import csv
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
fig,axes=plt.subplots(1,2,figsize=(13.2,5.8))
for names,label,color in [(['private120-control','policy-control-30'],'Current selected Rust','#b66526'),(['ready64-batch-30','ready64-batch-30-repeat'],'Unselected batching + ready 64','#156fa8')]:
 for i,name in enumerate(names):
  rows=list(csv.DictReader(open(f'artifacts/short-fanout-{name}.csv')))
  for ax,column in zip(axes,['p99_ms','schedule_p99_ms']):
   a=np.sort([float(r[column]) for r in rows]); y=np.arange(1,len(a)+1)*100/len(a)
   ax.plot(a,y,color=color,ls='-' if i==0 else '--',lw=1.6,label=f'{label} · run {i+1}')
for ax in axes:
 ax.set_ylim(95,100.05);ax.grid(alpha=.2);ax.set_xlabel("Each user's exact p99 (ms)")
 ax.set_ylabel('Percentile across all 100,000 private users')
axes[0].set_xlim(3,12);axes[0].set_xticks([3,4,5,6,8,10,12])
axes[1].set_xlim(4,26);axes[1].set_xticks([4,6,8,10,15,20,25])
fig.subplots_adjust(left=.07,right=.99,top=.82,bottom=.29,wspace=.23)
fig.legend(*axes[0].get_legend_handles_labels(),loc='center',bbox_to_anchor=(.5,.15),ncol=2,fontsize=9)
axes[0].set_title('Send → receive: slowest 5%')
axes[1].set_title('Intended send → receive: slowest 5%')
fig.suptitle('CPU savings did not establish consistent private-user tails',fontsize=15,weight='bold')
fig.text(.5,.025,'Two 30 s runs per build · 120 probes/user · 8,192 broadcast recipients · 16 windows/s\nEvery user included equally; no cohort or outlier removed · candidate remains unselected',fontsize=9,ha='center',va='bottom')
fig.savefig('artifacts/short-fanout-private-tail.png',dpi=175);fig.savefig('artifacts/short-fanout-private-tail.pdf')
