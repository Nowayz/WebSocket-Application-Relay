import subprocess,sys
for rate in ['400k','600k']:
 command=[sys.executable,'scripts/plot-user-latency-comparison.py','--subtitle',f'Client placement control · relay unchanged · 100,000 users · {rate} messages/s · 30 seconds']
 for kind,repetition in [('baseline',1),('pinned',1),('pinned',2),('baseline',2)]:
  command+=['--case',f'{"Inherited mask" if kind=="baseline" else "Pinned client"} {repetition}=artifacts/client-placement-{rate}-{kind}-{repetition}.csv']
 command+=['--output',f'artifacts/client-placement-{rate}-latency']
 subprocess.run(command,check=True)
