from pathlib import Path
import hashlib,json,os,subprocess,shutil
root=Path.cwd();dest=root/'artifacts/candidates/short-fanout-policy-final'
shutil.copytree(root/'artifacts/rust-fanout-work/examples',dest/'examples',dirs_exist_ok=True)
record={'base_revision':'279c3c1836c806ecf3343a2f00a89fac7cead022','selected_revision':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'binary_sha256':hashlib.sha256((dest/'war-relay').read_bytes()).hexdigest(),'build_command':'CARGO_TARGET_DIR=target cargo build --manifest-path artifacts/rust-fanout-work/Cargo.toml --release --bin war-relay','rustc':subprocess.check_output(['rustc','-Vv'],text=True),'cargo':subprocess.check_output(['cargo','-V'],text=True),'release_profile':{'lto':'thin','codegen_units':1,'strip':'debuginfo'},'environment_override_present':{k:bool(os.environ.get(k)) for k in ['RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','LD_PRELOAD']},'files':{str(p.relative_to(dest)):hashlib.sha256(p.read_bytes()).hexdigest() for folder in ['src','vendor','examples'] for p in (dest/folder).rglob('*') if p.is_file()}}
(dest/'build.json').write_text(json.dumps(record,indent=2)+'\n')
