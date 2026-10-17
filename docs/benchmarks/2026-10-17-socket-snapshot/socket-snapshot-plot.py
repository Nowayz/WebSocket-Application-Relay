import subprocess,sys
for rate in ['400k','600k']:
 command=[sys.executable,'scripts/plot-user-latency-comparison.py','--subtitle',f'100,000 users · {rate} messages/s · 30 seconds · 8 pinned relay workers']
 for kind,repetition in [('baseline',1),('snapshot',1),('snapshot',2),('baseline',2)]:
  command+=['--case',f'{"Baseline" if kind=="baseline" else "Candidate"} {repetition}=artifacts/socket-snapshot-{rate}-{kind}-{repetition}.csv']
 command+=['--output',f'artifacts/socket-snapshot-{rate}-user-latency']
 subprocess.run(command,check=True)
