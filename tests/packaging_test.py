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

validator_spec = importlib.util.spec_from_file_location('rpm_inspect', 'scripts/inspect-rpm-requires.py')
rpm_inspect = importlib.util.module_from_spec(validator_spec)
validator_spec.loader.exec_module(rpm_inspect)


class PackagingTests(unittest.TestCase):
    def test_tray_indicator_is_optional_and_existing_runtime_is_preserved(self):
        cfg = json.loads(Path('src-tauri/tauri.conf.json').read_text())
        for fmt, indicator in [('deb', 'libayatana-appindicator3-1'), ('rpm', 'libappindicator-gtk3')]:
            with self.subTest(format=fmt):
                package = cfg['bundle']['linux'][fmt]
                self.assertEqual(package['recommends'].count(indicator), 1)
                self.assertNotIn(indicator, package['depends'])
                self.assertIn('mpd', package['depends'])
        metadata = subprocess.check_output([
            'bash', '-c', 'source sonante-arch/PKGBUILD\n'
            'printf "required:%s\\n" "${depends[@]}" "${makedepends[@]}"\n'
            'printf "optional:%s\\n" "${optdepends[@]}"',
        ], text=True)
        entries = [line.partition(':') for line in metadata.splitlines()]
        required = [value for kind, _, value in entries if kind == 'required']
        optional = [value.split(':', 1)[0] for kind, _, value in entries if kind == 'optional']
        self.assertNotIn('libappindicator', required)
        self.assertEqual(optional.count('libappindicator'), 1)

    def test_versions_and_runtime_contract(self):
        subprocess.run(['node', 'scripts/verify-packaging.mjs'], check=True)
        cfg = json.loads(Path('src-tauri/tauri.conf.json').read_text())
        for fmt in ['deb', 'rpm']:
            self.assertIn('xdg-desktop-portal', cfg['bundle']['linux'][fmt]['depends'])
            self.assertIn('xdg-desktop-portal-gtk', cfg['bundle']['linux'][fmt]['recommends'])
        self.assertIn('libwebkit2gtk-4.1-0 (>= 2.40)', cfg['bundle']['linux']['deb']['depends'])
        self.assertIn('(webkit2gtk4.1 >= 2.40)', cfg['bundle']['linux']['rpm']['depends'])

    def test_rpm_webkit_header_structure(self):
        rpm_inspect.validate('(webkit2gtk4.1 >= 2.40)\t0\t\nmpd\t0\t\n')
        for row in [
            'webkit2gtk4.1 >= 2.40\t0\t',  # actual broken Tauri artifact
            'webkit2gtk4.1\t0\t',
            '(webkit2gtk4.1 >= 2.38)\t0\t',
            '(webkit2gtk4.1 >= 2.40)\t12\t2.40',
            '(webkit2gtk4.1 >= 2.40)\t0\t2.40',
            '(webkit2gtk4.1 >= 2.40 or other)\t0\t',
            '',
        ]:
            with self.subTest(row=row), self.assertRaises(ValueError):
                rpm_inspect.validate(row)

    def test_gpl_license_and_third_party_notices(self):
        license_bytes = Path('LICENSE').read_bytes()
        # Verbatim GNU GPLv3 text from https://www.gnu.org/licenses/gpl-3.0.txt.
        self.assertEqual(hashlib.sha256(license_bytes).hexdigest(),
                         '3972dc9744f6499f0f9b2dbf76696f2ae7ad8af9b23dde66d6af86c9dfb36986')
        authors = Path('AUTHORS').read_text()
        for value in ['Tiago Alam', 'original author', 'Copyright 2026 Tiago Alam',
                      'https://github.com/tiagoalam/Sonante']:
            self.assertIn(value, authors)
        notices = Path('THIRD_PARTY_NOTICES').read_text()
        for value in ['ISC License', 'Lucide Icons and Contributors', 'Cole Bemis',
                      'MPL-2.0', 'MIT License']:
            self.assertIn(value, notices)
        self.assertIn('GPL-3.0-only', Path('packaging/debian-copyright').read_text())

    def test_native_packages_include_license_payload(self):
        cfg = json.loads(Path('src-tauri/tauri.conf.json').read_text())
        self.assertEqual(cfg['bundle']['license'], 'GPL-3.0-only')
        self.assertEqual(cfg['bundle']['licenseFile'], '../LICENSE')
        for fmt, folder in [('deb', 'doc'), ('rpm', 'licenses')]:
            files = cfg['bundle']['linux'][fmt]['files']
            self.assertEqual(files[f'/usr/share/{folder}/sonante/LICENSE'], '../LICENSE')
            for name in ['AUTHORS', 'THIRD_PARTY_NOTICES']:
                self.assertEqual(files[f'/usr/share/doc/sonante/{name}'], f'../{name}')
            for source in files.values():
                self.assertTrue((Path('src-tauri') / source).is_file(), source)
        self.assertEqual(cfg['bundle']['linux']['deb']['files']['/usr/share/doc/sonante/copyright'],
                         '../packaging/debian-copyright')

    def test_arch_package_installs_license_and_notices(self):
        # Execute only package(), using a synthetic built tree; never run build().
        recipe = Path('sonante-arch/PKGBUILD').resolve()
        with tempfile.TemporaryDirectory(prefix='sn-license-') as directory:
            root = Path(directory)
            source = root / 'src' / 'sonante'
            for name in ['src-tauri/target/release/sonante', 'src-tauri/icons/32x32.png',
                         'src-tauri/icons/128x128.png', 'src-tauri/icons/128x128@2x.png',
                         'src-tauri/icons/icon.png', 'README.md']:
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(b'fixture')
            for name in ['LICENSE', 'AUTHORS', 'THIRD_PARTY_NOTICES']:
                (source / name).write_bytes(Path(name).read_bytes())
            (root / 'src' / 'sonante.desktop').write_bytes(Path('sonante-arch/sonante.desktop').read_bytes())
            result = subprocess.check_output([
                'bash', '-c', 'source "$1"\nprintf "%s\\n" "${license[@]}"\npackage',
                'license-test', str(recipe),
            ], env={**os.environ, 'srcdir': str(root / 'src'), 'pkgdir': str(root / 'pkg')}, text=True)
            self.assertEqual(result.strip(), 'GPL-3.0-only')
            for name, folder in [('LICENSE', 'licenses'), ('AUTHORS', 'doc'), ('THIRD_PARTY_NOTICES', 'doc')]:
                installed = root / 'pkg' / 'usr' / 'share' / folder / 'sonante' / name
                self.assertEqual(installed.read_bytes(), Path(name).read_bytes())
                self.assertEqual(installed.stat().st_mode & 0o777, 0o644)

    def test_package_inspection_checks_verbatim_deb_license(self):
        inspect = Path('scripts/inspect-packages.sh').resolve()
        with tempfile.TemporaryDirectory(prefix='sn-license-inspect-') as directory:
            root = Path(directory)
            for fmt, extension in [('deb', 'deb'), ('rpm', 'rpm')]:
                path = root / 'src-tauri' / 'target' / 'release' / 'bundle' / fmt / f'fixture.{extension}'
                path.parent.mkdir(parents=True)
                path.touch()
            (root / 'LICENSE').write_bytes(Path('LICENSE').read_bytes())
            (root / 'packaging').mkdir()
            copyright_text = Path('packaging/debian-copyright').read_bytes()
            (root / 'packaging' / 'debian-copyright').write_bytes(copyright_text)
            archive = root / 'data.tar'
            def write_archive(payload):
                with tarfile.open(archive, 'w') as stream:
                    for name, content in [('usr/share/doc/sonante/LICENSE', payload),
                                          ('usr/share/doc/sonante/copyright', copyright_text)]:
                        entry = tarfile.TarInfo(name)  # Tauri uses paths without a ./ prefix.
                        entry.size = len(content)
                        stream.addfile(entry, io.BytesIO(content))
                    for name in ['usr/bin/sonante', 'usr/share/applications/sonante.desktop',
                                 'usr/share/icons/hicolor/icon.png', 'usr/share/doc/sonante/AUTHORS',
                                 'usr/share/doc/sonante/THIRD_PARTY_NOTICES']:
                        stream.addfile(tarfile.TarInfo(name), io.BytesIO())
            fake = root / 'tools'
            fake.mkdir()
            contents = 'usr/bin/sonante\nusr/share/applications/sonante.desktop\nusr/share/icons/hicolor/icon.png\nusr/share/licenses/sonante/LICENSE\nusr/share/doc/sonante/AUTHORS\nusr/share/doc/sonante/THIRD_PARTY_NOTICES'
            deb_dependencies = 'mpd, alsa-utils, xdg-utils, xdg-desktop-portal, libasound2, libssl3'
            rpm_dependencies = 'mpd\nalsa-utils\nxdg-utils\nxdg-desktop-portal\nalsa-lib\nopenssl-libs\nlibssl.so.3()(64bit)'
            tool = f'''#!/usr/bin/env python3
import os, pathlib, sys
args = sys.argv[1:]
mode = os.environ.get('WEBKIT_FIXTURE', 'valid')
deb_webkit = {{'valid': 'libwebkit2gtk-4.1-0 (>= 2.40)', 'bare': 'libwebkit2gtk-4.1-0', 'old': 'libwebkit2gtk-4.1-0 (>= 2.38)', 'missing': ''}}
rpm_webkit = {{'valid': '(webkit2gtk4.1 >= 2.40)', 'literal': 'webkit2gtk4.1 >= 2.40', 'bare': 'webkit2gtk4.1', 'old': 'webkit2gtk4.1 >= 2.38', 'missing': ''}}
if args[0] == '--fsys-tarfile': sys.stdout.buffer.write(pathlib.Path({str(archive)!r}).read_bytes())
elif args[0] in ('-I', '-c', '-K', '-qip'): pass
elif args[0] == '-qlp': print({contents!r})
elif 'Depends' in args: print({deb_dependencies!r} + ', ' + deb_webkit[mode.removeprefix('deb-') if mode.startswith('deb-') else 'valid'])
elif any('REQUIRENAME' in arg for arg in args):
    key = mode.removeprefix('rpm-') if mode.startswith('rpm-') else 'valid'
    if key != 'missing': print(rpm_webkit[key] + '\\t0\\t')
elif '--requires' in args: print({rpm_dependencies!r} + '\\n' + rpm_webkit[mode.removeprefix('rpm-') if mode.startswith('rpm-') else 'valid'])
elif 'Version' in args or '%{{VERSION}}' in args: print('0.4.9')
elif 'Architecture' in args: print('amd64')
elif '%{{ARCH}}' in args: print('x86_64')
elif '%{{LICENSE}}' in args: print('GPL-3.0-only')
else: raise SystemExit(2)
'''
            for name in ['dpkg-deb', 'rpm']:
                path = fake / name
                path.write_text(tool)
                path.chmod(0o700)
            env = {**os.environ, 'PATH': f'{fake}:{os.environ["PATH"]}'}
            write_archive(Path('LICENSE').read_bytes())
            accepted = subprocess.run(['bash', str(inspect), '0.4.9'], cwd=root,
                                      env=env, capture_output=True)
            self.assertEqual(accepted.returncode, 0, accepted.stderr.decode())
            for fmt in ['deb', 'rpm']:
                for mode in (['bare', 'old', 'missing', 'literal'] if fmt == 'rpm' else ['bare', 'old', 'missing']):
                    with self.subTest(format=fmt, requirement=mode):
                        rejected = subprocess.run(['bash', str(inspect), '0.4.9'], cwd=root,
                                                  env={**env, 'WEBKIT_FIXTURE': f'{fmt}-{mode}'},
                                                  capture_output=True)
                        self.assertNotEqual(rejected.returncode, 0)
            write_archive(b'incomplete license')
            rejected = subprocess.run(['bash', str(inspect), '0.4.9'], cwd=root,
                                      env=env, capture_output=True)
            self.assertNotEqual(rejected.returncode, 0)

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
            fake_git.write_text(f'#!/bin/sh\ncase \" $* \" in *\" status \"*) exit 0;; esac\nexec cat {shlex.quote(str(archive))}\n')
            fake_git.chmod(0o700)
            result = subprocess.run(['bash', 'scripts/prepare-arch-ci.sh', str(root / 'stage')],
                                    env={**os.environ, 'PATH': f'{root}:{os.environ["PATH"]}'}, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('refusing mislabeled Arch package', result.stderr)
            self.assertFalse((root / 'stage' / 'PKGBUILD').exists())

    def arch_checkout(self, root):
        checkout = root / 'checkout'
        checkout.mkdir()
        for name, content in {
            'package.json': '{"version":"0.5.4"}',
            'src/source.rs': 'original source\n',
            '.gitignore': '/dist/\n/target/\n',
            'sonante-arch/PKGBUILD': Path('sonante-arch/PKGBUILD').read_text(),
            'sonante-arch/sonante.desktop': Path('sonante-arch/sonante.desktop').read_text(),
        }.items():
            path = checkout / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(content)
        env = {**os.environ, 'GIT_CONFIG_SYSTEM': os.devnull,
               'GIT_CONFIG_GLOBAL': os.devnull, 'GIT_CONFIG_COUNT': '0',
               'GIT_AUTHOR_NAME': 'Fixture', 'GIT_AUTHOR_EMAIL': 'fixture@example.invalid',
               'GIT_COMMITTER_NAME': 'Fixture', 'GIT_COMMITTER_EMAIL': 'fixture@example.invalid'}
        env.pop('GIT_CONFIG_PARAMETERS', None)
        for args in [['init', '-q'], ['add', '.'],
                     ['-c', 'core.hooksPath=/dev/null', '-c', 'commit.gpgSign=false',
                      'commit', '-qm', 'Synthetic packaging fixture']]:
            subprocess.run(['git', *args], cwd=checkout, env=env, check=True, capture_output=True)
        return checkout, env

    def test_arch_refuses_tracked_changes_without_version_bump(self):
        script = Path('scripts/prepare-arch-ci.sh').resolve()
        for change in ['unstaged', 'staged', 'deleted']:
            with self.subTest(change=change), tempfile.TemporaryDirectory(prefix='sn-arch-dirty-') as directory:
                root = Path(directory)
                checkout, env = self.arch_checkout(root)
                source = checkout / 'src/source.rs'
                if change == 'deleted':
                    source.unlink()
                else:
                    source.write_text('updated source\n')
                if change == 'staged':
                    subprocess.run(['git', 'add', 'src/source.rs'], cwd=checkout, env=env, check=True)
                stage = root / 'stage'
                result = subprocess.run(['bash', str(script), str(stage)], cwd=checkout,
                                        env=env, capture_output=True, text=True)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('refusing incomplete Arch package source', result.stderr)
                self.assertFalse(stage.exists())
                self.assertEqual(json.loads((checkout / 'package.json').read_text())['version'], '0.5.4')

    def test_arch_refuses_untracked_source_before_creating_inputs(self):
        script = Path('scripts/prepare-arch-ci.sh').resolve()
        with tempfile.TemporaryDirectory(prefix='sn-arch-new-') as directory:
            root = Path(directory)
            checkout, env = self.arch_checkout(root)
            new_source = checkout / 'src/new-module/source.rs'
            new_source.parent.mkdir()
            new_source.write_text('new accepted source\n')
            stage = root / 'stage'
            result = subprocess.run(['bash', str(script), str(stage)], cwd=checkout,
                                    env=env, capture_output=True, text=True)
            self.assertNotEqual(result.returncode, 0)
            self.assertIn('refusing incomplete Arch package source', result.stderr)
            self.assertFalse(stage.exists())
            self.assertEqual(new_source.read_text(), 'new accepted source\n')

    def test_arch_accepts_clean_source_with_ignored_build_output(self):
        script = Path('scripts/prepare-arch-ci.sh').resolve()
        with tempfile.TemporaryDirectory(prefix='sn-arch-clean-') as directory:
            root = Path(directory)
            checkout, env = self.arch_checkout(root)
            ignored = checkout / 'dist/index.html'
            ignored.parent.mkdir()
            ignored.write_text('generated assets are rebuilt by makepkg\n')
            stage = root / 'stage'
            subprocess.run(['bash', str(script), str(stage)], cwd=checkout, env=env,
                           check=True, capture_output=True)
            with tarfile.open(stage / 'sonante-source.tar') as archive:
                self.assertEqual(archive.extractfile('sonante/src/source.rs').read(), b'original source\n')
                self.assertNotIn('sonante/dist/index.html', archive.getnames())
            self.assertEqual(ignored.read_text(), 'generated assets are rebuilt by makepkg\n')

    def test_arch_git_status_failure_is_fatal_before_creating_inputs(self):
        script = Path('scripts/prepare-arch-ci.sh').resolve()
        with tempfile.TemporaryDirectory(prefix='sn-arch-status-') as directory:
            root = Path(directory)
            checkout, env = self.arch_checkout(root)
            fake = root / 'fake-bin'
            fake.mkdir()
            git = fake / 'git'
            git.write_text('#!/bin/sh\necho "Synthetic status failure" >&2\nexit 77\n')
            git.chmod(0o700)
            stage = root / 'stage'
            result = subprocess.run(['bash', str(script), str(stage)], cwd=checkout,
                                    env={**env, 'PATH': f'{fake}:{env["PATH"]}'}, capture_output=True)
            self.assertEqual(result.returncode, 77)
            self.assertFalse(stage.exists())

    def test_arch_archive_trust_is_scoped_to_exact_checkout(self):
        with tempfile.TemporaryDirectory(prefix='sn-arch-owner-') as directory:
            root = Path(directory)
            checkout, fixture_env = self.arch_checkout(root)
            script = Path('scripts/prepare-arch-ci.sh').resolve()
            config = root / 'gitconfig'
            config.write_text('')
            env = {**fixture_env, 'GIT_TEST_ASSUME_DIFFERENT_OWNER': '1',
                   'GIT_CONFIG_GLOBAL': str(config), 'GIT_CONFIG_SYSTEM': os.devnull,
                   'GIT_CONFIG_COUNT': '0'}
            env.pop('GIT_CONFIG_PARAMETERS', None)
            archive_command = ['archive', '--format=tar', '--prefix=sonante/', 'HEAD']
            refused = subprocess.run(['git', *archive_command], cwd=checkout, env=env, capture_output=True)
            self.assertNotEqual(refused.returncode, 0)
            self.assertIn(b'dubious ownership', refused.stderr)

            stage = root / 'stage'
            subprocess.run(['bash', str(script), str(stage)],
                           cwd=checkout, env=env, check=True, capture_output=True)
            expected = subprocess.check_output(
                ['git', '-c', f'safe.directory={checkout.resolve()}', *archive_command], cwd=checkout, env=env)
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

            still_refused = subprocess.run(['git', *archive_command], cwd=checkout, env=env, capture_output=True)
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
        self.assertIn('workflow_dispatch:', release)
        self.assertIn('uses: ./.github/workflows/ci.yml', release)
        self.assertIn('contents: read', release)
        for publishing_capability in ['release:', 'contents: write', 'gh release', 'GH_TOKEN:', 'GH_REPO:', '--clobber']:
            self.assertNotIn(publishing_capability, release)
        ci = Path('.github/workflows/ci.yml').read_text()
        for check in ['npm run test:frontend', 'cargo test', 'cargo check', '--locked', 'inspect-elf.sh', 'inspect-packages.sh', 'makepkg', 'sha256sum -c']:
            self.assertIn(check, ci)


if __name__ == '__main__':
    unittest.main()
