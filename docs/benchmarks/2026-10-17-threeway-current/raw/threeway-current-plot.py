#!/usr/bin/env python3
"""Matched process medians and every repeat; no historical rates mixed in."""
import json
from pathlib import Path
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
import numpy as np
ROOT=Path(__file__).resolve().parents[1]
d=json.loads((ROOT/'artifacts/threeway-current-summary.json').read_text())
fig,axs=plt.subplots(1,2,figsize=(12.8,5.1),layout='constrained')
colors=['#566779','#2878b8','#d48828']
labels=['Original GitHub C++','Current Rust','Current C++']
cases=['private','query','fanout'];names=['Private forwarding','Private + queries','Fanout (63 readers)']
for index,kind in enumerate(['cpp','rust','modern']):
    x=np.arange(3)+(index-1)*.24
    for ax,metric,scale in [(axs[0],'per_second',1e-6),(axs[1],'cpu_us',1)]:
        med=[d['summary'][case+'/'+kind][metric]['median']*scale for case in cases]
        ax.bar(x,med,width=.22,color=colors[index],label=labels[index],alpha=.85)
        for j,case in enumerate(cases):
            values=[r[metric]*scale for r in d['rows'] if r['case']==case and r['kind']==kind]
            ax.scatter(x[j]+np.linspace(-.065,.065,len(values)),values,color='#17212b',s=12,zorder=4,alpha=.7)
        ax.set_xticks(range(3),names);ax.grid(axis='y',alpha=.2);ax.set_axisbelow(True)
        ax.spines[['top','right']].set_visible(False)
axs[0].set_ylabel('Million verified deliveries / second — higher is better')
axs[1].set_ylabel('Server CPU microseconds / delivery — lower is better')
axs[1].legend(fontsize=9,loc='upper right')
fig.suptitle('Original relay vs today’s selected Rust and accepted C++',fontsize=15)
fig.supxlabel('Bars: medians of 6 fresh trials. Dots: all repeats. 4 server cores · 1 KiB values · 8,192 idle rooms\nTimed workload only; exact payload/order checked. Native protocols; no bridge. Loopback, 6 seconds/trial.\nQuery: one Rust repeat missed 1 of 1,200 scheduled queries; its timed data is retained.',fontsize=9)
for suffix in ['png','pdf']:fig.savefig(ROOT/f'artifacts/threeway-current-comparison.{suffix}',dpi=180)
