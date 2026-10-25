#!/usr/bin/env python3
"""Fail closed on blocked, incomplete, modified, symlinked or extra stage inputs."""
import hashlib
import json
from pathlib import Path
import re
import stat
import sys

def digest(path):
    checksum = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            checksum.update(block)
    return checksum.hexdigest()

def verify(root):
    if (root/'DISTRIBUTION_BLOCKED').exists():
        raise ValueError('Runtime distribution is blocked; audit fixture cannot be packaged')
    receipt=json.loads((root/'staging-receipt.json').read_text())
    paths=list(root.rglob('*'))
    if any(p.is_symlink() for p in paths):raise ValueError('Symlink in runtime stage')
    actual={str(p.relative_to(root)) for p in paths if not p.is_dir()}
    if actual != set(receipt)|{'staging-receipt.json'}:
        raise ValueError('Runtime stage inventory differs from receipt')
    allowed_dirs={p for name in receipt for p in (root/name).parents if p.is_relative_to(root)}
    if any(p not in allowed_dirs or p.stat().st_mode & 0o6022 for p in paths if p.is_dir()):
        raise ValueError('Unexpected or writable staging directory')
    for name,expected in receipt.items():
        relative=Path(name)
        if relative.is_absolute() or '..' in relative.parts:
            raise ValueError('Unsafe staging path')
        path=root/relative
        if any(p.is_symlink() for p in [path,*path.parents] if p.is_relative_to(root)):
            raise ValueError('Symlinked staging path')
        mode=path.stat().st_mode
        if not stat.S_ISREG(mode) or mode & 0o6022 or path.stat().st_nlink != 1:
            raise ValueError('Unsafe staging file')
        if digest(path)!=expected:
            raise ValueError('Staging file changed: '+name)
    manifest=json.loads((Path(__file__).resolve().parent.parent/'packaging/private-runtimes.json').read_text())
    runtime=root/manifest['install_root'].lstrip('/')
    if (runtime/'manifest.json').read_bytes()!=(Path(__file__).resolve().parent.parent/'packaging/private-runtimes.json').read_bytes():
        raise ValueError('Installed manifest differs from the compiled manifest')
    expected_files=dict(manifest['binaries'])
    for resources in manifest['resources'].values():expected_files.update(resources)
    runtime_files={str(p.relative_to(runtime)) for p in runtime.rglob('*') if p.is_file()}
    if runtime_files != set(expected_files)|{'manifest.json'}:
        raise ValueError('Unexpected or missing runtime resource')
    for name,expected in expected_files.items():
        path=runtime/name
        if hashlib.sha256(path.read_bytes()).hexdigest()!=expected or (name in manifest['binaries'] and re.search(rb'/(?:home|build|root)/',path.read_bytes())):
            raise ValueError('Runtime binary identity/privacy gate failed')
    inventory=json.loads((root/'usr/share/doc/sonante/runtime-inventory.json').read_text())
    if not inventory['distribution_ready'] or inventory['source_trees_installed'] or inventory['abi']['glibc_ceiling']!='2.35' or inventory['abi']['webkit_minimum']!='2.40':
        raise ValueError('Runtime build preparation incomplete or ABI contract changed')
    if (root/'usr/share/doc/sonante/runtime-sources').exists():raise ValueError('Source sidecar must not be installed')
    if any(not (runtime/name).stat().st_mode & 0o111 for name in manifest['binaries']):raise ValueError('Runtime not executable')
    return receipt

if __name__=='__main__':
    verify(Path(sys.argv[1]).resolve())
    print('Private runtime stage verified.')
