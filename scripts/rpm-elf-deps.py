#!/usr/bin/env python3
"""Add the built ELF's actual SONAME/version requirements to Tauri's RPM config."""
import json
import re
import subprocess
import sys


def requirements(dynamic, versions):
    names = re.findall(r'\(NEEDED\).*?\[([^\]]+)\]', dynamic)
    if not names:
        raise ValueError('No dynamic libraries found')
    result = {f'{name}()(64bit)' for name in names}
    library = None
    for line in versions.splitlines():
        match = re.search(r'File: (\S+)', line)
        if match:
            library = match[1]
        match = re.search(r'Name: (\S+)', line)
        if match and library:
            result.add(f'{library}({match[1]})(64bit)')
    return sorted(result)


if __name__ == '__main__':
    binary, output = sys.argv[1:]
    header = subprocess.check_output(['readelf', '-h', binary], text=True)
    if 'ELF64' not in header or 'X86-64' not in header:
        raise SystemExit('RPM requirement generation supports only ELF64 x86_64')
    dynamic = subprocess.check_output(['readelf', '-d', binary], text=True)
    versions = subprocess.check_output(['readelf', '--version-info', binary], text=True)
    with open('src-tauri/tauri.conf.json') as stream:
        base = json.load(stream)['bundle']['linux']['rpm']['depends']
    with open(output, 'x') as stream:
        json.dump({'bundle': {'linux': {'rpm': {'depends': sorted(set(base + requirements(dynamic, versions)))}}}}, stream, indent=2)
