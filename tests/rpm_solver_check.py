#!/usr/bin/env python3
"""Real Tauri emission + RPM dependency checks in a disposable RPM database.

Never installs packages or executes package scriptlets. Other dependencies are
synthetic providers: this isolates WebKit resolution, not Fedora runtime support.
Requires rpm/rpmbuild and the existing npm-installed Tauri CLI; no app compilation.
"""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile

REPO = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('inspect_rpm', REPO / 'scripts/inspect-rpm-requires.py')
inspect_rpm = importlib.util.module_from_spec(spec)
spec.loader.exec_module(inspect_rpm)


def run(*args, **kwargs):
    return subprocess.run(args, check=True, text=True, capture_output=True, **kwargs).stdout


def requirements(package):
    return run('rpm', '-qp', '--queryformat',
               '[%{REQUIRENAME}\t%{REQUIREFLAGS}\t%{REQUIREVERSION}\n]', str(package))


def bundle_fixture(root, requirement):
    root.mkdir()
    (root / 'src').mkdir()
    (root / 'src/main.rs').write_text('fn main() {}\n')
    (root / 'Cargo.toml').write_text(
        '[package]\nname="rpm-probe"\nversion="0.0.0"\nedition="2021"\n')
    binary = root / 'target/release/rpm-probe'
    binary.parent.mkdir(parents=True)
    binary.write_bytes(b'Packaging fixture; never execute.\n')
    binary.chmod(0o755)
    (root / 'tauri.conf.json').write_text(json.dumps({
        'productName': 'rpm-probe', 'version': '0.0.0', 'identifier': 'org.example.rpm-probe',
        'bundle': {'active': True, 'targets': ['rpm'],
                   'linux': {'rpm': {'depends': [requirement]}}},
    }))
    run(str(REPO / 'node_modules/.bin/tauri'), 'bundle', '--bundles', 'rpm', cwd=root)
    return next((root / 'target/release/bundle/rpm').glob('*.rpm'))


def check_resolution(root, package, rows, webkit_version, expected):
    # Supply every unrelated requirement without changing the consumer header.
    # No synthetic provider may hide a WebKit requirement, including the broken
    # literal. SONAME and symbol capabilities remain separate from version 2.40.
    provides = set()
    for row in rows.splitlines():
        name, flags, version = row.split('\t')
        if 'webkit2gtk4.1' in name or name.startswith('rpmlib('):
            continue
        if name.startswith('(') or '\n' in name:
            raise ValueError(f'Unsupported unrelated rich requirement: {name}')
        provides.add(name + (f' = {version}' if version else ''))
    if webkit_version is not None:
        provides.add(f'webkit2gtk4.1 = {webkit_version}')
    top = root / 'build'
    top.mkdir(parents=True)
    specfile = root / 'provider.spec'
    specfile.write_text(
        'Name: sonante-test-provider\nVersion: 1\nRelease: 1\n'
        'Summary: Synthetic dependency provider; never install\nLicense: MIT\n'
        'AutoReqProv: no\n' + ''.join(f'Provides: {dep}\n' for dep in sorted(provides)) +
        '%description\nSynthetic provider for dependency checks only.\n%files\n')
    run('rpmbuild', '--define', f'_topdir {top}', '-bb', str(specfile))
    provider = next((top / 'RPMS').rglob('*.rpm'))
    db = root / 'rpmdb'
    run('rpm', '--dbpath', str(db), '--initdb')
    checked = subprocess.run([
        'rpm', '--dbpath', str(db), '-U', '--test', '--noscripts', '--notriggers',
        str(package), str(provider),
    ], text=True, capture_output=True)
    if (checked.returncode == 0) != expected:
        raise AssertionError(f'{package.name}, WebKit {webkit_version}: '
                             f'{checked.stdout}\n{checked.stderr}')
    if not expected and 'webkit2gtk4.1' not in checked.stderr:
        raise AssertionError(f'Rejection must identify WebKit: {checked.stderr}')
    print(f'{package.name}: WebKit {webkit_version}: '
          f'{"accepted" if expected else "rejected"} (RPM --test only)')


def main():
    with tempfile.TemporaryDirectory(prefix='sonante-rpm-check-') as directory:
        root = Path(directory)
        good = bundle_fixture(root / 'rich', inspect_rpm.WEBKIT)
        literal = bundle_fixture(root / 'literal', 'webkit2gtk4.1 >= 2.40')
        inspect_rpm.validate(requirements(good))
        try:
            inspect_rpm.validate(requirements(literal))
        except ValueError:
            pass
        else:
            raise AssertionError('The actual Tauri literal RPM must be rejected')
        cases = [(good, True), (literal, False)]
        # Also test the actual release candidate emitted by the normal CI bundle.
        for candidate in sys.argv[1:]:
            package = Path(candidate).resolve()
            inspect_rpm.validate(requirements(package))
            cases.append((package, True))
        for index, (package, valid) in enumerate(cases):
            rows = requirements(package)
            for version in ['2.38.2', '2.40', '2.52.1', None]:
                check_resolution(root / f'case-{index}-{version}', package, rows,
                                 version, valid and version in ['2.40', '2.52.1'])


if __name__ == '__main__':
    main()
