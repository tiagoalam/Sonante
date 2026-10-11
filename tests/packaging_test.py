import hashlib
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

    def test_webp_loader_is_optional_in_all_packages(self):
        loader = 'webp-pixbuf-loader'
        cfg = json.loads(Path('src-tauri/tauri.conf.json').read_text())
        for fmt in ['deb', 'rpm']:
            with self.subTest(format=fmt):
                package = cfg['bundle']['linux'][fmt]
                self.assertEqual(package['recommends'].count(loader), 1)
                self.assertNotIn(loader, package['depends'])

        metadata = subprocess.check_output([
            'bash', '-c',
            'source sonante-arch/PKGBUILD\n'
            'printf "required:%s\\n" "${depends[@]}" "${makedepends[@]}"\n'
            'printf "optional:%s\\n" "${optdepends[@]}"',
        ], text=True)
        entries = [line.partition(':') for line in metadata.splitlines()]
        required = [value for kind, _, value in entries if kind == 'required']
        optional = [value.split(':', 1)[0] for kind, _, value in entries if kind == 'optional']
        self.assertTrue(required)
        self.assertNotIn(loader, required)
        self.assertEqual(optional.count(loader), 1)

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

    def test_arch_archive_trust_is_scoped_to_exact_checkout(self):
        with tempfile.TemporaryDirectory(prefix='sn-arch-owner-') as directory:
            root = Path(directory)
            config = root / 'gitconfig'
            config.write_text('')
            env = {**os.environ, 'GIT_TEST_ASSUME_DIFFERENT_OWNER': '1',
                   'GIT_CONFIG_GLOBAL': str(config), 'GIT_CONFIG_SYSTEM': os.devnull,
                   'GIT_CONFIG_COUNT': '0'}
            env.pop('GIT_CONFIG_PARAMETERS', None)
            archive_command = ['archive', '--format=tar', '--prefix=sonante/', 'HEAD']
            refused = subprocess.run(['git', *archive_command], env=env, capture_output=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn(b'dubious ownership', refused.stderr)

            stage = root / 'stage'
            subprocess.run(['bash', 'scripts/prepare-arch-ci.sh', str(stage)],
                           env=env, check=True, capture_output=True)
            expected = subprocess.check_output(
                ['git', '-c', f'safe.directory={Path.cwd().resolve()}', *archive_command], env=env)
            source = stage / 'sonante-source.tar'
            self.assertEqual(source.read_bytes(), expected)
            self.assertEqual(stage.stat().st_mode & 0o777, 0o700)
            desktop = stage / 'sonante.desktop'
            self.assertEqual(desktop.read_bytes(), Path('sonante-arch/sonante.desktop').read_bytes())
            recipe = (stage / 'PKGBUILD').read_text()
            self.assertTrue(recipe.startswith(Path('sonante-arch/PKGBUILD').read_text()))
            self.assertIn("source=('sonante-source.tar' 'sonante.desktop')", recipe)
            hashes = [hashlib.sha256(path.read_bytes()).hexdigest() for path in [source, desktop]]
            self.assertTrue(recipe.endswith(f'sha256sums=("{hashes[0]}" "{hashes[1]}")\n'))

            still_refused = subprocess.run(['git', *archive_command], env=env, capture_output=True)
            self.assertNotEqual(still_refused.returncode, 0)
            self.assertIn(b'dubious ownership', still_refused.stderr)
            self.assertEqual(config.read_text(), '')

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
