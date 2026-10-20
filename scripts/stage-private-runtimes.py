#!/usr/bin/env python3
"""Stage exact admitted, path-remapped runtime inputs; never install or run them.
Corresponding sources stay in a separate checksum-pinned sidecar.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import stat

SOURCE = Path(__file__).resolve().parent.parent
MANIFEST = SOURCE / 'packaging/private-runtimes.json'

def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()

def relative(name):
    p = Path(name)
    if not p.parts or p.is_absolute() or any(v in ('.', '..') for v in p.parts):
        raise ValueError('Unsafe input path')
    return p

def checked(entry):
    p = Path(entry['source'])
    if not stat.S_ISREG(p.lstat().st_mode) or digest(p) != entry['sha256']:
        raise ValueError('Input identity mismatch: ' + p.name)
    return p

def prepare(record, sidecar, output):
    pins = json.loads(MANIFEST.read_text())
    build = json.loads(record.read_text())
    if output.exists():
        raise ValueError('Use a fresh staging directory')
    if pins['schema'] != 2 or not build['admitted'] or build['abi'] != {
        'glibc_ceiling': '2.35', 'glibcxx_ceiling': '3.4.30',
        'cxxabi_ceiling': '1.3.13', 'webkit_minimum': '2.40'
    }:
        raise ValueError('Unvalidated build or unsupported ABI contract')
    identity = build['source_sidecar']
    if sidecar.name != identity['filename'] or digest(sidecar) != identity['sha256']:
        raise ValueError('Corresponding source sidecar mismatch')
    expected = dict(pins['binaries'])
    for resources in pins['resources'].values():
        for name, sha in resources.items():
            if name in expected and expected[name] != sha:
                raise ValueError('Conflicting resource identity')
            expected[name] = sha
    if set(build['files']) != set(expected):
        raise ValueError('Build resources differ from compiled manifest')
    for name, entry in build['files'].items():
        relative(name)
        if entry['sha256'] != expected[name] or entry['mode'] != (0o755 if name in pins['binaries'] else 0o644):
            raise ValueError('Resource identity/mode mismatch')
        checked(entry)
    for name, entry in build['notices'].items():
        relative(name); checked(entry)
    output.mkdir(parents=True)
    runtime = output / pins['install_root'].lstrip('/')
    def copy(entry, target, mode=0o644):
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(checked(entry), target); target.chmod(mode)
        if digest(target) != entry['sha256']:
            raise ValueError('Copy verification failed')
    for name, entry in build['files'].items():
        copy(entry, runtime / name, entry['mode'])
    runtime.joinpath('manifest.json').write_bytes(MANIFEST.read_bytes())
    for name, entry in build['notices'].items():
        copy(entry, output / 'usr/share/licenses/sonante/runtimes' / name)
    docs = output / 'usr/share/doc/sonante'
    docs.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(SOURCE/'packaging/PRIVATE_RUNTIME_SOURCES.txt', docs/'PRIVATE_RUNTIME_SOURCES.txt')
    # Public installed facts contain no builder paths or personal state.
    public = {'schema': 2, 'distribution_ready': True, 'abi': build['abi'],
              'source_sidecar': identity, 'runtime_sha256': pins['binaries'],
              'system_libraries': sorted(build['system_libraries']),
              'arch_packages': sorted({v['arch_package'] for v in build['system_libraries'].values()}),
              'source_trees_installed': False}
    (docs/'runtime-inventory.json').write_text(json.dumps(public, indent=2)+'\n')
    receipt = {str(p.relative_to(output)): digest(p) for p in sorted(output.rglob('*')) if p.is_file()}
    (output/'staging-receipt.json').write_text(json.dumps(receipt, indent=2)+'\n')
    return public

if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--build-inputs', type=Path, required=True)
    p.add_argument('--source-sidecar', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    a = p.parse_args(); prepare(a.build_inputs.resolve(), a.source_sidecar.resolve(), a.output.resolve())
    print('Verified resources/notices staged; corresponding sources remain separate. Nothing installed or packaged.')
