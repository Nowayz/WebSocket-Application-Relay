import csv
import numpy as np
import matplotlib
matplotlib.use('Agg')
import matplotlib.pyplot as plt
fig,ax=plt.subplots(figsize=(11,6),layout='constrained')
for name,label,color,style in [('control','Control, first pair','#2474a8','-'),('slot','Slot cache, first pair','#df7525','-'),('slot-reverse','Slot cache, reverse pair','#df7525','--'),('control-reverse','Control, reverse pair','#2474a8','--'),('no-heartbeat','Control, heartbeat disabled (diagnostic)','#555555',':')]:
    with open(f'artifacts/ready-slot-fairness-{name}.csv') as f:values=np.sort([float(r['p99_ms']) for r in csv.DictReader(f)])
    ax.plot(values,np.arange(1,len(values)+1)/len(values)*100,label=label,color=color,ls=style,lw=2)
ax.set(title='100,000 users at 400,000 packets/s: tail latency varies between runs',xlabel='Individual user p99 send-to-receive latency (ms)',ylabel='Connected users at or below latency (%)',ylim=(0,100),xlim=(0,45))
ax.grid(alpha=.2);ax.legend(loc='lower right',fontsize=9)
fig.supxlabel('1 KiB private traffic, 4 server workers, 8 client cores, 30 seconds / 120 samples per user.\nLocal loopback includes client scheduling and room send batching. Two orders; one heartbeat-disabled diagnostic. No causal attribution.',fontsize=9)
fig.savefig('artifacts/ready-slot-fairness.png',dpi=160)
fig.savefig('artifacts/ready-slot-fairness.pdf')
