#!/usr/bin/env python3
"""Read-only target ABI/SONAME preflight. No player, profile, audio or installation.
Usage: python3 check-private-runtime-host.py runtime-inventory.json
A pass is not a clean-install or hardware acceptance test.
"""
import ctypes
import json
from pathlib import Path
import platform
import re
import shutil
import subprocess
import sys

def version(v): return tuple(map(int,v.split('.')))

def check(inventory):
    failures=[];abi=inventory['abi']
    if platform.system()!='Linux' or platform.machine()!='x86_64':failures.append('Linux x86_64 required')
    libc=ctypes.CDLL('libc.so.6');libc.gnu_get_libc_version.restype=ctypes.c_char_p
    actual=libc.gnu_get_libc_version().decode()
    if version(actual)<version(abi['glibc_ceiling']):failures.append('GLIBC '+abi['glibc_ceiling']+' required')
    command=shutil.which('ldconfig')
    if not command:raise ValueError('ldconfig required for read-only SONAME inventory')
    listing=subprocess.check_output([command,'-p'],text=True)
    libs=dict(re.findall(r'^\s*(\S+) \([^\n]*x86-64[^\n]*=> (\S+)',listing,re.M))
    for name in inventory['system_libraries']:
        if name not in libs or not Path(libs[name]).is_file():failures.append('Missing x86_64 SONAME '+name)
    cpp=libs.get('libstdc++.so.6')
    if cpp:
        data=Path(cpp).read_bytes()
        for key,ns in [('glibcxx_ceiling','GLIBCXX'),('cxxabi_ceiling','CXXABI')]:
            if (ns+'_'+abi[key]).encode()+b'\0' not in data:failures.append('Missing '+ns+'_'+abi[key])
    webkit=None
    try:
        w=ctypes.CDLL('libwebkit2gtk-4.1.so.0')
        webkit='.'.join(str(getattr(w,'webkit_get_'+part+'_version')()) for part in ['major','minor','micro'])
        if version(webkit)<version(abi['webkit_minimum']):failures.append('WebKitGTK 4.1 API >= '+abi['webkit_minimum']+' required')
    except OSError:failures.append('WebKitGTK 4.1 API library unavailable')
    print(json.dumps({'architecture':platform.machine(),'glibc':actual,'required_glibc':abi['glibc_ceiling'],
        'webkit_4_1_version':webkit,'webkit_minimum':abi['webkit_minimum'],'required_system_sonames':len(inventory['system_libraries']),
        'passed':not failures,'failures':failures,'scope':'Read-only ABI/SONAME preflight; package installation and hardware remain separate gates.'},indent=2))
    return bool(failures)

if __name__=='__main__':sys.exit(check(json.loads(Path(sys.argv[1]).read_text())))
