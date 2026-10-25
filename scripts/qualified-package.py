#!/usr/bin/env python3
"""Inspect/reconstruct checksum-pinned approved private installers, never install/run them."""
import argparse
from contextlib import contextmanager
import hashlib
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import tempfile

ROOT = Path(__file__).resolve().parent.parent
PROFILES = ROOT / 'packaging/qualified-0.5.4'
RUNTIME = 'usr/lib/sonante/runtimes/v1/'
RPM_QUERY = '%{NAME}\n%{VERSION}-%{RELEASE}\n%{ARCH}\n[%{REQUIRENAME}\t%{REQUIREFLAGS}\t%{REQUIREVERSION}\n]'


def require(ok, message):
    if not ok:
        raise ValueError(message)


def stream_digest(stream):
    checksum = hashlib.sha256()
    for block in iter(lambda: stream.read(1024 * 1024), b''):
        checksum.update(block)
    return checksum.hexdigest()


def digest(path):
    with Path(path).open('rb') as f:
        return stream_digest(f)


def safe_name(raw):
    require(not raw.startswith('/') and '\\' not in raw, 'Absolute/invalid archive path')
    p = PurePosixPath(raw)
    require('..' not in p.parts, 'Archive traversal')
    return str(p)


def profile(name):
    all_profiles = json.loads((PROFILES / 'profiles.json').read_text())
    require(all_profiles['schema'] == 1, 'Unknown qualification schema')
    p = all_profiles['profiles'][name]
    for key in ['compiled_descriptor', 'payload_manifest', 'source_manifest', 'recipe'] + (['control'] if 'control' in p else []):
        item = p[key]
        path = PROFILES / safe_name(item['file'])
        require(not path.is_symlink() and digest(path) == item['sha256'], 'Profile input changed: ' + key)
    require(set(p['source_overrides']) == ({'packaging/private-runtimes.json'} if p['format'] == 'rpm' else set()), 'Unapproved source delta')
    if 'control' in p:
        require((PROFILES / p['control']['file']).read_text().strip() == p['metadata'].strip(), 'DEB control recipe/metadata disagree')
    payload = json.loads((PROFILES / p['payload_manifest']['file']).read_text())
    descriptor = (PROFILES / p['compiled_descriptor']['file']).read_bytes()
    runtime = json.loads(descriptor)
    expected = dict(runtime['binaries'])
    for resources in runtime['resources'].values():
        for path, sha in resources.items():
            require(path not in expected or expected[path] == sha, 'Inconsistent resource hashes')
            expected[path] = sha
    require(runtime['install_root'] == '/' + RUNTIME.rstrip('/'), 'Runtime root changed')
    require(runtime['abi'] == {'glibc_ceiling': '2.35', 'webkit_api': '4.1', 'webkit_minimum': '2.40'}, 'ABI contract changed')
    actual = {n[len(RUNTIME):]: e['sha256'] for n, e in payload.items() if n.startswith(RUNTIME)}
    expected['manifest.json'] = hashlib.sha256(descriptor).hexdigest()
    require(actual == expected, 'Profile payload/compiled descriptor disagree')
    require('lib/libpipewire-0.3.so.0' not in actual and 'lib/camilla/libpipewire-0.3.so.0' in actual, 'MPD must use system PipeWire client')
    if p['format'] == 'rpm':
        require('lib/libnettle.so.8' not in actual and {'lib/libbz2.so.1.0', 'lib/libsndio.so.7'} <= actual.keys(), 'Fedora compatibility profile changed')
    else:
        require('lib/libnettle.so.8' in actual and not {'lib/libbz2.so.1.0', 'lib/libsndio.so.7'} & actual.keys(), 'Ubuntu/Arch profile changed')
    return p, payload, descriptor


def deb_fields(text):
    result = {}
    current = None
    for line in text.splitlines():
        if line.startswith((' ', '\t')) and current:
            result[current] += ' ' + line.strip()
        elif ': ' in line:
            current, value = line.split(': ', 1)
            require(current not in result, 'Duplicate DEB field')
            result[current] = value
    return result


def validate_metadata(p, text):
    if p['format'] == 'deb':
        f = deb_fields(text)
        for key in ['Depends', 'Pre-Depends', 'Recommends', 'Suggests']:
            require(not re.search(r'(?<![\w-])wireplumber(?=[:\s,(|]|$)', f.get(key, '')), 'DEB must not impose or recommend WirePlumber')
        require(f.get('Package') == p['package_name'] and f.get('Version') == p['version'] + '-' + p['revision'] and f.get('Architecture') == 'amd64', 'DEB identity/version')
        require({'pipewire', 'libpipewire-0.3-0', 'alsa-utils', 'xdg-utils'} <= set(f.get('Depends', '').split(', ')), 'DEB required tools/libraries missing')
        require('libwebkit2gtk-4.1-0 (>= 2.40)' in f.get('Depends', ''), 'DEB WebKit floor missing')
    elif p['format'] == 'rpm':
        require('nettle\t12\t3.10' in text and 'pipewire-utils\t0\t' in text, 'Fedora system Nettle/tools requirement missing')
        require('(webkit2gtk4.1 >= 2.40)\t0\t' in text, 'RPM WebKit rich dependency missing')
    else:
        require('pkgver = ' + p['version'] + '-' + p['revision'] in text and 'arch = x86_64' in text, 'Arch identity/version')
        require('depend = pipewire\n' in text and 'depend = webkit2gtk-4.1>=2.40\n' in text, 'Arch required runtime missing')
    require(text == p['metadata'], 'Actual package metadata differs from approved recipe')


def elf_abi(data):
    fd = os.memfd_create('sonante-package-inspection', flags=0)
    try:
        with os.fdopen(os.dup(fd), 'wb') as f:
            f.write(data)
        text = subprocess.check_output(['readelf', '--version-info', '/proc/self/fd/' + str(fd)], pass_fds=(fd,), text=True)
    finally:
        os.close(fd)
    for namespace, limit in [('GLIBC', (2, 35)), ('GLIBCXX', (3, 4, 30)), ('CXXABI', (1, 3, 13))]:
        versions = [tuple(map(int, v.split('.'))) for v in re.findall(r'\b' + namespace + r'_(\d+(?:\.\d+)+)', text)]
        require(not versions or max(versions) <= limit, 'ELF exceeds ' + namespace + ' ceiling')


def inspect_tar(stream, p, expected, descriptor):
    actual = {}
    auxiliary = p.get('arch_metadata', {})
    allowed_dirs = {str(parent) for n in expected for parent in PurePosixPath(n).parents} | {'.'}
    elf_count = 0
    metadata = None
    with tarfile.open(fileobj=stream, mode='r|') as tar:
        for member in tar:
            name = safe_name(member.name)
            require(member.uid == member.gid == 0 and not member.mode & 0o6022, 'Unsafe package ownership/mode: ' + name)
            if member.isdir():
                require(name in allowed_dirs, 'Unexpected package directory: ' + name)
                continue
            require(member.isfile() and name not in actual, 'Links/special/duplicate package entry: ' + name)
            require(name in expected or name in auxiliary, 'Unexpected package file: ' + name)
            require(member.size == (expected | auxiliary)[name]['bytes'], 'Package file size mismatch: ' + name)
            data = tar.extractfile(member).read()
            entry = {'sha256': hashlib.sha256(data).hexdigest(), 'mode': member.mode, 'bytes': len(data)}
            require(entry == (expected | auxiliary)[name], 'Package file hash/mode/size mismatch: ' + name)
            actual[name] = entry
            if name == '.PKGINFO':
                metadata = data.decode()
            if name == 'usr/bin/sonante':
                require(descriptor in data, 'Application compiled descriptor differs from installed profile')
                sanitized = data.replace(b'/hubs/home/recentlyAdded?type=8', b'/hubs/APPROVED_PLEX_ENDPOINT')
                require(not re.search(rb'/home/[^/\x00\s]+/|/build/(?:app-source|app-vendor|app-target)/|/root/\.cargo/', sanitized), 'Private build path in application')
            if data.startswith(b'\x7fELF'):
                elf_abi(data)
                elf_count += 1
    require(actual == expected | auxiliary, 'Missing motor/resource/manifest/payload file')
    require(elf_count == p['elf_count'], 'ELF inventory mismatch')
    return metadata, elf_count


def validate_rpm_properties(provides, scripts):
    require(not any('.so' in line.split()[0] for line in provides.splitlines() if line.strip()) and 'sonante = 0.5.4' in provides, 'Private SONAMEs must not be global RPM Provides')
    require(not scripts.strip(), 'RPM scriptlets forbidden')


@contextmanager
def archive_stream(command):
    # Always drain or reap the inspection child, including malformed archives.
    with tempfile.TemporaryFile() as errors:
        proc = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=errors)
        try:
            yield proc.stdout
            while proc.stdout.read(1024 * 1024):
                pass
            status = proc.wait()
            errors.seek(0)
            require(status == 0, 'Archive command failed: ' + errors.read(65536).decode(errors='replace'))
        finally:
            if proc.poll() is None:
                proc.kill()
            proc.wait()
            proc.stdout.close()


def verify_package(package, p, expected, descriptor):
    fmt = p['format']
    if fmt == 'deb':
        metadata = subprocess.check_output(['dpkg-deb', '-f', str(package)], text=True)
        validate_metadata(p, metadata)
        # Reject all maintainer scripts, not merely a recognized script name.
        controls = subprocess.check_output(['dpkg-deb', '--ctrl-tarfile', str(package)])
        with tarfile.open(fileobj=io.BytesIO(controls)) as tar:
            for m in tar:
                name = safe_name(m.name)
                require(m.isdir() or (name in {'control', 'md5sums'} and m.isfile() and m.uid == m.gid == 0), 'Unexpected DEB control/script')
        command = ['dpkg-deb', '--fsys-tarfile', str(package)]
    elif fmt == 'rpm':
        metadata = subprocess.check_output(['rpm', '-qp', '--queryformat', RPM_QUERY, str(package)], text=True)
        validate_metadata(p, metadata)
        provides = subprocess.check_output(['rpm', '-qp', '--provides', str(package)], text=True)
        validate_rpm_properties(provides, subprocess.check_output(['rpm', '-qp', '--scripts', str(package)], text=True))
        command = ['bsdtar', '-cf', '-', '--format=gnutar', '@' + str(package)]
    else:
        command = ['zstd', '-dc', str(package)]
    with archive_stream(command) as stream:
        arch_metadata, elf_count = inspect_tar(stream, p, expected, descriptor)
    if fmt == 'arch':
        validate_metadata(p, arch_metadata)
    # Content checks precede the fixed identity check: a hash alone is not qualification.
    require(digest(package) == p['package_sha256'] and package.stat().st_size == p['package_bytes'], 'Package is not the approved revision')
    return {'files': len(expected), 'elf': elf_count, 'package_sha256': digest(package)}


def validate_sources(data, p):
    expected = json.loads((PROFILES / p['source_manifest']['file']).read_text()) | p['source_overrides']
    seen = {}
    versions = {}
    with tarfile.open(fileobj=io.BytesIO(data)) as tar:
        for m in tar:
            name = safe_name(m.name)
            require(name.startswith('sonante-private-source/') and m.isfile(), 'Unexpected source snapshot entry')
            name = name.split('/', 1)[1]
            require(name not in seen and name in expected, 'Unexpected/duplicate source file')
            value = tar.extractfile(m).read()
            seen[name] = {'sha256': hashlib.sha256(value).hexdigest(), 'mode': m.mode}
            if name in {'package.json', 'src-tauri/tauri.conf.json'}:
                versions[name] = json.loads(value)['version']
    require(seen == expected and len(seen) == 416, 'Source snapshot/profile mismatch')
    require(set(versions.values()) == {p['version']}, 'Source version differs from installer')


def inspect_inputs(archive, p, output=None):
    require(digest(archive) == p['external_inputs']['sha256'], 'Build-input archive changed')
    expected = p['selected_inputs']
    seen = set()
    prefix = p['external_inputs']['prefix'] + '/'
    with archive_stream(['zstd', '-dc', str(archive)]) as stream:
        with tarfile.open(fileobj=stream, mode='r|') as tar:
            for m in tar:
                name = safe_name(m.name)
                require(name.startswith(prefix), 'Unexpected build-input prefix')
                name = name[len(prefix):]
                if name not in expected:
                    continue
                require(m.isfile() and name not in seen, 'Invalid selected build input')
                f = tar.extractfile(m)
                # Only small application source snapshot is opened; runtime/source
                # sidecars remain archives. No engine source tree is re-extracted.
                if name == 'sonante-private-source.tar':
                    data = f.read()
                    require(hashlib.sha256(data).hexdigest() == expected[name], 'Source tar changed')
                    validate_sources(data, p)
                    if output:
                        (output / name).write_bytes(data)
                elif output:
                    with (output / name).open('xb') as target:
                        shutil.copyfileobj(f, target)
                    require(digest(output / name) == expected[name], 'Selected build input changed')
                else:
                    require(stream_digest(f) == expected[name], 'Selected build input changed')
                seen.add(name)
    require(seen == set(expected), 'Missing selected build input')


def external_sources(p, sidecar, compat):
    require(sidecar.name == p['source_sidecar']['filename'] and digest(sidecar) == p['source_sidecar']['sha256'], 'Corresponding runtime source sidecar required')
    if 'compat_sources' in p:
        require(compat is not None and compat.name == p['compat_sources']['filename'] and digest(compat) == p['compat_sources']['sha256'], 'Fedora compatibility source sidecar required')


def prepare(p, inputs, output):
    require(not output.exists(), 'Preparation requires a new empty output path')
    output.parent.mkdir(parents=True, exist_ok=True)
    # Publish prepared inputs only after every check, never a partial ready folder.
    with tempfile.TemporaryDirectory(prefix='sonante-prepare-', dir=output.parent) as temp:
        dest = Path(temp) / 'inputs'
        dest.mkdir()
        inspect_inputs(inputs, p, dest)
        recipe = PROFILES / p['recipe']['file']
        target = {'arch': 'PKGBUILD', 'rpm': 'sonante-private.spec', 'deb': 'repackage-deb.py'}[p['format']]
        shutil.copyfile(recipe, dest / target)
        if p['format'] == 'deb':
            shutil.copyfile(PROFILES / 'recipes/ubuntu.control', dest / 'debian-control')
        (dest / 'qualification-profile.json').write_text(json.dumps(p, indent=2) + '\n')
        dest.rename(output)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=['verify', 'prepare'])
    parser.add_argument('--profile', required=True)
    parser.add_argument('--build-inputs', type=Path, required=True)
    parser.add_argument('--source-sidecar', type=Path, required=True)
    parser.add_argument('--compat-sources', type=Path)
    parser.add_argument('--package', type=Path)
    parser.add_argument('--output', type=Path)
    a = parser.parse_args()
    p, payload, descriptor = profile(a.profile)
    external_sources(p, a.source_sidecar.resolve(), a.compat_sources.resolve() if a.compat_sources else None)
    if a.action == 'prepare':
        require(a.output is not None and a.package is None, 'prepare needs --output only')
        prepare(p, a.build_inputs.resolve(), a.output.resolve())
        result = {'prepared': True, 'package_generated': False, 'installed': False}
    else:
        require(a.package is not None and a.output is None, 'verify needs --package only')
        result = verify_package(a.package.resolve(), p, payload, descriptor)
        inspect_inputs(a.build_inputs.resolve(), p)
        result.update(qualified_content=True, runtime_source_sidecar=True, exact_build_inputs=True)
    print(json.dumps({'profile': a.profile, **result, 'new_hardware_validation': False, 'public_release_authorized': False}, indent=2))


if __name__ == '__main__':
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError, tarfile.TarError) as error:
        raise SystemExit('QUALIFICATION FAILED: ' + str(error))
