#!/usr/bin/env bash
set -euo pipefail
binary="$1"
ceiling="${2:-}"
profile="${3:-}"
if [[ -n "$profile" && "$profile" != openssl3 ]]; then
  echo "Unknown ELF inspection profile: $profile" >&2
  exit 2
fi
file "$binary"
readelf -d "$binary"
readelf --version-info "$binary"
python3 - "$binary" "$ceiling" "$profile" <<'PY'
import re, subprocess, sys
dynamic = subprocess.check_output(['readelf', '-d', sys.argv[1]], text=True)
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
if len(sys.argv) > 3 and sys.argv[3] == 'openssl3':
    needed = set(re.findall(r'\(NEEDED\).*?\[([^\]]+)\]', dynamic))
    openssl = {'libssl.so.3', 'libcrypto.so.3'}
    if not openssl <= needed or {'libssl.so.4', 'libcrypto.so.4'} & needed:
        raise SystemExit('Expected OpenSSL 3 SONAME pair; OpenSSL 4 and incomplete pairs are unsupported')
    openssl_versions = set(re.findall(r'\bOPENSSL_[A-Za-z0-9_.]+', versions))
    parsed_openssl = []
    for value in openssl_versions:
        match = re.fullmatch(r'OPENSSL_(\d+(?:\.\d+)+)', value)
        if not match:
            raise SystemExit('Unclassified OpenSSL symbol ABI: ' + value)
        parsed_openssl.append(tuple(map(int, match.group(1).split('.'))))
    if not parsed_openssl or max(parsed_openssl) > (3, 5, 9):
        raise SystemExit('OpenSSL symbols exceed the 3.5.9 compatibility ceiling')
    print('OpenSSL SONAME requirements:', ', '.join(sorted(openssl)))
    print('OpenSSL symbol versions:', ', '.join(sorted(openssl_versions)))
PY
