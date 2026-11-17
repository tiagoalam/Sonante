#!/usr/bin/env python3
from pathlib import Path
import argparse,hashlib,json,os,subprocess
p=argparse.ArgumentParser();p.add_argument('--verify-only',action='store_true');p.add_argument('--build',action='store_true');a=p.parse_args()
if not a.verify_only and not a.build:p.error('choose --verify-only or --build')
r=Path(__file__).resolve().parent;s=r/'source';m=json.loads((r/'SOURCES.json').read_text())
files={str(p.relative_to(s)):hashlib.sha256(p.read_bytes()).hexdigest() for p in s.rglob('*') if p.is_file() and p.relative_to(s).parts[0] not in ['target','.git']}
assert files==m['files'],'source membership or bytes changed'
for name,key in [('windows-camilla-base2.patch','base_patch_sha256'),('windows-camilla-analyzer3.patch','analyzer_patch_sha256')]:assert hashlib.sha256((r/name).read_bytes()).hexdigest()==m[key]
print('Complete corresponding source/vendor/lock/patch bytes verified; no engine executed.')
if a.build:
 target=r/'build';assert not target.exists(),'use a fresh extraction; preserve prior builds'
 e=os.environ.copy();e['CARGO_TARGET_DIR']=str(target);e['RUSTFLAGS']='-C target-feature=+crt-static'
 subprocess.run(['nice','-n','10','cargo-xwin','build','--manifest-path',str(s/'Cargo.toml'),'--target','x86_64-pc-windows-msvc','--release','--locked','--offline','-j1'],cwd=s,env=e,check=True)
 print('Cross-build only; native behavior and binary byte reproducibility are separate gates.')
