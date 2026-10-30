#!/usr/bin/env python3
"""Assemble complete Linux installers from a clean checkout and its CI binaries.

Pinned public installers supply unchanged runtimes/licenses, never an application.
No application, audio engine, package installation or user profile is executed.
"""
import argparse
from contextlib import contextmanager
import gzip
import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import tarfile
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
RUNTIME = 'usr/lib/sonante/runtimes/v1/'
MAX_FILE = 128 * 1024 * 1024
MAX_PAYLOAD = 400 * 1024 * 1024
VARIANTS = ('ubuntu', 'fedora', 'arch')


def require(ok, message):
    if not ok:
        raise ValueError(message)


def digest(path, algorithm='sha256'):
    value = hashlib.new(algorithm)
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            value.update(block)
    return value.hexdigest()


def safe_name(raw):
    require(isinstance(raw, str) and 0 < len(raw) <= 4096
            and not re.search(r'[\\\x00-\x1f\x7f]', raw), 'Invalid archive path')
    path = PurePosixPath(raw)
    require(not path.is_absolute() and '..' not in path.parts, 'Archive traversal')
    return str(path)


def checked_file(path, expected=None):
    info = path.lstat()
    require(stat.S_ISREG(info.st_mode) and info.st_nlink == 1, 'Expected a regular input file')
    if expected is not None:
        require(info.st_size == expected['bytes'] and digest(path) == expected['sha256'],
                'Pinned input size/SHA256 mismatch: ' + path.name)
    return path


@contextmanager
def archive_stream(command):
    with subprocess.Popen(command, stdout=subprocess.PIPE) as child:
        try:
            yield child.stdout
            while child.stdout.read(1024 * 1024):
                pass
            require(child.wait() == 0, 'Archive reader failed')
        finally:
            if child.poll() is None:
                child.kill()
            child.wait()


def package_command(path, variant):
    return (['dpkg-deb', '--fsys-tarfile', str(path)] if variant == 'ubuntu'
            else ['bsdtar', '-cf', '-', '--format=gnutar', '@' + str(path)])


def read_payload(stream, stage=None, arch=False):
    """Verify actual encoded members; never let tar extract links or special files."""
    files, metadata, seen = {}, {}, set()
    total = 0
    with tarfile.open(fileobj=stream, mode='r|') as archive:
        for item in archive:
            name = safe_name(item.name)
            require(name not in seen, 'Duplicate package entry')
            seen.add(name)
            require(item.uid == item.gid == 0 and not item.mode & 0o7022,
                    'Unsafe package ownership/mode')
            extra = arch and name in {'.PKGINFO', '.MTREE'}
            require(name in {'.', 'usr'} or name.startswith('usr/') or extra,
                    'Unexpected package path')
            if item.isdir():
                require(not extra, 'Invalid Arch metadata')
                if stage and name != '.':
                    (stage / name).mkdir(parents=True, exist_ok=True)
                    (stage / name).chmod(0o755)
                continue
            require(item.isfile() and 0 <= item.size <= MAX_FILE, 'Links/special or oversized file')
            total += item.size
            require(total <= MAX_PAYLOAD, 'Package exceeds payload bound')
            source = archive.extractfile(item)
            value = hashlib.sha256()
            size = 0
            target = None
            data = bytearray() if extra else None
            if stage and not extra:
                path = stage / name
                path.parent.mkdir(parents=True, exist_ok=True)
                target = path.open('xb')
            try:
                for block in iter(lambda: source.read(1024 * 1024), b''):
                    size += len(block)
                    value.update(block)
                    if target:
                        target.write(block)
                    if data is not None:
                        require(size <= 1024 * 1024, 'Oversized Arch metadata')
                        data.extend(block)
            finally:
                if target:
                    target.close()
            require(size == item.size, 'Truncated package file')
            entry = {'sha256': value.hexdigest(), 'bytes': size, 'mode': item.mode}
            if extra:
                metadata[name] = {'entry': entry, 'data': bytes(data)}
            else:
                files[name] = entry
                if stage:
                    (stage / name).chmod(item.mode)
    return files, metadata


def inspect_package(path, variant, stage=None):
    with archive_stream(package_command(path, variant)) as stream:
        return read_payload(stream, stage, variant == 'arch')


def fetch_input(entry, directory):
    name = safe_name(entry['filename'])
    require(PurePosixPath(name).name == name, 'Input filename must be a basename')
    target = directory / name
    if target.exists() or target.is_symlink():
        return checked_file(target, entry)
    require(entry['url'] == 'https://github.com/tiagoalam/Sonante/releases/download/v0.5.5/' + name,
            'Unapproved runtime input origin')
    temporary = directory / (name + '.part')
    require(not temporary.exists() and not temporary.is_symlink(), 'Interrupted download retained; use a fresh inputs directory')
    request = urllib.request.Request(entry['url'], headers={'User-Agent': 'Sonante-complete-packages/1'})
    with urllib.request.urlopen(request, timeout=60) as response, temporary.open('xb') as output:
        require(response.geturl().startswith('https://'), 'Insecure download redirect')
        size = 0
        for block in iter(lambda: response.read(1024 * 1024), b''):
            size += len(block)
            require(size <= entry['bytes'], 'Download exceeds pinned size')
            output.write(block)
    checked_file(temporary, entry)
    temporary.rename(target)
    return target


def verify_checksums(directory, filename):
    rows = checked_file(directory / filename).read_text().splitlines()
    require(bool(rows), 'Missing candidate checksums')
    names = set()
    for row in rows:
        parts = row.split(None, 1)
        require(len(parts) == 2 and re.fullmatch('[a-f0-9]{64}', parts[0]), 'Malformed candidate checksum')
        name = safe_name(parts[1].lstrip('*'))
        require(PurePosixPath(name).name == name and name not in names, 'Unsafe/duplicate checksum filename')
        names.add(name)
        require(digest(checked_file(directory / name)) == parts[0], 'Candidate checksum mismatch')
    return names


def extract_application(package, variant, output):
    found = False
    with archive_stream(package_command(package, variant)) as stream:
        with tarfile.open(fileobj=stream, mode='r|') as archive:
            for item in archive:
                name = safe_name(item.name)
                if name != 'usr/bin/sonante':
                    continue
                require(not found and item.isfile() and 0 < item.size <= MAX_FILE,
                        'Missing/duplicate/invalid CI application')
                with output.open('xb') as target:
                    shutil.copyfileobj(archive.extractfile(item), target)
                found = True
    require(found, 'CI package has no application')
    output.chmod(0o755)
    return output


def run(*args):
    return subprocess.check_output([str(v) for v in args], text=True)


def elf_info(path, arch_application=False):
    header = run('readelf', '-h', path)
    require('ELF64' in header and 'X86-64' in header, 'Expected ELF64 x86_64')
    dynamic, versions = run('readelf', '-d', path), run('readelf', '--version-info', path)
    require('GLIBC_PRIVATE' not in versions, 'Private GLIBC ABI')
    result = {}
    for namespace, limit in [('GLIBC', (2, 35)), ('GLIBCXX', (3, 4, 30)), ('CXXABI', (1, 3, 13))]:
        names = set(re.findall(r'\b' + namespace + r'_[A-Za-z0-9_.]+', versions))
        if namespace == 'GLIBC' and 'GLIBC_ABI_DT_RELR' in names:
            names.remove('GLIBC_ABI_DT_RELR')
            names.add('GLIBC_2.36')
        require(all(re.fullmatch(namespace + r'_\d+(?:\.\d+)+', value) for value in names), 'Unknown symbol ABI')
        values = [tuple(map(int, value.split('_', 1)[1].split('.'))) for value in names]
        if values:
            highest = max(values)
            require(arch_application and namespace == 'GLIBC' or highest <= limit, 'ELF exceeds ABI ceiling')
            result[namespace] = '.'.join(map(str, highest))
    return result, set(re.findall(r'\(NEEDED\).*?\[([^\]]+)\]', dynamic))


def verify_runtime(stage, descriptor):
    pins = json.loads(descriptor)
    require(pins['schema'] == 2 and pins['install_root'] == '/' + RUNTIME.rstrip('/'), 'Unsupported runtime manifest')
    runtime = stage / RUNTIME
    require((runtime / 'manifest.json').read_bytes() == descriptor, 'Installed/compiled descriptor mismatch')
    expected = dict(pins['binaries'])
    for group in pins['resources'].values():
        for name, sha in group.items():
            require(name not in expected or expected[name] == sha, 'Conflicting runtime pins')
            expected[name] = sha
    actual = {str(p.relative_to(runtime)) for p in runtime.rglob('*') if p.is_file()}
    require(actual == set(expected) | {'manifest.json'}, 'Incomplete/extra runtime files')
    for name, sha in expected.items():
        path = checked_file(runtime / safe_name(name))
        require(digest(path) == sha, 'Runtime hash mismatch')
        require(path.stat().st_mode & 0o7777 == (0o755 if name in pins['binaries'] else 0o644), 'Runtime mode mismatch')


def deb_control(base, version, revision, stage):
    text = run('dpkg-deb', '-f', base)
    require(re.search(r'^Package: sonante$', text, re.M) and re.search(r'^Architecture: amd64$', text, re.M), 'DEB identity')
    require('libc6 (>= 2.35)' in text and 'libwebkit2gtk-4.1-0 (>= 2.40)' in text, 'DEB ABI requirements')
    require(not re.search(r'(?m)^(?:Depends|Recommends):.*\bwireplumber\b', text), 'Required WirePlumber is not supported')
    text, count = re.subn(r'^Version: .+$', 'Version: ' + version + '-' + revision, text, flags=re.M)
    require(count == 1, 'DEB version field')
    text = re.sub(r'^Installed-Size: .*\n?', '', text, flags=re.M)
    size = sum(p.stat().st_size for p in stage.rglob('*') if p.is_file())
    return text.rstrip() + '\nInstalled-Size: ' + str((size + 1023) // 1024) + '\n'


def verify_deb_controls(package, expected=None):
    data = subprocess.check_output(['dpkg-deb', '--ctrl-tarfile', str(package)])
    values = {}
    with tarfile.open(fileobj=io.BytesIO(data)) as archive:
        for item in archive:
            name = safe_name(item.name)
            require(item.uid == item.gid == 0 and not item.mode & 0o7022, 'Unsafe DEB control')
            if item.isdir():
                require(name == '.', 'Unexpected control directory')
                continue
            require(item.isfile() and name in {'control', 'md5sums'} and name not in values and item.size <= 1024 * 1024,
                    'DEB installer scripts/invalid control forbidden')
            values[name] = archive.extractfile(item).read()
    require(set(values) == {'control', 'md5sums'}, 'Incomplete DEB controls')
    if expected:
        require(values == expected, 'Encoded DEB control changed')


def build_deb(base, stage, output, version, revision):
    controls = {'control': deb_control(base, version, revision, stage).encode(),
                'md5sums': ''.join(digest(p, 'md5') + '  ' + str(p.relative_to(stage)) + '\n'
                                   for p in sorted(stage.rglob('*')) if p.is_file()).encode()}
    control_dir = stage / 'DEBIAN'
    control_dir.mkdir(mode=0o755)
    for name, data in controls.items():
        (control_dir / name).write_bytes(data)
        (control_dir / name).chmod(0o644)
    # The CLI thread option requires dpkg 1.21.9; Jammy's older dpkg accepts
    # this environment variable and retains its ordinary compressor behavior.
    subprocess.run(['dpkg-deb', '--build', '--root-owner-group', '-Zxz', '-z6', str(stage), str(output)],
                   env={**os.environ, 'DPKG_DEB_THREADS_MAX': '2'}, check=True)
    require(run('dpkg-deb', '-f', output, 'Version').strip() == version + '-' + revision, 'Encoded DEB version')
    verify_deb_controls(output, controls)


def write_tar(path, stage, names, epoch, include_dirs=False):
    if include_dirs:
        directories = {str(parent) for name in names for parent in PurePosixPath(name).parents if str(parent) != '.'}
        names = set(names) | directories
    with tarfile.open(path, 'w', format=tarfile.GNU_FORMAT) as archive:
        for name in sorted(names):
            file = stage / name
            item = archive.gettarinfo(str(file), arcname=name)
            require(item.isfile() or include_dirs and item.isdir(), 'Invalid payload entry')
            item.uid = item.gid = 0
            item.uname = item.gname = 'root'
            item.mtime = epoch
            if item.isfile():
                with file.open('rb') as stream:
                    archive.addfile(item, stream)
            else:
                archive.addfile(item)


def build_rpm(base, stage, output, work, version, revision, epoch):
    require(not run('rpm', '-qp', '--scripts', base).strip(), 'Base RPM scriptlets forbidden')
    requires = {line for line in run('rpm', '-qp', '--requires', base).splitlines() if not line.startswith('rpmlib(')}
    require({'nettle >= 3.10', '(webkit2gtk4.1 >= 2.40)', 'pipewire-utils'} <= requires, 'Fedora requirements missing')
    spec = importlib.util.spec_from_file_location('rpm_elf_deps', ROOT / 'scripts/rpm-elf-deps.py')
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    app = stage / 'usr/bin/sonante'
    requires.update(module.requirements(run('readelf', '-d', app), run('readelf', '--version-info', app)))
    files = sorted(str(p.relative_to(stage)) for p in stage.rglob('*') if p.is_file())
    payload = work / 'payload.tar'
    write_tar(payload, stage, files, epoch)
    text = ('Name: sonante\nVersion: ' + version + '\nRelease: ' + revision + '\n'
            'Summary: Sonante local and Plex music player\n'
            'License: GPL-3.0-only AND GPL-2.0-or-later AND (GPL-3.0-only OR MPL-2.0)\n'
            'URL: https://github.com/tiagoalam/Sonante\nBuildArch: x86_64\nAutoReqProv: no\n'
            'Conflicts: sonante-bin, sonante-private\nObsoletes: sonante-bin < ' + version + ', sonante-private < ' + version + '\nRecommends: xdg-desktop-portal-gtk\n'
            + ''.join('Requires: ' + value + '\n' for value in sorted(requires))
            + '%global debug_package %{nil}\n%global __os_install_post %{nil}\nSource0: payload.tar\n'
            '%description\nLocal and Plex music player with pinned Fedora audio runtimes.\n'
            '%prep\n%build\n%install\nmkdir -p %{buildroot}\ntar -xf %{SOURCE0} -C %{buildroot}\n'
            '%files\n%defattr(-,root,root,-)\n' + ''.join('/' + name + '\n' for name in files))
    recipe = work / 'sonante.spec'
    recipe.write_text(text)
    top = work / 'rpm-build'
    top.mkdir()
    command = ['rpmbuild']
    for key, value in {'_topdir': top, '_sourcedir': work, '_buildhost': 'sonante-build',
                       '_build_id_links': 'none', '_binary_payload': 'w6.xzdio', '_smp_build_ncpus': '2',
                       '_binary_filedigest_algorithm': '8'}.items():
        command += ['--define', key + ' ' + str(value)]
    subprocess.run(command + ['-bb', str(recipe)], env={**os.environ, 'SOURCE_DATE_EPOCH': str(epoch)}, check=True)
    candidates = list((top / 'RPMS').rglob('*.rpm'))
    require(len(candidates) == 1, 'Unexpected RPM outputs')
    shutil.copyfile(candidates[0], output)
    require(run('rpm', '-qp', '--queryformat', '%{NAME}\n%{VERSION}-%{RELEASE}\n%{ARCH}\n', output)
            == 'sonante\n' + version + '-' + revision + '\nx86_64\n', 'Encoded RPM identity')
    require(not run('rpm', '-qp', '--scripts', output).strip(), 'RPM scriptlets forbidden')
    require(not any('.so' in line.split()[0] for line in run('rpm', '-qp', '--provides', output).splitlines() if line), 'Private RPM SONAME Provides forbidden')
    require(requires <= set(run('rpm', '-qp', '--requires', output).splitlines()), 'RPM requirements changed')


def verify_rpm_header(package, expected):
    query = '[%{FILENAMES}\t%{FILESIZES}\t%{FILEMODES}\t%{FILEUSERNAME}\t%{FILEGROUPNAME}\t%{FILEDIGESTS}\n]'
    rows = run('rpm', '-qp', '--queryformat', query, package)
    actual = {}
    for line in rows.splitlines():
        name, size, mode, user, group, sha = line.split('\t')
        require(name.startswith('/') and user == group == 'root' and stat.S_ISREG(int(mode)), 'Unsafe RPM header file')
        name = safe_name(name[1:])
        require(name not in actual, 'Duplicate RPM header file')
        actual[name] = {'sha256': sha, 'bytes': int(size), 'mode': int(mode) & 0o7777}
    require(actual == expected, 'RPM header/payload digest mismatch')


def arch_info(previous, version, revision, glibc, size, epoch):
    allowed = {'pkgname', 'pkgbase', 'xdata', 'url', 'arch', 'license', 'replaces', 'conflict', 'depend', 'optdepend', 'provides'}
    lines = []
    for line in previous.splitlines():
        key, sep, value = line.partition(' = ')
        if sep and key in allowed:
            if key == 'depend' and value.startswith('glibc>='):
                value = 'glibc>=' + glibc
            lines.append(key + ' = ' + value)
    require({'pkgname = sonante', 'arch = x86_64', 'depend = glibc>=' + glibc,
             'depend = webkit2gtk-4.1>=2.40', 'depend = pipewire'} <= set(lines), 'Arch required metadata')
    require('depend = wireplumber' not in lines, 'Required WirePlumber is not supported')
    lines += ['pkgver = ' + version + '-' + revision, 'pkgdesc = Local and Plex music player with pinned audio runtimes',
              'builddate = ' + str(epoch), 'packager = Tiago Alam', 'size = ' + str(size)]
    return '\n'.join(lines) + '\n'


def build_arch(previous, stage, output, work, version, revision, glibc, epoch):
    files = sorted(str(p.relative_to(stage)) for p in stage.rglob('*') if p.is_file())
    (stage / '.PKGINFO').write_text(arch_info(previous, version, revision, glibc,
                                          sum((stage / name).stat().st_size for name in files), epoch))
    (stage / '.PKGINFO').chmod(0o644)
    for path in sorted(stage.rglob('*'), key=lambda p: len(p.parts), reverse=True):
        os.utime(path, (epoch, epoch))
    mtree = subprocess.check_output(['bsdtar', '--uid', '0', '--gid', '0', '-cf', '-', '--format=mtree',
                                     '--options=!all,use-set,type,uid,gid,mode,time,size,md5,sha256,link',
                                     '-C', str(stage), '.PKGINFO', 'usr'])
    (stage / '.MTREE').write_bytes(gzip.compress(mtree, mtime=0))
    (stage / '.MTREE').chmod(0o644)
    payload = work / 'arch-payload.tar'
    write_tar(payload, stage, files + ['.PKGINFO', '.MTREE'], epoch, include_dirs=True)
    with output.open('xb') as stream:
        subprocess.run(['zstd', '-q', '-T2', '-10', '-c', str(payload)], stdout=stream, check=True)


def source_identity():
    def git(*args):
        return subprocess.check_output(['git', *args], cwd=ROOT, text=True).strip()
    require(not git('status', '--porcelain=v1', '--untracked-files=normal'), 'Complete packages require a clean source checkout')
    version = json.loads((ROOT / 'package.json').read_text())['version']
    require(re.fullmatch(r'\d+\.\d+\.\d+', version), 'Unsupported package version')
    return {'commit': git('rev-parse', 'HEAD'), 'tree': git('rev-parse', 'HEAD^{tree}'),
            'epoch': int(git('show', '-s', '--format=%ct', 'HEAD')), 'version': version}


def source_archive(output, identity, revision, descriptor=None):
    suffix = '-fedora' if descriptor else ''
    name = 'sonante-' + identity['version'] + '-' + revision + suffix + '-sources.tar.gz'
    count = 0
    with (output / name).open('xb') as target, gzip.GzipFile(filename='', mode='wb', fileobj=target, mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode='w|') as result, archive_stream(['git', '-C', str(ROOT), 'archive', '--format=tar', identity['commit']]) as stream:
            with tarfile.open(fileobj=stream, mode='r|') as source:
                for item in source:
                    path = safe_name(item.name)
                    top = path.split('/')[0]
                    require(top not in {'AGENTS.md', 'AGENTS.override.md', 'MAINTAINER_AGENTS.md', 'PROJECT_STATE.md', 'LEIA-ME-PRIMEIRO.md',
                                        'desenvolvimento', 'pacotes', '.cargo', '.codex', '.agents', '.aws',
                                        'config.json', 'favorites.json', 'playlists.json', 'queue_cache.json', 'mpd.conf', 'mpd-v2.db', 'runtime'}
                            and not top.startswith('PROJECT_HISTORY_')
                            and (not top.startswith('.env') or top == '.env.example'), 'Private source input forbidden')
                    if item.isdir():
                        continue
                    require(item.isfile() and item.size <= MAX_FILE, 'Unsupported source entry')
                    data = descriptor if descriptor and path == 'packaging/private-runtimes.json' else source.extractfile(item).read()
                    entry = tarfile.TarInfo('sonante-' + identity['version'] + '-' + revision + '/' + path)
                    entry.uid = entry.gid = 0
                    entry.uname = entry.gname = 'root'
                    entry.mode, entry.mtime, entry.size = item.mode, identity['epoch'], len(data)
                    result.addfile(entry, io.BytesIO(data))
                    count += 1
    require(count > 0, 'Empty source archive')
    return name


def assemble(variant, inputs, candidates, work, output, identity, revision):
    entry = inputs['packages'][variant]
    base = checked_file(work.parent / 'inputs' / entry['filename'], entry)
    task = work / variant
    task.mkdir(mode=0o755)
    stage = task / 'stage'
    stage.mkdir(mode=0o755)
    if variant == 'ubuntu':
        verify_deb_controls(base)
    files, metadata = inspect_package(base, variant, stage)
    require(len(files) == entry['payload_files'] and 'usr/bin/sonante' in files, 'Baseline payload inventory changed')
    if variant == 'arch':
        require(metadata.keys() == {'.PKGINFO', '.MTREE'}, 'Arch installer metadata/scripts changed')
    for directory in stage.rglob('*'):
        if directory.is_dir():
            directory.chmod(0o755)
    descriptor_path = safe_name(entry['descriptor'])
    require(descriptor_path.startswith('packaging/'), 'Invalid descriptor source path')
    descriptor = checked_file(ROOT / descriptor_path).read_bytes()
    require(hashlib.sha256(descriptor).hexdigest() == entry['descriptor_sha256'], 'Runtime descriptor pin changed')
    verify_runtime(stage, descriptor)
    inventory = json.loads((stage / 'usr/share/doc/sonante/runtime-inventory.json').read_text())
    require(inventory['distribution_ready'] and not inventory['source_trees_installed'], 'Runtime distribution record is incomplete')
    sidecars = [inventory['source_sidecar'], *inventory.get('additional_source_sidecars', [])]
    for source in sidecars:
        require(any(all(value[key] == source[key] for key in ['filename', 'sha256', 'bytes']) for value in inputs['sources']), 'Runtime corresponding-source identity missing')
    _, previous_needed = elf_info(stage / 'usr/bin/sonante', variant == 'arch')
    directory = candidates / variant
    app = task / 'sonante'
    if variant == 'fedora':
        names = verify_checksums(directory, 'SHA256SUMS.txt')
        require({'sonante', 'source-commit.txt', 'manifest.json'} <= names, 'Fedora candidate provenance missing')
        require((directory / 'source-commit.txt').read_text().strip() == identity['commit'], 'Foreign Fedora source commit')
        require((directory / 'manifest.json').read_bytes() == descriptor, 'Wrong Fedora candidate descriptor')
        shutil.copyfile(checked_file(directory / 'sonante'), app)
    else:
        names = verify_checksums(directory, 'SHA256SUMS-' + variant + '.txt')
        packages = list(directory.glob('*.deb' if variant == 'ubuntu' else '*.pkg.tar.zst'))
        require(len(packages) == 1 and packages[0].name in names, 'Ambiguous/unverified CI candidate')
        require('source-commit.txt' in names and (directory / 'source-commit.txt').read_text().strip() == identity['commit'], 'Foreign CI source commit')
        if variant == 'ubuntu':
            require(run('dpkg-deb', '-f', packages[0], 'Version').strip() == identity['version'], 'Wrong Ubuntu candidate version')
        else:
            require('pkgver = ' + identity['version'] + '-1\n' in run('bsdtar', '-xOf', packages[0], '.PKGINFO'), 'Wrong Arch candidate version')
        extract_application(packages[0], variant, app)
    require(descriptor in checked_file(app).read_bytes(), 'Application compiled descriptor mismatch')
    _, needed = elf_info(app, variant == 'arch')
    require(bool(needed), 'Application has no dynamic library requirements')
    require(needed <= previous_needed, 'New application library requires explicit dependency review')
    shutil.copyfile(app, stage / 'usr/bin/sonante')
    (stage / 'usr/bin/sonante').chmod(0o755)
    updates = {'usr/share/licenses/sonante/LICENSE': 'LICENSE', 'usr/share/doc/sonante/AUTHORS': 'AUTHORS',
               'usr/share/doc/sonante/THIRD_PARTY_NOTICES': 'THIRD_PARTY_NOTICES',
               'usr/share/applications/sonante.desktop': 'sonante-arch/sonante.desktop',
               **{'usr/share/icons/hicolor/' + size + '/apps/sonante.png': 'src-tauri/icons/' + icon
                  for size, icon in [('32x32', '32x32.png'), ('128x128', '128x128.png'), ('256x256', '128x128@2x.png'), ('512x512', 'icon.png')]}}
    for name, source in updates.items():
        require(name in files, 'Missing application presentation/license file')
        shutil.copyfile(checked_file(ROOT / source), stage / name)
        (stage / name).chmod(0o644)
    elf = {}
    for name in files:
        path = stage / name
        with path.open('rb') as stream:
            magic = stream.read(4)
        if magic == b'\x7fELF':
            elf[name] = elf_info(path, variant == 'arch' and name == 'usr/bin/sonante')[0]
    glibc = max((value.get('GLIBC', '0') for value in elf.values()), key=lambda s: tuple(map(int, s.split('.'))))
    receipt = {'schema': 1, 'version': identity['version'], 'revision': revision, 'variant': variant,
               'source_commit': identity['commit'], 'source_tree': identity['tree'],
               'base_installer_sha256': entry['sha256'], 'application_sha256': digest(app),
               'compiled_descriptor_sha256': entry['descriptor_sha256'], 'runtime_pins_unchanged': True,
               'minimum_glibc': glibc, 'ELF_files_checked': len(elf),
               'sources': ['sonante-' + identity['version'] + '-' + revision + ('-fedora' if variant == 'fedora' else '') + '-sources.tar.gz']
                          + [v['filename'] for v in inputs['sources']],
               'validation': 'Encoded payload, dependency, descriptor, runtime hash and ELF ABI checks; no installation or hardware execution.'}
    (stage / 'usr/share/doc/sonante/RELEASE-BUILD.json').write_text(json.dumps(receipt, indent=2) + '\n')
    (stage / 'usr/share/doc/sonante/RELEASE-BUILD.json').chmod(0o644)
    # Every immutable runtime/license must survive byte-for-byte, with no extras.
    mutable = set(updates) | {'usr/bin/sonante', 'usr/share/doc/sonante/RELEASE-BUILD.json'}
    expected = {}
    require({str(p.relative_to(stage)) for p in stage.rglob('*') if p.is_file()} == files.keys(), 'Unexpected staged payload')
    for name, previous in files.items():
        path = stage / name
        current = {'sha256': digest(path), 'bytes': path.stat().st_size, 'mode': path.stat().st_mode & 0o7777}
        require(name in mutable or current == previous, 'Immutable runtime/license changed')
        expected[name] = current
    version, epoch = identity['version'], identity['epoch']
    name = ('sonante_' + version + '-' + revision + '_amd64.deb' if variant == 'ubuntu' else
            'sonante-' + version + '-' + revision + ('.x86_64.rpm' if variant == 'fedora' else '-x86_64.pkg.tar.zst'))
    package = output / name
    if variant == 'ubuntu':
        build_deb(base, stage, package, version, revision)
    elif variant == 'fedora':
        build_rpm(base, stage, package, task, version, revision, epoch)
        verify_rpm_header(package, expected)
    else:
        build_arch(metadata['.PKGINFO']['data'].decode(), stage, package, task, version, revision, glibc, epoch)
    actual, meta = inspect_package(package, variant)
    require(actual == expected, 'Final encoded payload differs from verified staging')
    if variant == 'arch':
        require(meta.keys() == {'.PKGINFO', '.MTREE'}, 'Final Arch metadata incomplete')
        for key in meta:
            require(meta[key]['data'] == (stage / key).read_bytes(), 'Encoded Arch metadata changed')
    receipt.update(package=name, package_sha256=digest(package), package_bytes=package.stat().st_size)
    return receipt


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--candidates', type=Path, required=True)
    parser.add_argument('--work-dir', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--revision', default='1')
    args = parser.parse_args()
    require(re.fullmatch('[1-9][0-9]{0,3}', args.revision), 'Invalid package revision')
    identity = source_identity()
    work, output = args.work_dir.resolve(), args.output.resolve()
    require(not work.exists() and not output.exists(), 'Use fresh work/output directories; prior attempts are preserved')
    require(not work.is_relative_to(ROOT) and not output.is_relative_to(ROOT), 'Build outside the source checkout')
    work.mkdir(parents=True, mode=0o755)
    output.mkdir(parents=True, mode=0o755)
    inputs = json.loads((ROOT / 'packaging/complete-packages.json').read_text())
    require(inputs['schema'] == 1 and set(inputs['packages']) == set(VARIANTS), 'Unknown complete-package inputs')
    directory = work / 'inputs'
    directory.mkdir(mode=0o755)
    for entry in [*inputs['packages'].values(), *inputs['sources']]:
        fetch_input(entry, directory)
    receipts = []
    tasks = work / 'build'
    tasks.mkdir(mode=0o755)
    for variant in VARIANTS:
        receipts.append(assemble(variant, inputs, args.candidates.resolve(), tasks, output, identity, args.revision))
    source_archive(output, identity, args.revision)
    source_archive(output, identity, args.revision, (ROOT / inputs['packages']['fedora']['descriptor']).read_bytes())
    for entry in inputs['sources']:
        shutil.copyfile(directory / entry['filename'], output / entry['filename'])
        checked_file(output / entry['filename'], entry)
    require(source_identity() == identity, 'Source checkout changed during assembly')
    (output / 'BUILD-RECEIPT.json').write_text(json.dumps({'source': identity, 'packages': receipts,
                                                       'installed_or_executed': False}, indent=2) + '\n')
    checks = ''.join(digest(path) + '  ' + path.name + '\n' for path in sorted(output.iterdir()) if path.is_file())
    (output / 'SHA256SUMS.txt').write_text(checks)
    print('Complete DEB, Fedora RPM and Arch packages verified, with corresponding sources and checksums.')


if __name__ == '__main__':
    main()
