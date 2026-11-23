"""Debian system-Nettle profile and unchanged non-DEB profile contracts."""
import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('complete_packages', 'scripts/complete-packages.py')
c = importlib.util.module_from_spec(spec)
spec.loader.exec_module(c)


class DebianSystemNettleTests(unittest.TestCase):
    def setUp(self):
        self.inputs = json.loads(Path('packaging/complete-packages.json').read_text())

    def test_manifest_delta_is_only_private_nettle_for_ubuntu(self):
        entries = self.inputs['packages']
        base_path = Path(entries['ubuntu']['baseline_descriptor'])
        target_path = Path(entries['ubuntu']['descriptor'])
        base_bytes, target_bytes = base_path.read_bytes(), target_path.read_bytes()
        self.assertEqual(hashlib.sha256(base_bytes).hexdigest(), entries['ubuntu']['baseline_descriptor_sha256'])
        self.assertEqual(hashlib.sha256(target_bytes).hexdigest(), entries['ubuntu']['descriptor_sha256'])
        base, target = json.loads(base_bytes), json.loads(target_bytes)
        self.assertEqual(base['binaries'], target['binaries'], 'MPD executables and hashes remain fixed')
        self.assertEqual(base['resources']['mpd-direct']['lib/libnettle.so.8'],
                         base['resources']['mpd-eq']['lib/libnettle.so.8'])
        before, after = c.descriptor_files(base_bytes), c.descriptor_files(target_bytes)
        self.assertEqual(set(before) - set(after), {'lib/libnettle.so.8'})
        self.assertEqual(set(after) - set(before), set())
        self.assertEqual({name for name in set(before) & set(after) if before[name] != after[name]}, {'manifest.json'})
        for runtime in ['mpd-direct', 'mpd-eq']:
            self.assertNotIn('lib/libnettle.so.8', target['resources'][runtime])
            self.assertEqual({k: v for k, v in base['resources'][runtime].items() if k != 'lib/libnettle.so.8'},
                             target['resources'][runtime])

    def test_assembler_removes_only_nettle_and_preserves_mpd_hashes(self):
        mpd = b'fixture mpd'
        camilla = b'fixture camilladsp'
        nettle = b'fixture nettle'
        mpd_hash, camilla_hash, nettle_hash = [hashlib.sha256(value).hexdigest() for value in (mpd, camilla, nettle)]
        baseline = json.dumps({'schema': 2, 'install_root': '/' + c.RUNTIME.rstrip('/'),
            'binaries': {'mpd-direct': mpd_hash, 'mpd-eq': mpd_hash, 'camilladsp': camilla_hash},
            'resources': {'mpd-direct': {'lib/libnettle.so.8': nettle_hash},
                          'mpd-eq': {'lib/libnettle.so.8': nettle_hash}}}).encode()
        target = json.dumps({'schema': 2, 'install_root': '/' + c.RUNTIME.rstrip('/'),
            'binaries': {'mpd-direct': mpd_hash, 'mpd-eq': mpd_hash, 'camilladsp': camilla_hash},
            'resources': {}}).encode()
        contents_by_name = {'mpd-direct': mpd, 'mpd-eq': mpd, 'camilladsp': camilla,
                            'lib/libnettle.so.8': nettle}
        pins = json.loads(baseline)
        with tempfile.TemporaryDirectory(prefix='sonante-deb-nettle-') as tmp:
            root = Path(tmp)
            stage = root / 'stage'
            runtime = stage / c.RUNTIME
            runtime.mkdir(parents=True)
            (runtime / 'manifest.json').write_bytes(baseline)
            files = {c.RUNTIME + 'manifest.json': {
                'sha256': hashlib.sha256(baseline).hexdigest(), 'bytes': len(baseline), 'mode': 0o644}}
            for name, digest in c.descriptor_files(baseline).items():
                if name == 'manifest.json':
                    continue
                path = runtime / name
                path.parent.mkdir(parents=True, exist_ok=True)
                contents = contents_by_name[name]
                path.write_bytes(contents)
                path.chmod(0o755 if name in pins['binaries'] else 0o644)
                files[c.RUNTIME + name] = {'sha256': digest, 'bytes': len(contents),
                                           'mode': 0o755 if name in pins['binaries'] else 0o644}
            c.reconcile_runtime_profile(stage, files, 'ubuntu', baseline, target)
            self.assertFalse((runtime / 'lib/libnettle.so.8').exists())
            self.assertEqual((runtime / 'manifest.json').read_bytes(), target)
            self.assertNotIn(c.RUNTIME + 'lib/libnettle.so.8', files)
            for name, digest in pins['binaries'].items():
                self.assertEqual(hashlib.sha256((runtime / name).read_bytes()).hexdigest(), digest)
            self.assertEqual(set(c.descriptor_files(target)),
                             {p.relative_to(runtime).as_posix() for p in runtime.rglob('*') if p.is_file()})

    def test_arch_and_fedora_descriptors_remain_byte_identical_to_baselines(self):
        for variant in ['arch', 'fedora']:
            entry = self.inputs['packages'][variant]
            self.assertEqual(Path(entry['descriptor']).read_bytes(), Path(entry['baseline_descriptor']).read_bytes())
            self.assertEqual(entry['descriptor_sha256'], entry['baseline_descriptor_sha256'])
        pkgbuild = Path('sonante-arch/PKGBUILD').read_text()
        self.assertIn('install -m644 packaging/qualified-0.5.4/ubuntu-arch.runtime.json packaging/private-runtimes.json', pkgbuild)
        stage_spec = importlib.util.spec_from_file_location('stage_private_runtimes', 'scripts/stage-private-runtimes.py')
        stage = importlib.util.module_from_spec(stage_spec)
        stage_spec.loader.exec_module(stage)
        arch_manifest = Path('packaging/qualified-0.5.4/ubuntu-arch.runtime.json')
        self.assertEqual(stage.approved_manifest(arch_manifest), arch_manifest.resolve())
        self.assertIn('lib/libnettle.so.8', json.loads(arch_manifest.read_text())['resources']['mpd-direct'])
        verifier_spec = importlib.util.spec_from_file_location('verify_private_runtime_stage', 'scripts/verify-private-runtime-stage.py')
        verifier = importlib.util.module_from_spec(verifier_spec)
        verifier_spec.loader.exec_module(verifier)
        self.assertEqual(verifier.approved_profile(arch_manifest.read_bytes()), arch_manifest.resolve())
        with self.assertRaisesRegex(ValueError, 'approved platform profile'):
            stage.approved_manifest('packaging/not-approved.json')

    def test_deb_depends_supports_jammy_and_t64_nettle_package_names(self):
        control = 'Package: sonante\nDepends: libc6 (>= 2.35), libwebkit2gtk-4.1-0 (>= 2.40)\nDescription: fixture\n'
        updated = c.ensure_control_dependency(control, 'Depends', 'libnettle8 | libnettle8t64')
        self.assertIn('libnettle8 | libnettle8t64', updated)
        self.assertEqual(c.ensure_control_dependency(updated, 'Depends', 'libnettle8 | libnettle8t64'), updated)
        folded = 'Package: sonante\nDepends: libc6,\n libwebkit2gtk-4.1-0\nDescription: fixture\n'
        folded_updated = c.ensure_control_dependency(folded, 'Depends', 'libnettle8 | libnettle8t64')
        self.assertIn('Depends: libc6, libwebkit2gtk-4.1-0, libnettle8 | libnettle8t64\n', folded_updated)
        for existing in ['libnettle8', 'libnettle8 | libnettle8t64 | other-package']:
            with self.subTest(existing=existing), self.assertRaisesRegex(ValueError, 'Incomplete existing'):
                c.ensure_control_dependency('Depends: ' + existing + '\n', 'Depends', 'libnettle8 | libnettle8t64')


if __name__ == '__main__':
    unittest.main()
