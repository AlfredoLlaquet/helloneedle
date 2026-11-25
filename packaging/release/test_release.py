# SPDX-License-Identifier: GPL-3.0-or-later
"""Release transaction tests use a local in-memory GitHub substitute."""
from contextlib import chdir
import hashlib
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import native_packages
import publish


class ReleaseTransaction(unittest.TestCase):
    version = '1.0.0'
    packages = ('helloneedle-1.0.0.pkg', 'helloneedle_1.0.0_amd64.deb')

    def setUp(self):
        work = self.enterContext(tempfile.TemporaryDirectory())
        self.enterContext(chdir(work))
        self.enterContext(patch.dict(os.environ, {'GH_REPO': 'example/test', 'DEFAULT_BRANCH': 'main', 'VERSION': self.version,
            'SOURCE_REVISION': 'a' * 40, 'GITHUB_SHA': 'b' * 40, 'PUBLISH': 'false', 'GITHUB_OUTPUT': str(Path(work) / 'output'),
            'CHECKSUM_BUNDLE': ''}))
        self.flow = publish.Release()
        self.flow.dist.mkdir()
        self.assets = {}
        self.body = 'Previous two-platform release'
        self.draft = True
        self.immutable = True
        self.target = self.flow.source
        self.tag = None
        self.operations = []
        self.installers = (f'HelloNeedle-{self.version}-Windows-x64-Setup.exe',
                           f'HelloNeedle-{self.version}-macOS-Universal.dmg')
        for name in self.installers + self.packages:
            (self.flow.dist / name).write_bytes(name.encode())
        self.flow.sums.write_text(''.join(f'{publish.digest(p)}  {p.name}\n' for p in self.flow.dist.iterdir()))
        for name in self.installers:
            self.assets[name] = name.encode()
        self.assets['SHA256SUMS'] = b'previous two-platform checksum list'
        self.assets[self.flow.receipt.name] = b'previous two-platform receipt'
        self.enterContext(patch.object(publish, 'pages', side_effect=self.pages))
        self.enterContext(patch.object(publish.subprocess, 'check_output', side_effect=self.git))
        real_run = subprocess.run
        def run(cmd, **kwargs):
            if cmd[:2] == ['gh', 'api']:
                kwargs['stdout'].write(self.assets[self.flow.receipt.name])
                return subprocess.CompletedProcess(cmd, 0)
            if cmd[:3] == ['gh', 'release', 'verify']:
                return subprocess.CompletedProcess(cmd, 0)
            if cmd[:3] == ['gh', 'release', 'verify-asset']:
                self.operations.append(('immutable', Path(cmd[-1]).name))
                return subprocess.CompletedProcess(cmd, 0)
            return real_run(cmd, **kwargs)
        self.enterContext(patch.object(publish.subprocess, 'run', side_effect=run))
        self.enterContext(patch.object(publish, 'gh', side_effect=self.gh))
        self.enterContext(patch.object(self.flow, 'verify_receipt', side_effect=self.verify))

    def git(self, cmd, **_):
        if cmd[:2] != ['git', 'ls-remote']:
            raise AssertionError(cmd)
        return f'{self.tag}\trefs/tags/{self.flow.tag}\n' if self.tag else ''

    def pages(self, url):
        if '/assets' in url:
            return [{'id': i, 'name': n, 'digest': 'sha256:' + hashlib.sha256(b).hexdigest()}
                    for i, (n, b) in enumerate(self.assets.items(), 1)]
        return [{'id': 1, 'tag_name': self.flow.tag, 'target_commitish': self.target, 'draft': self.draft, 'body': self.body}]

    def gh(self, *cmd):
        if cmd[:2] == ('release', 'upload'):
            p = Path(cmd[3])
            self.operations.append(('replace' if '--clobber' in cmd else 'add', p.name))
            self.assets[p.name] = p.read_bytes()
        elif cmd[:2] == ('release', 'edit'):
            if '--notes-file' in cmd:
                self.body = Path(cmd[-1]).read_text()
                self.operations.append(('notes', self.flow.tag))
            elif '--draft=false' in cmd:
                self.draft, self.tag = False, self.flow.source
                self.operations.append(('publish', self.flow.tag))
        elif cmd[:2] == ('release', 'view'):
            return '{"isDraft":' + str(self.draft).lower() + ',"isImmutable":' + str(self.immutable).lower() + '}'
        else:
            raise AssertionError(cmd)
        return ''

    def verify(self, path, *, new=False):
        if path.read_text() != 'proof-' + publish.digest(self.flow.sums):
            raise ValueError('The receipt belongs to a different manifest')

    def test_stage_then_publish_and_verify_published_retry(self):
        self.flow.plan()
        self.assertEqual(Path('output').read_text(), 'sign=true\n')
        proof = Path('signed')
        proof.write_text('proof-' + publish.digest(self.flow.sums))
        with patch.dict(os.environ, CHECKSUM_BUNDLE=str(proof)):
            self.flow.finish()
        self.assertTrue(self.draft)
        self.assertEqual({item for kind, item in self.operations if kind == 'replace'}, {'SHA256SUMS', self.flow.receipt.name})
        self.assertEqual({item for kind, item in self.operations if kind == 'add'}, set(self.packages))
        for name in self.packages:
            self.assertIn(f'/releases/download/{self.flow.tag}/{name})', self.body)
        self.flow.receipt.unlink()
        self.operations.clear()
        self.flow.plan()
        self.assertTrue(Path('output').read_text().endswith('sign=false\n'))
        self.flow.publish = True
        self.flow.finish()
        self.assertEqual([k for k, _ in self.operations if k != 'immutable'], ['publish'])
        self.assertEqual({item for kind, item in self.operations if kind == 'immutable'},
                         set(self.installers + self.packages))
        self.operations.clear()
        self.flow.plan()
        self.flow.finish()
        self.assertEqual(set(k for k, _ in self.operations), {'immutable'})

    def test_reject_mutable_publication(self):
        self.immutable = False
        self.flow.publish = True
        proof = Path('signed')
        proof.write_text('proof-' + publish.digest(self.flow.sums))
        with patch.dict(os.environ, CHECKSUM_BUNDLE=str(proof)), self.assertRaisesRegex(ValueError, 'immutable public release'):
            self.flow.finish()

    def test_recover_interrupted_receipt_update(self):
        self.assets['SHA256SUMS'] = self.flow.sums.read_bytes()
        self.flow.plan()
        self.assertEqual(Path('output').read_text(), 'sign=true\n')
        self.assertFalse(self.flow.receipt.exists())

    def test_preserve_wrong_target_and_altered_existing_product(self):
        self.tag = 'c' * 40
        with self.assertRaisesRegex(ValueError, 'another source'): self.flow.plan()
        self.tag = None
        self.target = 'd' * 40
        with self.assertRaisesRegex(ValueError, 'another source'): self.flow.plan()
        self.target = self.flow.source
        self.assets[self.installers[0]] = b'other bytes'
        with self.assertRaisesRegex(ValueError, 'differs'): self.flow.plan()
        self.assertEqual(self.operations, [])


class FivePackageTransaction(ReleaseTransaction):
    version = '1.1.0'
    packages = ('helloneedle-1.1.0-FreeBSD-14-amd64.pkg', 'helloneedle-1.1.0-FreeBSD-15-amd64.pkg',
                'helloneedle_1.1.0_amd64.deb')


class FreeBSDTargets(unittest.TestCase):
    def test_reusable_job_name_preserves_each_release_check(self):
        for target in native_packages.package_targets('1.2.0'):
            build, *checks = target.jobs
            self.assertTrue(target.passed_required_jobs(set(target.jobs)))
            if not target.freebsd_release:
                self.assertFalse(target.passed_required_jobs({build + ' / other job'}))
                continue
            child = f'Build, install and test on FreeBSD {target.freebsd_release} amd64'
            qualified = f'{build} / {child}'
            self.assertTrue(target.passed_required_jobs({qualified, *checks}))
            self.assertFalse(target.passed_required_jobs({child, *checks}))
            self.assertFalse(target.passed_required_jobs({f'{build} / other job', *checks}))
            if checks:
                self.assertFalse(target.passed_required_jobs({qualified}))

    def test_build_receipt_and_both_pkg_manifests_match_the_target(self):
        version = '1.1.0'
        targets = [t for t in native_packages.package_targets(version) if t.freebsd_release]
        self.assertEqual({t.freebsd_release for t in targets}, {'14.4', '15.1'})
        deps = {name: {'origin': origin, 'version': '1'} for name, origin in (
            ('qt6-base', 'devel/qt6-base'), ('qt6-svg', 'graphics/qt6-svg'),
            ('qt6-translations', 'devel/qt6-translations'), ('poppler-utils', 'graphics/poppler-utils'),
            ('antiword', 'textproc/antiword'))}
        files = {'usr/local/bin/helloneedle': 'digest'}
        for target in targets:
            release = target.freebsd_release
            major = release.split('.')[0]
            with self.subTest(release=release):
                self.assertEqual(target.filename, f'helloneedle-{version}-FreeBSD-{major}-amd64.pkg')
                self.assertIn(f'FreeBSD {release} amd64, pkg and native X11', target.jobs)
                if major == '14':
                    self.assertIn('Install the FreeBSD 14.x package on FreeBSD 14.5 amd64', target.jobs)
                manifest = {'name': 'helloneedle', 'origin': 'deskutils/helloneedle', 'prefix': '/usr/local',
                            'version': version, 'abi': f'FreeBSD:{major}:amd64', 'arch': f'freebsd:{major}:x86:64',
                            'deps': deps}
                records = {'+COMPACT_MANIFEST': manifest.copy(),
                           '+MANIFEST': dict(manifest, files={'/' + path: digest for path, digest in files.items()})}
                receipt = {'machine': 'amd64', 'operating_system': f'{release}-RELEASE\n{release}-RELEASE-p2',
                           'managed_packages': [name + ' 1' for name in deps],
                           'runtime_linkage': {'helloneedle': 'libQt6Svg.so.6'}}
                native_packages.validate_freebsd(records, receipt, files, version, release)
                other = '15.1' if major == '14' else '14.4'
                with self.assertRaisesRegex(ValueError, 'ABI'):
                    native_packages.validate_freebsd(records, receipt, files, version, other)
                for metadata in records.values():
                    with patch.dict(metadata, abi=f'FreeBSD:{other.split(".")[0]}:amd64'):
                        with self.assertRaisesRegex(ValueError, 'ABI'):
                            native_packages.validate_freebsd(records, receipt, files, version, release)
                with patch.dict(receipt, operating_system=f'{release}-RELEASE\n{other}-RELEASE'):
                    with self.assertRaisesRegex(ValueError, 'build or package manifests'):
                        native_packages.validate_freebsd(records, receipt, files, version, release)


if __name__ == '__main__':
    unittest.main()
