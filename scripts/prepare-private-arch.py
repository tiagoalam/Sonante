#!/usr/bin/env python3
"""Prepare checksum-pinned makepkg inputs/recipe. Does NOT run makepkg or install.
Requires the final source manifest and distribution-ready runtime stage.
"""
import argparse
import hashlib
import importlib.util
import json
from pathlib import Path
import shlex
import shutil
import re
import subprocess
import tarfile

SOURCE=Path(__file__).resolve().parent.parent
spec=importlib.util.spec_from_file_location('verify_stage',SOURCE/'scripts/verify-private-runtime-stage.py')
verifier=importlib.util.module_from_spec(spec);spec.loader.exec_module(verifier)

def archive(target,root,names,prefix):
    with tarfile.open(target,'w',format=tarfile.PAX_FORMAT) as tar:
        for name in sorted(names):
            path=root/name
            if path.is_symlink() or not path.is_file():raise ValueError('Invalid source input')
            info=tar.gettarinfo(str(path),arcname=prefix+'/'+name)
            info.uid=info.gid=0;info.uname=info.gname='root';info.mtime=0
            with path.open('rb') as stream:tar.addfile(info,stream)
    return verifier.digest(target)

def prepare(stage,source_manifest,sidecar,application,app_receipt,output,check_only=False):
    receipt=verifier.verify(stage) # Reject audit/private-path fixtures BEFORE creating any inputs.
    manifest=json.loads(source_manifest.read_text())
    for name,entry in manifest.items():
        path=Path(name)
        if path.is_absolute() or '..' in path.parts:raise ValueError('Unsafe snapshot path')
        path=SOURCE/path
        if path.is_symlink() or not path.is_file() or path.stat().st_mode & 0o7777 != entry['mode']:
            raise ValueError('Invalid snapshot source or mode')
        if verifier.digest(path)!=entry['sha256']:raise ValueError('Snapshot changed')
    inventory=json.loads((stage/'usr/share/doc/sonante/runtime-inventory.json').read_text())
    identity=inventory['source_sidecar']
    if sidecar.name!=identity['filename'] or verifier.digest(sidecar)!=identity['sha256']:
        raise ValueError('Exact corresponding source sidecar required')
    packages=set(inventory['arch_packages'])-{'glibc','gcc-libs'}
    packages.update(['glibc>=2.35','gcc-libs>=12','alsa-utils','gtk3','openssl','webkit2gtk-4.1>=2.40','xdg-utils','xdg-desktop-portal','pipewire','wireplumber'])
    if any(p.startswith('lib32-') for p in packages):raise ValueError('Wrong dependency architecture')
    app=json.loads(app_receipt.read_text())
    app_hash=verifier.digest(application)
    source_hash=hashlib.sha256(source_manifest.read_bytes()).hexdigest()
    if application.is_symlink() or not application.is_file() or app_hash!=app['sha256'] or source_hash!=app['source_manifest_sha256'] or not app['custom_protocol']:
        raise ValueError('Application does not match the validated Ubuntu source/build receipt')
    data=application.read_bytes()
    if re.search(rb'/home/[^/\x00\s]+/|/build/(?:app-source|app-vendor|app-target)/|/root/\.cargo/',data):raise ValueError('Private application build path')
    versions=subprocess.check_output(['readelf','--version-info',str(application)],text=True)
    required=max(set(re.findall(r'\bGLIBC_(\d+(?:\.\d+)+)',versions)),key=lambda v:tuple(map(int,v.split('.'))))
    if tuple(map(int,required.split('.')))>(2,35):raise ValueError('Application exceeds GLIBC 2.35')
    if check_only:
        print('Exact source/application/runtime/sidecar inputs and ABI verified; no makepkg inputs generated.')
        return
    output.mkdir(parents=True,exist_ok=False)
    source_hash=archive(output/'sonante-private-source.tar',SOURCE,manifest,'sonante-private-source')
    runtime_hash=archive(output/'sonante-private-runtimes.tar',stage,[*receipt,'staging-receipt.json'],'sonante-private-runtimes')
    shutil.copyfile(sidecar,output/sidecar.name)
    shutil.copyfile(application,output/'sonante-private-application');(output/'sonante-private-application').chmod(0o755)
    recipe=(SOURCE/'sonante-arch/PKGBUILD.private.in').read_text().replace('@SOURCE_SHA256@',source_hash).replace('@RUNTIME_SHA256@',runtime_hash).replace('@APPLICATION_SHA256@',app_hash).replace('@SIDECAR_FILE@',sidecar.name).replace('@SIDECAR_SHA256@',identity['sha256']).replace('@DEPENDENCIES@',' '.join(shlex.quote(p) for p in sorted(packages)))
    (output/'PKGBUILD').write_text(recipe)
    (output/'input-identity.json').write_text(json.dumps({'source_sha256':source_hash,'runtime_sha256':runtime_hash,'source_manifest_sha256':hashlib.sha256(source_manifest.read_bytes()).hexdigest(),'source_count':len(manifest),'application_sha256':app_hash,'application_required_glibc':required,'source_sidecar':identity,'package_generated':False},indent=2)+'\n')

if __name__=='__main__':
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('--check-only',action='store_true');p.add_argument('--stage',type=Path,required=True);p.add_argument('--source-manifest',type=Path,required=True);p.add_argument('--application',type=Path,required=True);p.add_argument('--app-receipt',type=Path,required=True);p.add_argument('--source-sidecar',type=Path,required=True);p.add_argument('--output',type=Path,required=True);a=p.parse_args();prepare(a.stage.resolve(),a.source_manifest.resolve(),a.source_sidecar.resolve(),a.application.resolve(),a.app_receipt.resolve(),a.output.resolve(),a.check_only)
