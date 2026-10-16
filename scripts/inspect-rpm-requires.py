#!/usr/bin/env python3
"""Validate RPM requirement fields, not the ambiguous --requires display."""
import sys

WEBKIT = '(webkit2gtk4.1 >= 2.40)'


def validate(rows):
    webkit = []
    for row in rows.splitlines():
        name, flags, version = row.split('\t')
        flags = int(flags)
        if 'webkit2gtk4.1' in name:
            webkit.append((name, flags, version))
    # Tauri 2.12.0 / rpm-rs 0.16.0 Dependency::any preserves this rich
    # expression verbatim. RPM identifies rich deps by the leading '(';
    # flags=0 / empty EVR are correct for this representation, not for the
    # unparenthesized literal that the previous inspection accepted.
    if webkit != [(WEBKIT, 0, '')]:
        raise ValueError(f'RPM must encode the WebKit >= 2.40 rich dependency: {webkit!r}')


if __name__ == '__main__':
    validate(sys.stdin.read())
    print('RPM WebKit requirement fields verified (rich dependency, >= 2.40)')
