"""Finite departure test with a constrained existing global queue budget."""
import argparse, importlib.util, json
from pathlib import Path
root=Path('/home/phil/dev/war_next')
spec=importlib.util.spec_from_file_location('departure',root/'scripts/departure-fairness.py')
fixture=importlib.util.module_from_spec(spec);spec.loader.exec_module(fixture)
parser=argparse.ArgumentParser();parser.add_argument('--server',type=Path,required=True);parser.add_argument('--output',type=Path,required=True)
args=parser.parse_args();args.audience=100000;args.server_cpus='0-7'
original=fixture.subprocess.Popen
fixture_options=['--max-message-bytes','65536','--queue-bytes','262144','--total-queue-bytes','262144']
class Constrained(original):
    def __init__(self,command,*a,**kw):
        command=command+fixture_options
        self.admin=int(command[command.index('--admin-bind')+1].rsplit(':',1)[1])
        super().__init__(command,*a,**kw)
    def terminate(self):
        try:
            pressure_metrics.append(fixture.wire.runtime.metrics(self.admin))
        except OSError:
            pass
        return super().terminate()
fixture.subprocess.Popen=Constrained
pressure_metrics=[]
with args.output.open('x') as output:
    result=fixture.run(args)
    result['pressure_options']=fixture_options
    result['pressure_metrics']=pressure_metrics
    json.dump(result,output,indent=2);output.write('\n')
print(json.dumps({'passed':result['passed'],'error':result.get('error'),
                  'global_pressure':[x['war_global_pressure_total'] for x in pressure_metrics]}))
raise SystemExit(0 if result['passed'] else 1)
