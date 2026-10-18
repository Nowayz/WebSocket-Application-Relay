import csv,json
from pathlib import Path
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
colors={'before':'#c56b22','after':'#1771b8','cpp':'#368449'}
labels={'before':'Previous Rust','after':'Revised Rust','cpp':'Modern C++'}
fig,axes=plt.subplots(1,2,figsize=(13.5,5.2),layout='constrained')
for kind in ['before','cpp','after']:
 for repeat in [1,2]:
  rows=list(csv.DictReader(open(f'artifacts/mixed100k-final-{kind}-{repeat}.csv')))
  values=np.sort([float(row['p99_ms']) for row in rows]);percent=np.arange(1,len(values)+1)*100/len(values)
  for ax in axes:
   ax.plot(values,percent,color=colors[kind],ls='-' if repeat==1 else '--',lw=1.5,alpha=.9,label=labels[kind]+f' · run {repeat}')
for ax in axes:
 ax.set_xscale('log');ax.grid(True,which='both',alpha=.18);ax.set_xlabel("Each user's p99 latency (ms, log scale)")
 ax.set_ylabel('Percentage of the 100,000 private users')
axes[0].set_ylim(0,100);axes[0].set_title('Every user, weighted equally')
axes[1].set_ylim(99,100);axes[1].set_title('Slowest 1% of users');axes[1].legend(loc='lower right',fontsize=9)
fig.suptitle('100,000 users under matched mixed load',fontsize=16,weight='bold')
fig.supxlabel('120 checked 1 KiB probes per user per run · 8,192 room readers · 524,288 fanout deliveries/s offered · 32 churn cycles/s · two inspectors',fontsize=10)
fig.savefig('artifacts/mixed100k-user-fairness.png',dpi=180);fig.savefig('artifacts/mixed100k-user-fairness.pdf')
plt.close(fig)
fig,axes=plt.subplots(1,2,figsize=(13.5,4.6),sharey=True,layout='constrained')
for kind in ['before','cpp','after']:
 for repeat in [1,2]:
  row=json.load(open(f'artifacts/mixed100k-final-{kind}-{repeat}.windows.json'))
  x=[w['intended_second'] for w in row['windows']]
  for ax,field in zip(axes,['send_to_receive','intended_to_receive']):
   ax.plot(x,[w[field]['p99_ms'] for w in row['windows']],color=colors[kind],ls='-' if repeat==1 else '--',label=f'{labels[kind]} · {repeat}',alpha=.8)
for ax,title in zip(axes,['Send to receive','Intended send to receive (includes scheduling)']):
 ax.set_title(title);ax.set_xlabel('Intended-send cohort (second)');ax.set_ylabel('All-user cohort p99 (ms)');ax.grid(alpha=.2)
axes[1].legend(fontsize=8)
fig.suptitle('Latency through the entire measured interval',fontsize=16,weight='bold')
fig.savefig('artifacts/mixed100k-timeline.png',dpi=180);fig.savefig('artifacts/mixed100k-timeline.pdf')
