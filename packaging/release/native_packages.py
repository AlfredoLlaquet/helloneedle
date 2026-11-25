# SPDX-License-Identifier: GPL-3.0-or-later
"""Check complete FreeBSD and Ubuntu package artifacts without installing or executing them."""
from dataclasses import dataclass
from email.parser import Parser
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
import zipfile

@dataclass(frozen=True)
class Package:
    artifact: str
    filename: str
    jobs: tuple[str, ...]
    freebsd_release: str = ''

    def passed_required_jobs(self, successful):
        build, *install_checks = self.jobs
        if self.freebsd_release and build not in successful:
            # GitHub prefixes the reusable workflow's job name with its caller.
            build = f'{build} / Build, install and test on FreeBSD {self.freebsd_release} amd64'
        return build in successful and successful.issuperset(install_checks)


def package_targets(version):
    # The immutable 1.0.0 release distributed one FreeBSD binary under its original name.
    releases = ('15.1',) if version == '1.0.0' else ('15.1', '14.4')
    targets = []
    for release in releases:
        major = release.split('.')[0]
        filename = f'helloneedle-{version}-FreeBSD-{major}-amd64.pkg'
        if version == '1.0.0':
            filename = f'helloneedle-{version}.pkg'
        jobs = (f'FreeBSD {release} amd64, pkg and native X11',)
        if release == '14.4':
            jobs += ('Install the FreeBSD 14.x package on FreeBSD 14.5 amd64',)
        targets.append(Package(f'helloneedle-freebsd-{release}-amd64', filename, jobs, release))
    targets.append(Package('helloneedle-ubuntu-24.04-x64', f'helloneedle_{version}_amd64.deb',
                           ('Ubuntu 24.04 x64, Debian package and native X11',)))
    return targets


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def checksums(path, *, payload=False):
    entries = {}
    for line in path.read_text(encoding='utf-8' if payload else 'ascii').splitlines():
        match = re.fullmatch(r'([0-9a-f]{64})  (.+)', line)
        if not match:
            raise ValueError(f'Malformed checksum in {path.name}')
        name = match[2]
        if payload:
            parts = PurePosixPath(name).parts
            valid = parts and parts[0] == 'usr' and '..' not in parts and '/'.join(parts) == name
        else:
            valid = re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9_.+-]*', name)
        if not valid or name in entries:
            raise ValueError(f'Invalid or repeated checksum filename in {path.name}: {name}')
        entries[name] = match[1]
    if not entries:
        raise ValueError(f'Empty checksum list: {path.name}')
    return entries


def verify_payload(package, inventory, revision, version):
    target = next((t for t in package_targets(version) if t.filename == package.name), None)
    if target is None:
        raise ValueError(f'Unexpected release package filename: {package.name}')
    freebsd = bool(target.freebsd_release)
    expected = checksums(inventory, payload=True)
    prefix = 'usr/local' if freebsd else 'usr'
    executables = {f'{prefix}/bin/helloneedle', f'{prefix}/bin/helloneedle-worker'}
    record = f'{prefix}/share/doc/helloneedle/BUILD-ENVIRONMENT.json'
    records = {}
    found = set()
    command = ['zstd', '-dc', package] if freebsd else ['dpkg-deb', '--fsys-tarfile', package]
    # Materialize a TAR, never extract its contents or run package scripts.
    with tempfile.TemporaryFile(dir=package.parent) as stream:
        subprocess.run(command, stdout=stream, check=True)
        stream.seek(0)
        with tarfile.open(fileobj=stream, mode='r:') as archive:
            for member in archive:
                if member.isdir():
                    continue
                path = member.name.removeprefix('./')
                if freebsd:
                    path = path.removeprefix('/')
                if not member.isfile() or path in found:
                    raise ValueError(f'Unexpected package entry: {member.name}')
                found.add(path)
                with archive.extractfile(member) as data:
                    if freebsd and path in ('+MANIFEST', '+COMPACT_MANIFEST'):
                        records[path] = json.load(data)
                        continue
                    if path not in expected:
                        raise ValueError(f'Unlisted payload: {path}')
                    if path in executables:
                        header = data.read(20)
                        if (header[:7] != b'\x7fELF\x02\x01\x01' or header[18:20] != b'\x3e\x00' or
                                header[7] not in ((9,) if freebsd else (0, 3))):
                            raise ValueError(f'Unexpected amd64 ELF platform: {path}')
                        data.seek(0)
                    if path == record:
                        records[record] = json.load(data)
                        data.seek(0)
                    if hashlib.file_digest(data, 'sha256').hexdigest() != expected[path]:
                        raise ValueError(f'Installed file hash differs: {path}')
    metadata_files = {'+MANIFEST', '+COMPACT_MANIFEST'} if freebsd else set()
    if found != expected.keys() | metadata_files or not executables.issubset(found) or record not in records:
        raise ValueError(f'Incomplete package payload or inventory: {package.name}')
    receipt = records[record]
    source = f'helloneedle-{version}-source-{revision}.tar.gz'
    if (receipt['commit'], receipt['version'], receipt['corresponding_source']) != (revision, version, source):
        raise ValueError(f'Embedded build revision, source or version differs in {package.name}')
    if freebsd:
        validate_freebsd(records, receipt, expected, version, target.freebsd_release)
    else:
        validate_debian(package, receipt, version)


def validate_freebsd(records, receipt, files, version, release):
    compact, full = records['+COMPACT_MANIFEST'], records['+MANIFEST']
    major = release.split('.')[0]
    fields = {'name': 'helloneedle', 'origin': 'deskutils/helloneedle', 'prefix': '/usr/local', 'version': version,
              'abi': f'FreeBSD:{major}:amd64', 'arch': f'freebsd:{major}:x86:64'}
    for info in (compact, full):
        if any(info.get(k) != value for k, value in fields.items()):
            raise ValueError('Unexpected FreeBSD package name, version, origin, ABI or architecture')
    kernel_and_userland = receipt['operating_system'].splitlines()
    release_pattern = re.escape(release) + r'-RELEASE(?:-p\d+)?'
    if (receipt['machine'] != 'amd64' or not kernel_and_userland
            or any(not re.fullmatch(release_pattern, v) for v in kernel_and_userland)
            or set(full['files']) != {'/' + k for k in files} or compact['deps'] != full['deps']):
        raise ValueError('FreeBSD build or package manifests disagree')
    origins = {'qt6-base': 'devel/qt6-base', 'qt6-translations': 'devel/qt6-translations',
               'poppler-utils': 'graphics/poppler-utils', 'antiword': 'textproc/antiword'}
    # The first release predates the SVG button artwork. Check the libraries
    # actually linked by the build whose package is being verified.
    if 'libQt6Svg.so' in receipt['runtime_linkage']['helloneedle']:
        origins['qt6-svg'] = 'graphics/qt6-svg'
    installed = dict(line.split(' ', 1) for line in receipt['managed_packages'])
    for name, origin in origins.items():
        dep = compact['deps'].get(name, {})
        if not installed.get(name) or (dep.get('origin'), dep.get('version')) != (origin, installed[name]):
            raise ValueError(f'FreeBSD dependency differs from build inventory: {name}')


def validate_debian(package, receipt, version):
    fields = Parser().parsestr(subprocess.check_output(['dpkg-deb', '--field', package], text=True))
    if [fields.get(k) for k in ('Package', 'Version', 'Architecture')] != ['helloneedle', version, 'amd64']:
        raise ValueError('Unexpected Debian package name, version or architecture')
    if receipt['machine'] != 'x86_64' or not re.match(r'^Ubuntu 24\.04(?:\.| )', receipt['operating_system']):
        raise ValueError('Expected an Ubuntu 24.04 x86_64 build')
    managed = {line.split()[0].split(':')[0] for line in receipt['managed_packages']}
    tools = {'qt6-qpa-plugins', 'qt6-translations-l10n', 'poppler-utils', 'antiword'}
    required = tools | {'libc6', 'libqt6core6t64', 'libqt6gui6t64', 'libqt6widgets6t64'}
    if 'libQt6Svg.so' in receipt['runtime_linkage']['helloneedle']:
        required.add('libqt6svg6')
        if 'qt6-svg-dev' not in managed:
            raise ValueError('Missing Qt SVG build inventory')
    dependencies = {dep.strip().split()[0] for dep in fields.get('Depends', '').split(',') if dep.strip()}
    if not required.issubset(dependencies) or not tools.issubset(managed):
        raise ValueError('Missing Ubuntu Qt or document runtime dependencies')


def add_packages(repo, run_id, revision, version, dest):
    endpoint = f'repos/{repo}/actions/runs/{run_id}'
    def pages(suffix, key):
        result = subprocess.check_output(['gh', 'api', '--paginate', '--slurp', endpoint + suffix])
        return [v for page in json.loads(result) for v in page[key]]
    successful = {j['name'] for j in pages('/jobs?filter=latest&per_page=100', 'jobs') if j['conclusion'] == 'success'}
    artifacts = pages('/artifacts?per_page=100', 'artifacts')
    for target in package_targets(version):
        filename = target.filename
        matches = [a for a in artifacts if a['name'] == target.artifact]
        if not target.passed_required_jobs(successful) or len(matches) != 1 or matches[0]['expired']:
            raise ValueError(f'Missing successful native test or unique live artifact for {target.artifact}')
        item = matches[0]
        with tempfile.TemporaryDirectory(dir=dest.parent) as temp:
            folder = Path(temp)
            wrapper = folder / 'download.zip'
            with wrapper.open('wb') as out:
                subprocess.run(['gh', 'api', f'repos/{repo}/actions/artifacts/{int(item["id"])}/zip'], stdout=out, check=True)
            if item.get('digest') != 'sha256:' + digest(wrapper):
                raise ValueError(f'Downloaded Actions archive differs from GitHub digest: {target.artifact}')
            with zipfile.ZipFile(wrapper) as archive:
                members = archive.infolist()
                wanted = {filename, 'SHA256SUMS', 'CONTENTS.sha256'}
                if len(members) != len(wanted) or {m.filename for m in members} != wanted:
                    raise ValueError(f'Unexpected files in native artifact: {target.artifact}')
                for member in members:
                    if member.is_dir() or stat.S_ISLNK(member.external_attr >> 16):
                        raise ValueError(f'Unexpected link in native artifact: {member.filename}')
                    with archive.open(member) as src, (folder / member.filename).open('wb') as out:
                        shutil.copyfileobj(src, out)
            package, inventory = folder / filename, folder / 'CONTENTS.sha256'
            if checksums(folder / 'SHA256SUMS') != {filename: digest(package), inventory.name: digest(inventory)}:
                raise ValueError(f'Package or payload inventory checksum differs: {target.artifact}')
            verify_payload(package, inventory, revision, version)
            shutil.move(package, dest / filename)
