#!/usr/bin/env bash
set -euo pipefail
binary="$1"
ceiling="${2:-}"
file "$binary"
readelf -d "$binary"
readelf --version-info "$binary"
python3 - "$binary" "$ceiling" <<'PY'
import re, subprocess, sys
versions = subprocess.check_output(['readelf', '--version-info', sys.argv[1]], text=True)
names = set(re.findall(r'\bGLIBC_[A-Za-z0-9_.]+', versions))
unclassified = sorted(name for name in names if not re.fullmatch(r'GLIBC_\d+(?:\.\d+)+', name))
if unclassified:
    print('Unclassified GLIBC symbol requirements:', ', '.join(unclassified))
    if sys.argv[2]:
        raise SystemExit('Cannot confirm GLIBC baseline with unclassified requirements')
required = sorted({tuple(map(int, value.split('.'))) for value in re.findall(r'\bGLIBC_(\d+(?:\.\d+)+)', versions)})
if not required:
    raise SystemExit('No GLIBC requirements found; cannot confirm baseline')
maximum = required[-1]
print('Maximum required GLIBC symbol version:', '.'.join(map(str, maximum)))
if sys.argv[2] and maximum > tuple(map(int, sys.argv[2].split('.'))):
    raise SystemExit('GLIBC requirements exceed the declared build baseline')
PY
