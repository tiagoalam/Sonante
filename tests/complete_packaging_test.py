"""Complete-installer boundaries, using tiny archives and an unexecuted ELF fixture."""
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('complete_packages', 'scripts/complete-packages.py')
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)


def archive_bytes(entries):
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w') as archive:
        for name, data, options in entries:
            item = tarfile.TarInfo(name)
            item.mode, item.size = 0o644, len(data)
            for key, value in options.items():
                setattr(item, key, value)
            archive.addfile(item, io.BytesIO(data) if item.isfile() else None)
    output.seek(0)
    return output


class CompletePackagingTests(unittest.TestCase):
    def test_public_baseline_and_descriptors_match_the_published_profiles(self):
        inputs = json.loads(Path('packaging/complete-packages.json').read_text())
        self.assertEqual(set(inputs['packages']), {'ubuntu', 'fedora', 'arch'})
        self.assertEqual(inputs['baseline_release'], 'v0.5.5')
        for variant, entry in inputs['packages'].items():
            self.assertEqual(entry['url'], 'https://github.com/tiagoalam/Sonante/releases/download/v0.5.5/' + entry['filename'])
            self.assertEqual(hashlib.sha256(Path(entry['descriptor']).read_bytes()).hexdigest(), entry['descriptor_sha256'])
            self.assertEqual(entry['payload_files'], 174 if variant == 'fedora' else 171)
        self.assertEqual(len(inputs['sources']), 2)

    def test_payload_hashes_and_modes_are_measured_from_the_archive(self):
        files, meta = c.read_payload(archive_bytes([('usr/bin/sonante', b'application', {'mode': 0o755})]))
        self.assertEqual(files, {'usr/bin/sonante': {'sha256': hashlib.sha256(b'application').hexdigest(), 'bytes': 11, 'mode': 0o755}})
        self.assertEqual(meta, {})

    def test_paths_duplicates_ownership_links_and_installer_scripts_are_rejected(self):
        cases = [('/usr/bin/sonante', {}), ('../usr/bin/sonante', {}), ('usr/../config.json', {}),
                 ('usr\\bin\\sonante', {}), ('usr/bin/sonante\n', {}), ('etc/mpd.conf', {}),
                 ('.INSTALL', {}), ('usr/bin/sonante', {'uid': 1000}), ('usr/bin/sonante', {'mode': 0o666}),
                 ('usr/bin/sonante', {'mode': 0o4755}), ('usr/link', {'type': tarfile.SYMTYPE, 'linkname': '/etc/passwd'}),
                 ('usr/link', {'type': tarfile.LNKTYPE, 'linkname': 'usr/bin/sonante'}),
                 ('usr/fifo', {'type': tarfile.FIFOTYPE})]
        for name, options in cases:
            with self.subTest(name=name, options=options), self.assertRaises(ValueError):
                c.read_payload(archive_bytes([(name, b'x', options)]), arch=True)
        with self.assertRaisesRegex(ValueError, 'Duplicate'):
            c.read_payload(archive_bytes([('usr/a', b'a', {}), ('usr/a', b'a', {})]))

    def test_truncated_and_oversized_payloads_are_rejected(self):
        with self.assertRaises(tarfile.TarError):
            c.read_payload(io.BytesIO(b'truncated'))
        with patch.object(c, 'MAX_FILE', 2), self.assertRaisesRegex(ValueError, 'oversized'):
            c.read_payload(archive_bytes([('usr/a', b'123', {})]))
        with patch.object(c, 'MAX_PAYLOAD', 2), self.assertRaisesRegex(ValueError, 'bound'):
            c.read_payload(archive_bytes([('usr/a', b'12', {}), ('usr/b', b'3', {})]))

    def test_tampered_cached_downloads_and_unsafe_names_do_not_access_network(self):
        with tempfile.TemporaryDirectory(prefix='sn-complete-input-') as directory:
            root = Path(directory)
            (root / 'input').write_bytes(b'wrong')
            entry = {'filename': 'input', 'bytes': 5, 'sha256': '0' * 64, 'url': 'https://invalid.example/input'}
            with patch.object(c.urllib.request, 'urlopen') as request:
                with self.assertRaisesRegex(ValueError, 'SHA256'):
                    c.fetch_input(entry, root)
                with self.assertRaises(ValueError):
                    c.fetch_input({**entry, 'filename': '../input'}, root)
                with self.assertRaisesRegex(ValueError, 'origin'):
                    c.fetch_input({**entry, 'filename': 'other'}, root)
                request.assert_not_called()

    def test_candidate_checksums_reject_changed_data_and_path_escape(self):
        with tempfile.TemporaryDirectory(prefix='sn-complete-sha-') as directory:
            root = Path(directory)
            (root / 'sonante').write_bytes(b'candidate')
            sums = root / 'checksums'
            sums.write_text(c.digest(root / 'sonante') + '  sonante\n')
            self.assertEqual(c.verify_checksums(root, 'checksums'), {'sonante'})
            (root / 'sonante').write_bytes(b'changed')
            with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                c.verify_checksums(root, 'checksums')
            sums.write_text('0' * 64 + '  ../sonante\n')
            with self.assertRaises(ValueError):
                c.verify_checksums(root, 'checksums')

    def test_arch_declares_the_native_application_floor_and_keeps_runtime_dependencies(self):
        original = 'pkgname = sonante\narch = x86_64\npkgver = 0.5.5-10\ndepend = glibc>=2.35\ndepend = pipewire\ndepend = webkit2gtk-4.1>=2.40\noptdepend = wireplumber: Shared volume\n'
        result = c.arch_info(original, '0.5.6', '1', '2.43', 123, 10)
        self.assertIn('pkgver = 0.5.6-1\n', result)
        self.assertIn('depend = glibc>=2.43\n', result)
        self.assertNotIn('glibc>=2.35', result)
        self.assertIn('optdepend = wireplumber: Shared volume\n', result)
        for missing in ['depend = pipewire\n', 'depend = webkit2gtk-4.1>=2.40\n']:
            with self.subTest(missing=missing), self.assertRaises(ValueError):
                c.arch_info(original.replace(missing, ''), '0.5.6', '1', '2.43', 123, 10)

    def test_elf_baseline_rejects_unknown_abi_and_does_not_apply_ubuntu_floor_to_arch_app(self):
        for symbol, arch, accepted in [('GLIBC_2.35', False, True), ('GLIBC_2.43', False, False),
                                       ('GLIBC_2.43', True, True), ('GLIBC_ABI_DT_RELR', False, False),
                                       ('GLIBC_ABI_DT_RELR', True, True), ('GLIBC_PRIVATE', True, False),
                                       ('GLIBC_UNKNOWN', True, False), ('GLIBCXX_3.4.31', True, False)]:
            def output(*args):
                return {'-h': 'ELF64 X86-64', '-d': '(NEEDED) [libc.so.6]', '--version-info': symbol}[args[1]]
            with self.subTest(symbol=symbol, arch=arch), patch.object(c, 'run', side_effect=output):
                if accepted:
                    c.elf_info(Path('unexecuted'), arch)
                else:
                    with self.assertRaises(ValueError):
                        c.elf_info(Path('unexecuted'), arch)

    def test_rpm_header_must_match_encoded_payload_digests_and_modes(self):
        expected = {'usr/bin/sonante': {'sha256': 'a' * 64, 'bytes': 10, 'mode': 0o755}}
        row = '/usr/bin/sonante\t10\t33261\troot\troot\t' + 'a' * 64 + '\n'
        with patch.object(c, 'run', return_value=row):
            c.verify_rpm_header(Path('uninstalled.rpm'), expected)
        for changed in [row.replace('root', 'other', 1), row.replace('10\t', '11\t'), row.replace('a' * 64, 'b' * 64), row + row]:
            with self.subTest(row=changed), patch.object(c, 'run', return_value=changed), self.assertRaises(ValueError):
                c.verify_rpm_header(Path('uninstalled.rpm'), expected)

    def test_source_archives_keep_the_tracked_source_and_only_overlay_the_fedora_descriptor(self):
        identity = {'version': '0.5.6', 'commit': 'a' * 40, 'epoch': 1}
        entries = [('package.json', b'{"version":"0.5.6"}', {}), ('packaging/private-runtimes.json', b'standard', {})]
        with tempfile.TemporaryDirectory(prefix='sn-complete-source-') as directory:
            root = Path(directory)
            for overlay in [None, b'fedora']:
                with patch.object(c, 'archive_stream', return_value=archive_bytes(entries)):
                    name = c.source_archive(root, identity, '1', overlay)
                with tarfile.open(root / name) as archive:
                    values = {m.name.split('/', 1)[1]: archive.extractfile(m).read() for m in archive}
                self.assertEqual(values['package.json'], entries[0][1])
                self.assertEqual(values['packaging/private-runtimes.json'], overlay or b'standard')
            with patch.object(c, 'archive_stream', return_value=archive_bytes([('AGENTS.override.md', b'private', {})])):
                with self.assertRaisesRegex(ValueError, 'Private source'):
                    c.source_archive(root, identity, '2')

    @unittest.skipUnless(all(shutil.which(tool) for tool in ['dpkg-deb', 'readelf', 'cc', 'bsdtar', 'zstd']), 'Packaging/readelf/C compiler tools required')
    def test_complete_deb_and_arch_replace_the_application_and_preserve_pinned_runtimes(self):
        # Compile a minimal ELF for its headers/symbols; never launch it or any audio engine.
        with tempfile.TemporaryDirectory(prefix='sn-complete-deb-') as directory:
            root = Path(directory)
            source, inputs_dir, tasks, output = [root / name for name in ['source', 'inputs', 'build', 'output']]
            for path in [source, inputs_dir, tasks, output]:
                path.mkdir()
            pin = hashlib.sha256(b'fixture runtime').hexdigest()
            descriptor = json.dumps({'schema': 2, 'install_root': '/' + c.RUNTIME.rstrip('/'),
                                     'binaries': {name: pin for name in ['mpd-direct', 'mpd-eq', 'camilladsp']}, 'resources': {}}).encode()
            (source / 'packaging').mkdir()
            (source / 'packaging/runtime.json').write_bytes(descriptor)
            program = root / 'fixture.c'
            program.write_text('int main(void) { return 0; }\n')
            binary = root / 'fixture-elf'
            subprocess.run(['cc', str(program), '-Wl,-z,nopack-relative-relocs', '-o', str(binary)], check=True, capture_output=True)
            application = binary.read_bytes() + descriptor
            sources = {'LICENSE': b'fixture license', 'AUTHORS': b'fixture authors', 'THIRD_PARTY_NOTICES': b'fixture notices',
                       'sonante-arch/sonante.desktop': b'fixture desktop',
                       **{'src-tauri/icons/' + name: b'fixture icon' for name in ['32x32.png', '128x128.png', '128x128@2x.png', 'icon.png']}}
            for name, data in sources.items():
                path = source / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(data)
            sidecar = {'filename': 'fixture-sources', 'sha256': 'a' * 64, 'bytes': 1}
            files = {'usr/bin/sonante': application,
                     c.RUNTIME + 'manifest.json': descriptor,
                     **{c.RUNTIME + name: b'fixture runtime' for name in ['mpd-direct', 'mpd-eq', 'camilladsp']},
                     'usr/share/licenses/sonante/LICENSE': b'old fixture license',
                     'usr/share/doc/sonante/AUTHORS': b'old authors', 'usr/share/doc/sonante/THIRD_PARTY_NOTICES': b'old notices',
                     'usr/share/doc/sonante/RELEASE-BUILD.json': b'{}',
                     'usr/share/doc/sonante/runtime-inventory.json': json.dumps({'distribution_ready': True, 'source_trees_installed': False, 'source_sidecar': sidecar}).encode(),
                     'usr/share/applications/sonante.desktop': b'old desktop',
                     **{'usr/share/icons/hicolor/' + size + '/apps/sonante.png': b'old icon'
                        for size in ['32x32', '128x128', '256x256', '512x512']}}
            def make_deb(path, version, contents):
                stage = root / (path.name + '-stage')
                stage.mkdir()
                for name, data in contents.items():
                    file = stage / name
                    file.parent.mkdir(parents=True, exist_ok=True)
                    file.write_bytes(data)
                    file.chmod(0o755 if name == 'usr/bin/sonante' or name in {c.RUNTIME + n for n in ['mpd-direct', 'mpd-eq', 'camilladsp']} else 0o644)
                control = stage / 'DEBIAN'
                control.mkdir()
                (control / 'control').write_text('Package: sonante\nVersion: ' + version + '\nArchitecture: amd64\nMaintainer: Fixture\nDepends: libc6 (>= 2.35), libwebkit2gtk-4.1-0 (>= 2.40)\nDescription: fixture\n')
                (control / 'md5sums').write_text(''.join(c.digest(stage / name, 'md5') + '  ' + name + '\n' for name in sorted(contents)))
                subprocess.run(['dpkg-deb', '--build', '--root-owner-group', str(stage), str(path)], check=True, capture_output=True)
            base = inputs_dir / 'baseline.deb'
            make_deb(base, '0.5.5-10', files)
            candidates = root / 'candidates'
            native = candidates / 'ubuntu'
            native.mkdir(parents=True)
            candidate = native / 'candidate.deb'
            make_deb(candidate, '0.5.6', {'usr/bin/sonante': application})
            (native / 'source-commit.txt').write_text('b' * 40 + '\n')
            sums = native / 'SHA256SUMS-ubuntu.txt'
            sums.write_text(''.join(c.digest(p) + '  ' + p.name + '\n' for p in [candidate, native / 'source-commit.txt']))
            entry = {'filename': base.name, 'bytes': base.stat().st_size, 'sha256': c.digest(base), 'payload_files': len(files),
                     'descriptor': 'packaging/runtime.json', 'descriptor_sha256': hashlib.sha256(descriptor).hexdigest()}
            identity = {'version': '0.5.6', 'commit': 'b' * 40, 'tree': 'c' * 40, 'epoch': 1}
            with patch.object(c, 'ROOT', source):
                receipt = c.assemble('ubuntu', {'packages': {'ubuntu': entry}, 'sources': [sidecar]}, candidates, tasks, output, identity, '1')
            package = output / receipt['package']
            payload, _ = c.inspect_package(package, 'ubuntu')
            self.assertEqual(receipt['version'], '0.5.6')
            self.assertEqual(receipt['revision'], '1')
            self.assertEqual(receipt['application_sha256'], hashlib.sha256(application).hexdigest())
            self.assertEqual(set(payload), set(files))
            self.assertEqual(payload[c.RUNTIME + 'mpd-direct']['sha256'], pin)
            self.assertEqual(payload['usr/share/licenses/sonante/LICENSE']['sha256'], hashlib.sha256(sources['LICENSE']).hexdigest())
            self.assertEqual(c.run('dpkg-deb', '-f', package, 'Version').strip(), '0.5.6-1')
            c.verify_deb_controls(package)

            # The Arch result is also encoded and reread, including directory entries and its actual glibc floor.
            previous = 'pkgname = sonante\narch = x86_64\ndepend = glibc>=2.35\ndepend = pipewire\ndepend = webkit2gtk-4.1>=2.40\n'
            arch_base_stage = root / 'arch-base-stage'
            arch_base_stage.mkdir()
            c.inspect_package(base, 'ubuntu', arch_base_stage)
            arch_base = inputs_dir / 'baseline.pkg.tar.zst'
            arch_work = root / 'arch-work'
            arch_work.mkdir()
            c.build_arch(previous, arch_base_stage, arch_base, arch_work, '0.5.5', '10', '2.35', 1)
            native_arch = candidates / 'arch'
            native_arch.mkdir()
            native_stage = root / 'native-arch-stage'
            (native_stage / 'usr/bin').mkdir(parents=True)
            (native_stage / 'usr/bin/sonante').write_bytes(application)
            (native_stage / 'usr/bin/sonante').chmod(0o755)
            candidate_work = root / 'candidate-arch-work'
            candidate_work.mkdir()
            arch_candidate = native_arch / 'candidate.pkg.tar.zst'
            c.build_arch(previous, native_stage, arch_candidate, candidate_work, '0.5.6', '1', '2.35', 1)
            (native_arch / 'source-commit.txt').write_text(identity['commit'] + '\n')
            (native_arch / 'SHA256SUMS-arch.txt').write_text(''.join(c.digest(p) + '  ' + p.name + '\n' for p in [arch_candidate, native_arch / 'source-commit.txt']))
            arch_entry = {**entry, 'filename': arch_base.name, 'bytes': arch_base.stat().st_size, 'sha256': c.digest(arch_base)}
            with patch.object(c, 'ROOT', source):
                result = c.assemble('arch', {'packages': {'arch': arch_entry}, 'sources': [sidecar]}, candidates, tasks, output, identity, '1')
            final_arch = output / result['package']
            payload, metadata = c.inspect_package(final_arch, 'arch')
            self.assertEqual(set(payload), set(files))
            self.assertEqual(payload[c.RUNTIME + 'mpd-direct']['sha256'], pin)
            self.assertEqual(set(metadata), {'.PKGINFO', '.MTREE'})
            self.assertIn('depend = glibc>=' + result['minimum_glibc'] + '\n', metadata['.PKGINFO']['data'].decode())
            with c.archive_stream(c.package_command(final_arch, 'arch')) as stream:
                with tarfile.open(fileobj=stream, mode='r|') as archive:
                    self.assertTrue(any(item.isdir() and item.name == 'usr/bin' for item in archive))


if __name__ == '__main__':
    unittest.main()
