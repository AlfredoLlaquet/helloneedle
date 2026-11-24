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

import publish


class ReleaseTransaction(unittest.TestCase):
    def setUp(self):
        work = self.enterContext(tempfile.TemporaryDirectory())
        self.enterContext(chdir(work))
        self.enterContext(patch.dict(os.environ, {'GH_REPO': 'example/test', 'DEFAULT_BRANCH': 'main', 'VERSION': '1.0.0',
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
        for name in ('program.exe', 'program.dmg', 'helloneedle-1.0.0.pkg', 'helloneedle_1.0.0_amd64.deb'):
            (self.flow.dist / name).write_bytes(name.encode())
        self.flow.sums.write_text(''.join(f'{publish.digest(p)}  {p.name}\n' for p in self.flow.dist.iterdir()))
        self.assets['program.exe'] = b'program.exe'
        self.assets['program.dmg'] = b'program.dmg'
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
        return f'{self.tag}\trefs/tags/v1.0.0\n' if self.tag else ''

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
        self.assertEqual({item for kind, item in self.operations if kind == 'add'}, {'helloneedle-1.0.0.pkg', 'helloneedle_1.0.0_amd64.deb'})
        self.assertIn('FreeBSD 15.1', self.body)
        self.flow.receipt.unlink()
        self.operations.clear()
        self.flow.plan()
        self.assertTrue(Path('output').read_text().endswith('sign=false\n'))
        self.flow.publish = True
        self.flow.finish()
        self.assertEqual([k for k, _ in self.operations if k != 'immutable'], ['publish'])
        self.assertEqual(len([k for k, _ in self.operations if k == 'immutable']), 4)
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
        self.assets['program.exe'] = b'other bytes'
        with self.assertRaisesRegex(ValueError, 'differs'): self.flow.plan()
        self.assertEqual(self.operations, [])


if __name__ == '__main__':
    unittest.main()
