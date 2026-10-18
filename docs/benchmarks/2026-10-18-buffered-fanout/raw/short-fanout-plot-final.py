import csv
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
fig,axes=plt.subplots(1,2,figsize=(12.8,5.4),layout='constrained')
for name,label,color in [('policy-control-30','Previous selected Rust','#b66526'),('policy-30','Buffered multicast Rust','#156fa8')]:
 for ax,suffix,column,scale in [(axes[0],'-fanout','p99_ns',1e6),(axes[1],'','p99_ms',1)]:
  rows=list(csv.DictReader(open(f'artifacts/short-fanout-{name}{suffix}.csv')))
  values=np.sort([float(r[column])/scale for r in rows])
  y=np.arange(1,len(values)+1)*100/len(values)
  ax.plot(values,y,color=color,lw=2.2,label=label)
for ax in axes:
 ax.set_ylim(0,100.5);ax.grid(alpha=.2);ax.set_xlabel("Recipient's exact p99 latency (ms)");ax.set_ylabel('Percentage of recipients, weighted equally')
 ax.axhline(99,color='#888888',lw=.8,ls=':');ax.legend(loc='lower right',fontsize=9)
axes[0].set_title('8,192 broadcast recipients')
axes[1].set_title('All 100,000 private users')
fig.suptitle('Already-buffered multicast improves latency at the same offered load',fontsize=15,weight='bold')
fig.supxlabel('30 s per runtime · 120 private probes/user · 4 broadcasts/window at 16 Hz · exact payload/FIFO checks\nNative plaintext loopback · broadcast latency starts at window send · finite observations, not a latency guarantee',fontsize=9)
fig.savefig('artifacts/short-fanout-policy-fairness.png',dpi=175)
fig.savefig('artifacts/short-fanout-policy-fairness.pdf')
