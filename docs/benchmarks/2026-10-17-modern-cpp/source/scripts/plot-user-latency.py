#!/usr/bin/env python3
"""Plot per-user fairness from capacity diagnostic CSVs; no pooled-percentile substitution."""
import argparse
import csv
import json
from pathlib import Path
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--normal',type=Path,required=True)
parser.add_argument('--overload',type=Path,required=True)
parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args()
plt.rcParams.update({'font.family':'DejaVu Sans','font.size':10,'axes.spines.top':False,'axes.spines.right':False})
data={}
summary={}
for label,path in [('Normal',args.normal),('Overload',args.overload)]:
    with path.open() as stream: rows=list(csv.DictReader(stream))
    rows.sort(key=lambda r:(int(r['room']),int(r['member'])))
    data[label]={key:np.array([float(r[key]) for r in rows]) for key in rows[0] if key!='id'}
    summary[label]={'users':len(rows),'samples_per_user':sorted(set(int(r['samples']) for r in rows)),
        'user_p99_ms':dict(zip(['minimum','median','p95','p99','maximum'],np.quantile(data[label]['p99_ms'],[0,.5,.95,.99,1]).tolist())),
        'worst_observed_packet_ms':float(data[label]['max_ms'].max()),
        'users_with_p99_below_ms':{str(limit):float(np.mean(data[label]['p99_ms']<=limit)*100) for limit in [1,2,5,10,20,50,100,250,500]},
        'schedule_p99_ms':dict(zip(['median','p95','p99','maximum'],np.quantile(data[label]['schedule_p99_ms'],[.5,.95,.99,1]).tolist()))}
fig,axes=plt.subplots(2,2,figsize=(15,10),layout='constrained')
colors={'p50_ms':'#2374ab','p99_ms':'#e07a24','max_ms':'#b52e49'}
for ax,label in zip(axes[0],data):
    for key,title in [('p50_ms','Each user’s median'),('p99_ms','Each user’s p99'),('max_ms','Each user’s worst packet')]:
        values=np.sort(data[label][key]); ax.plot(values,np.arange(1,len(values)+1)/len(values)*100,label=title,color=colors[key],lw=2)
    ax.set(xscale='log',xlabel='Observed send-to-receive latency (ms, logarithmic)',ylabel='Connected users at or below latency (%)',ylim=(0,100),title=f'{label}: distribution across all 100,000 users')
    ax.grid(alpha=.2,which='both'); ax.legend(loc='lower right',fontsize=9)
ax=axes[1,0]
normal=data['Normal']; rank=np.arange(len(normal['p99_ms']))
ax.scatter(rank,normal['p99_ms'],s=1,alpha=.28,color='#2374ab',rasterized=True)
ax.set(xlabel='User index ordered by room and member (all 100,000)',ylabel='User p99 send-to-receive latency (ms)',title='Normal load: does a subset of users get worse latency?')
ax.grid(alpha=.2)
ax=axes[1,1]
for label,color in [('Normal','#2374ab'),('Overload','#b52e49')]:
    values=np.sort(data[label]['schedule_p99_ms']);ax.plot(values,np.arange(1,len(values)+1)/len(values)*100,label=label,color=color,lw=2)
ax.set(xscale='log',xlabel='User p99 intended-send-to-receive latency (ms, log)',ylabel='Connected users at or below latency (%)',ylim=(0,100),title='Include waiting when the generator falls behind')
ax.grid(alpha=.2,which='both');ax.legend()
fig.suptitle('Realtime relay latency fairness · selected Mio runtime · 100,000 connected users',fontsize=17,fontweight='bold')
fig.supxlabel('1 KiB private messages, 16-user rooms, 4 server workers / 8 client cores. Normal: 2 packets/user/s × 60 s; overload: 10 × 10 s.\nExact per-user samples; empirical tails from 120 / 100 packets each. Local loopback includes client scheduling and batched sends.',fontsize=10)
args.output.parent.mkdir(parents=True,exist_ok=True)
fig.savefig(args.output.with_suffix('.png'),dpi=160)
fig.savefig(args.output.with_suffix('.pdf'))
args.output.with_suffix('.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps(summary,indent=2))
