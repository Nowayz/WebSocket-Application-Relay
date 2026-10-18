from pathlib import Path
import hashlib,json,shutil,subprocess,sys
r=Path('artifacts/rust-fanout-work');o=Path('artifacts/candidates')/sys.argv[1];o.mkdir(exist_ok=False)
shutil.copyfile('target/release/war-relay',o/'war-relay');(o/'war-relay').chmod(0o755);shutil.copytree(r/'src',o/'src')
for name in ['Cargo.toml','Cargo.lock']:shutil.copyfile(r/name,o/name)
d={'base_revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'binary_sha256':hashlib.sha256((o/'war-relay').read_bytes()).hexdigest(),'sources':{str(p.relative_to(r)):hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted((r/'src').rglob('*.rs'))},'compiler':subprocess.check_output(['rustc','-Vv'],text=True),'build_command':'CARGO_TARGET_DIR=/home/phil/dev/war_next/target cargo build --manifest-path artifacts/rust-fanout-work/Cargo.toml --release --bin war-relay','scope':sys.argv[2]};(o/'build.json').write_text(json.dumps(d,indent=2)+'\n')
base=json.loads(Path('artifacts/candidates/observer-quantum/build.json').read_text());assert d['sources']['src/frame.rs']==base['sources']['src/frame.rs'];assert {p for p,v in d['sources'].items() if base['sources'][p]!=v}=={'src/relay.rs','src/server/socket.rs'}
for name in ['capacity','mixed']:
 p=Path(f'artifacts/rust-owner-fanout-isolated-{name}.py');s=p.read_text().replace('artifacts/candidates/rust-owner-fanout-isolated/war-relay',str(o/'war-relay'));Path(f'artifacts/{sys.argv[1]}-{name}.py').write_text(s)
p=Path('artifacts/rust-owner-fanout-pressure-v2.py');s=p.read_text().replace('rust-owner-fanout-isolated/war-relay',sys.argv[1]+'/war-relay').replace('rust-owner-fanout-pressure-v2','rust-owner-fanout-pressure-v3');Path('artifacts/rust-owner-fanout-pressure-v3.py').write_text(s)
