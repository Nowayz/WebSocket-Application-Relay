import shutil,subprocess,tarfile,tempfile,json
from pathlib import Path
root=Path.cwd();provenance=json.loads((root/'vendor/provenance.json').read_text())
with tempfile.TemporaryDirectory(prefix='war-vendor-patch-') as directory:
 work=Path(directory);(work/'a').mkdir();(work/'b').mkdir()
 for name,info in provenance.items():
  with tarfile.open(root/'target/vendor-source-cache'/f"{name}-{info['version']}.crate") as archive:
   archive.extractall(work/'a',filter='data')
  (work/'a'/f"{name}-{info['version']}").rename(work/'a'/name)
  shutil.copytree(root/'vendor'/name,work/'b'/name)
 result=subprocess.run(['git','diff','--no-index','--','a','b'],cwd=work,text=True,capture_output=True)
 assert result.returncode==1,result.stderr
 patch=result.stdout.replace('a/a/','a/').replace('b/b/','b/')
 (root/'vendor/read-buffer-reclaim.patch').write_text(patch)
