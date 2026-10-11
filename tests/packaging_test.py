import importlib.util
import io
import json
import os
from pathlib import Path
import shlex
import subprocess
import tarfile
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('rpm_deps', 'scripts/rpm-elf-deps.py')
rpm_deps = importlib.util.module_from_spec(spec)
spec.loader.exec_module(rpm_deps)


class PackagingTests(unittest.TestCase):
    def test_versions_and_runtime_contract(self):
        subprocess.run(['node', 'scripts/verify-packaging.mjs'], check=True)
        cfg = json.loads(Path('src-tauri/tauri.conf.json').read_text())
        for fmt in ['deb', 'rpm']:
            self.assertIn('xdg-desktop-portal', cfg['bundle']['linux'][fmt]['depends'])
            self.assertIn('xdg-desktop-portal-gtk', cfg['bundle']['linux'][fmt]['recommends'])

    def test_release_tag_mismatch_is_refused(self):
        result = subprocess.run(['node', 'scripts/verify-packaging.mjs'], env={**os.environ, 'RELEASE_TAG': 'v9.9.9'}, capture_output=True)
        self.assertNotEqual(result.returncode, 0)

    def test_rpm_uses_sonames_and_versioned_symbols(self):
        dynamic = ' (NEEDED) Shared library: [libssl.so.3]\n (NEEDED) Shared library: [libc.so.6]'
        versions = ' File: libc.so.6 Cnt: 2\n Name: GLIBC_2.35 Flags: none\n File: libssl.so.3 Cnt: 1\n Name: OPENSSL_3.0.0 Flags: none'
        self.assertEqual(rpm_deps.requirements(dynamic, versions), ['libc.so.6()(64bit)', 'libc.so.6(GLIBC_2.35)(64bit)', 'libssl.so.3()(64bit)', 'libssl.so.3(OPENSSL_3.0.0)(64bit)'])
        with self.assertRaises(ValueError):
            rpm_deps.requirements('', '')

    def test_scripts_parse_and_desktop_is_valid(self):
        for script in ['scripts/inspect-elf.sh', 'scripts/inspect-packages.sh', 'scripts/prepare-arch-ci.sh', 'sonante-arch/PKGBUILD']:
            subprocess.run(['bash', '-n', script], check=True)
        subprocess.run(['desktop-file-validate', 'sonante-arch/sonante.desktop'], check=True)

    def test_arch_refuses_mislabeled_committed_source(self):
        with tempfile.TemporaryDirectory(prefix='sn-pkg-') as directory:
            root = Path(directory)
            archive = root / 'old-source.tar'
            payload = b'{"version":"0.0.0"}'
            with tarfile.open(archive, 'w') as stream:
                entry = tarfile.TarInfo('sonante/package.json')
                entry.size = len(payload)
                stream.addfile(entry, io.BytesIO(payload))
            fake_git = root / 'git'
            fake_git.write_text(f'#!/bin/sh\nexec cat {shlex.quote(str(archive))}\n')
            fake_git.chmod(0o700)
            result = subprocess.run(['bash', 'scripts/prepare-arch-ci.sh', str(root / 'stage')],
                                    env={**os.environ, 'PATH': f'{root}:{os.environ["PATH"]}'}, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('refusing mislabeled Arch package', result.stderr)
            self.assertFalse((root / 'stage' / 'PKGBUILD').exists())

    def test_glibc_ceiling_rejects_newer_or_unclassified_symbols(self):
        with tempfile.TemporaryDirectory(prefix='sn-abi-') as directory:
            root = Path(directory)
            binary = root / 'unexecuted-binary'
            binary.touch()
            fake_readelf = root / 'readelf'
            for symbol, success in [('GLIBC_2.35', True), ('GLIBC_2.36', False), ('GLIBC_ABI_DT_RELR', False)]:
                fake_readelf.write_text(f'#!/bin/sh\nprintf "%s\\n" {symbol}\n')
                fake_readelf.chmod(0o700)
                result = subprocess.run(['bash', 'scripts/inspect-elf.sh', str(binary), '2.35'],
                                        env={**os.environ, 'PATH': f'{root}:{os.environ["PATH"]}'}, capture_output=True)
                self.assertEqual(result.returncode == 0, success, symbol)

    def test_release_manual_dispatch_has_no_upload(self):
        release = Path('.github/workflows/release.yml').read_text()
        self.assertIn("if: github.event_name == 'release'", release)
        self.assertIn('needs: packages', release)
        self.assertIn('GH_REPO:', release)
        self.assertNotIn('gh release create', release)
        self.assertNotIn('--clobber', release)
        ci = Path('.github/workflows/ci.yml').read_text()
        for check in ['npm run test:frontend', 'cargo test', 'cargo check', '--locked', 'inspect-elf.sh', 'inspect-packages.sh', 'makepkg', 'sha256sum -c']:
            self.assertIn(check, ci)


if __name__ == '__main__':
    unittest.main()
