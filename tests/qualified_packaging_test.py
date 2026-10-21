"""Small negative fixtures for the qualified-installer boundary; no app execution."""
import hashlib
import importlib.util
import io
import json
import re
from pathlib import Path
import tarfile
import unittest

spec = importlib.util.spec_from_file_location('qualified', 'scripts/qualified-package.py')
q = importlib.util.module_from_spec(spec)
spec.loader.exec_module(q)


class QualifiedPackageTests(unittest.TestCase):
    def archive(self, files, mode=0o644, uid=0, special=False):
        target = io.BytesIO()
        with tarfile.open(fileobj=target, mode='w') as tar:
            for name, data in files.items():
                item = tarfile.TarInfo(name)
                item.mode = mode
                item.uid = uid
                item.size = len(data)
                if special:
                    item.type = tarfile.SYMTYPE
                    item.linkname = '/etc/passwd'
                tar.addfile(item, io.BytesIO(data) if not special else None)
        target.seek(0)
        return target

    def setUp(self):
        self.files = {q.RUNTIME + n: n.encode() for n in ['mpd-direct', 'mpd-eq', 'camilladsp', 'manifest.json', 'lib/resource.so']}
        self.expected = {n: {'sha256': hashlib.sha256(b).hexdigest(), 'mode': 0o644, 'bytes': len(b)} for n, b in self.files.items()}
        self.p = {'elf_count': 0}

    def test_all_committed_profiles_bind_payload_descriptor_and_recipe(self):
        for name in json.loads((q.PROFILES / 'profiles.json').read_text())['profiles']:
            with self.subTest(profile=name):
                p, payload, descriptor = q.profile(name)
                self.assertIn('usr/bin/sonante', payload)
                self.assertEqual(len(payload), 174 if p['format'] == 'rpm' else 171)
                self.assertEqual(json.loads(descriptor)['abi']['glibc_ceiling'], '2.35')
                q.validate_metadata(p, p['metadata'])

    def test_valid_inventory_fixture(self):
        q.inspect_tar(self.archive(self.files), self.p, self.expected, b'')

    def test_each_missing_motor_resource_manifest_is_rejected(self):
        for name in self.files:
            with self.subTest(missing=name), self.assertRaisesRegex(ValueError, 'Missing motor/resource/manifest'):
                q.inspect_tar(self.archive({n: b for n, b in self.files.items() if n != name}), self.p, self.expected, b'')

    def test_modified_resource_extra_file_and_duplicate_are_rejected(self):
        bad = dict(self.files)
        bad[q.RUNTIME + 'lib/resource.so'] = b'x' * len(bad[q.RUNTIME + 'lib/resource.so'])
        with self.assertRaisesRegex(ValueError, 'hash/mode/size'):
            q.inspect_tar(self.archive(bad), self.p, self.expected, b'')
        with self.assertRaisesRegex(ValueError, 'Unexpected package file'):
            q.inspect_tar(self.archive({'etc/mpd.conf': b'bad'}), self.p, self.expected, b'')
        a = self.archive({q.RUNTIME + 'mpd-direct': b'mpd-direct'})
        with tarfile.open(fileobj=a, mode='a') as tar:
            item = tarfile.TarInfo(q.RUNTIME + 'mpd-direct'); item.mode = 0o644; item.size = 10
            tar.addfile(item, io.BytesIO(b'mpd-direct'))
        a.seek(0)
        with self.assertRaisesRegex(ValueError, 'duplicate'):
            q.inspect_tar(a, self.p, self.expected, b'')

    def test_nonroot_writable_and_symlink_are_rejected(self):
        for kwargs in [{'uid': 1000}, {'mode': 0o666}, {'special': True}]:
            with self.subTest(kwargs=kwargs), self.assertRaises(ValueError):
                q.inspect_tar(self.archive(self.files, **kwargs), self.p, self.expected, b'')

    def test_unsafe_archive_paths(self):
        for name in ['/usr/bin/sonante', '../usr/bin/sonante', 'usr/../../etc/passwd', 'usr\\bin\\sonante']:
            with self.subTest(name=name), self.assertRaises(ValueError):
                q.safe_name(name)

    def test_truncated_tar_is_rejected(self):
        with self.assertRaises((ValueError, tarfile.TarError)):
            q.inspect_tar(io.BytesIO(b'truncated'), self.p, self.expected, b'')

    def test_deb_wireplumber_rejected_in_all_dependency_fields(self):
        p, _, _ = q.profile('ubuntu-20261020.2')
        for field in ['Depends', 'Pre-Depends', 'Recommends', 'Suggests']:
            for value in ['wireplumber', 'wireplumber:any (>= 0.4)', 'foo | wireplumber']:
                text = p['metadata'] + field + ': ' + value + '\n' if field not in {'Depends', 'Recommends'} else p['metadata'].replace(field + ': ', field + ': ' + value + ', ', 1)
                with self.subTest(field=field, value=value), self.assertRaisesRegex(ValueError, 'WirePlumber'):
                    q.validate_metadata(p, text)
        folded = p['metadata'].replace('Depends: ', 'Depends: foo,\n wireplumber, ', 1)
        with self.assertRaisesRegex(ValueError, 'WirePlumber'):
            q.validate_metadata(p, folded)

    def test_missing_real_deb_library_or_tools_rejected(self):
        p, _, _ = q.profile('ubuntu-20261020.2')
        for dep in ['libpipewire-0.3-0', 'pipewire', 'alsa-utils', 'xdg-utils']:
            with self.subTest(dep=dep), self.assertRaises(ValueError):
                q.validate_metadata(p, re.sub(r'(?m)^Depends: (.*)$', lambda m: 'Depends: '+', '.join(x for x in m[1].split(', ') if x!=dep), p['metadata']))

    def test_wrong_compiled_descriptor_rejected(self):
        payload = b'ELF bytes with actual descriptor'
        expected = {'usr/bin/sonante': {'sha256': hashlib.sha256(payload).hexdigest(), 'mode': 0o644, 'bytes': len(payload)}}
        with self.assertRaisesRegex(ValueError, 'compiled descriptor'):
            q.inspect_tar(self.archive({'usr/bin/sonante': payload}), {'elf_count': 0}, expected, b'wrong-profile')

    def test_fedora_requires_system_nettle_and_actual_tools(self):
        p, _, _ = q.profile('fedora-20261020.3')
        for token in ['nettle\t12\t3.10', 'pipewire-utils\t0\t', '(webkit2gtk4.1 >= 2.40)\t0\t']:
            with self.subTest(token=token), self.assertRaises(ValueError):
                q.validate_metadata(p, p['metadata'].replace(token, 'missing'))

    def test_rpm_private_global_provides_and_scripts_rejected(self):
        q.validate_rpm_properties('sonante = 0.5.4\nsonante-private = 0.5.4-20261020.3\n', '')
        for extra in ['libnettle.so.8()(64bit)', 'libpipewire-0.3.so = 1']:
            with self.subTest(extra=extra), self.assertRaises(ValueError):
                q.validate_rpm_properties('sonante = 0.5.4\n'+extra, '')
        with self.assertRaises(ValueError):
            q.validate_rpm_properties('sonante = 0.5.4', 'postinstall scriptlet: reset')

    def test_source_snapshot_mismatch_is_rejected(self):
        p, _, _ = q.profile('ubuntu-20261020.2')
        with self.assertRaises(ValueError):
            q.validate_sources(self.archive({'sonante-private-source/package.json': b'{"version":"0.5.4"}'}).getvalue(), p)

    def test_source_sidecar_identity_required(self):
        p, _, _ = q.profile('ubuntu-20261020.2')
        with self.assertRaises((ValueError, FileNotFoundError)):
            q.external_sources(p, Path('missing-source-sidecar'), None)


if __name__ == '__main__':
    unittest.main()
