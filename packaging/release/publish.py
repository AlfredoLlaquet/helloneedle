#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Stage a verified draft or publish it, keeping the original application tag and installer receipts."""
import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import time

from native_packages import digest


def gh(*args):
    return subprocess.check_output(['gh', *args], text=True)


def pages(endpoint):
    return [row for page in json.loads(gh('api', '--paginate', '--slurp', endpoint)) for row in page]


def unfinished(asset):
    return asset.get('state') == 'starter' and asset.get('size') == 0 and not asset.get('digest')


class Release:
    def __init__(self):
        self.repo = os.environ['GH_REPO']
        self.workflow = os.environ['GITHUB_SHA']
        self.default = os.environ['DEFAULT_BRANCH']
        self.source = os.environ['SOURCE_REVISION']
        self.version = os.environ['VERSION']
        self.tag = 'v' + self.version
        self.publish = os.environ['PUBLISH'] == 'true'
        self.dist = Path('dist')
        self.sums = self.dist / 'SHA256SUMS'
        self.receipt = self.dist / f'HelloNeedle-{self.version}-Release.sigstore.json'
        self.api = f'repos/{self.repo}/releases'

    def state(self):
        refs = subprocess.check_output(['git', 'ls-remote', 'origin', f'refs/tags/{self.tag}', f'refs/tags/{self.tag}^{{}}'], text=True)
        tags = {row.split()[1]: row.split()[0] for row in refs.splitlines()}
        tag = 'refs/tags/' + self.tag
        target = tags.get(tag + '^{}', tags.get(tag))
        if target is not None and target != self.source:
            raise ValueError(f'{self.tag} already points to another source revision')
        matches = [r for r in pages(self.api + '?per_page=100') if r['tag_name'] == self.tag]
        if not matches:
            return None, {}
        if len(matches) != 1:
            raise ValueError(f'Multiple releases match {self.tag}')
        record = matches[0]
        if record['draft'] and record['target_commitish'] != self.source:
            raise ValueError('This draft points to another source revision')
        if not record['draft'] and target != self.source:
            raise ValueError('The published tag must resolve to the tested source revision')
        items = pages(f'{self.api}/{record["id"]}/assets?per_page=100')
        assets = {a['name']: a for a in items}
        if len(assets) != len(items):
            raise ValueError('Duplicate release asset names')
        return record, assets

    def local_hashes(self):
        return {p.name: 'sha256:' + digest(p) for p in self.dist.iterdir() if p.is_file()}

    def check_existing(self, record, assets):
        local = self.local_hashes()
        for name, value in assets.items():
            if name in (self.sums.name, self.receipt.name):
                continue
            if record['draft'] and name in local and unfinished(value):
                continue
            if name not in local or local[name] != value.get('digest'):
                raise ValueError(f'Existing release asset differs from tested bytes: {name}')
        if record and not record['draft'] and (set(local) | {self.receipt.name}) != set(assets):
            raise ValueError('Published release contents differ from the complete four-platform set')

    def verify_receipt(self, receipt, *, new=False):
        flags = ['attestation', 'verify', str(self.sums), '--repo', self.repo, '--bundle', str(receipt),
                 '--signer-workflow', self.repo + '/.github/workflows/installers-release.yml',
                 '--source-ref', 'refs/heads/' + self.default, '--deny-self-hosted-runners', '--format', 'json']
        if new:
            flags.extend(['--source-digest', self.workflow])
        results = json.loads(gh(*flags))
        for item in results:
            commit = item['verificationResult']['signature']['certificate']['sourceRepositoryDigest']
            if re.fullmatch('[0-9a-f]{40}', commit) and all(subprocess.run(
                    ['git', 'merge-base', '--is-ancestor', a, b]).returncode == 0
                    for a, b in ((self.source, commit), (commit, self.workflow))):
                return
        raise ValueError('The receipt must come from the trusted release workflow and its default-branch history')

    def plan(self):
        record, assets = self.state()
        self.check_existing(record, assets)
        sign = True
        previous = assets.get(self.receipt.name)
        if previous and not unfinished(previous) and assets.get(self.sums.name, {}).get('digest') == 'sha256:' + digest(self.sums):
            with self.receipt.open('wb') as out:
                subprocess.run(['gh', 'api', '-H', 'Accept: application/octet-stream',
                                f'{self.api}/assets/{int(previous["id"])}'], stdout=out, check=True)
            if 'sha256:' + digest(self.receipt) != previous['digest']:
                raise ValueError('Previously uploaded receipt differs from the release asset digest')
            try:
                self.verify_receipt(self.receipt)
                sign = False
            except (subprocess.CalledProcessError, ValueError):
                if not record['draft']:
                    raise
                # A draft can be interrupted between its two aggregate uploads.
                self.receipt.unlink()
        if record and not record['draft'] and sign:
            raise ValueError('The published release must already contain the correct verified manifest and receipt')
        with Path(os.environ['GITHUB_OUTPUT']).open('a', encoding='utf-8') as output:
            print('sign=' + str(sign).lower(), file=output)

    def finish(self):
        new = os.getenv('CHECKSUM_BUNDLE')
        if new:
            shutil.copyfile(new, self.receipt)
        self.verify_receipt(self.receipt, new=bool(new))
        subprocess.run(['sha256sum', '-c', self.sums.name], cwd=self.dist, check=True)
        record, assets = self.state()
        self.check_existing(record, assets)
        notes = (Path(__file__).parent / 'notes.md').read_text().replace('@VERSION@', self.version).replace('@REPOSITORY@', self.repo)
        with tempfile.TemporaryDirectory() as temp:
            notes_path = Path(temp) / 'notes.md'
            notes_path.write_text(notes, encoding='utf-8')
            if record is None:
                gh('release', 'create', self.tag, '--draft', '--target', self.source,
                   '--title', 'Hello Needle ' + self.version, '--notes-file', str(notes_path))
                record, assets = self.state()
                if record is None or not record['draft']:
                    raise ValueError('Expected a draft after release creation')
            local = self.local_hashes()
            if record['draft']:
                # Product files and per-installer proofs are append-only. Only a draft's
                # aggregate manifest and its freshly verified receipt can be replaced.
                names = sorted(set(local) - {self.sums.name, self.receipt.name}) + [self.sums.name, self.receipt.name]
                for name in names:
                    existing = assets.get(name)
                    if existing and existing['digest'] == local[name]:
                        continue
                    replace = existing and (unfinished(existing) or name in (self.sums.name, self.receipt.name))
                    flags = ['--clobber'] if replace else []
                    gh('release', 'upload', self.tag, str(self.dist / name), *flags)
                if (record.get('body') or '').splitlines() != notes.splitlines():
                    gh('release', 'edit', self.tag, '--notes-file', str(notes_path))
            record, assets = self.state()
            if {n: a['digest'] for n, a in assets.items()} != local:
                raise ValueError('Remote release assets differ from the verified local bytes')
            if record['draft'] and (record.get('body') or '').splitlines() != notes.splitlines():
                raise ValueError('Draft release notes differ from the four-platform instructions')
            if record['draft'] and self.publish:
                gh('release', 'edit', self.tag, '--draft=false', '--latest')
        record, assets = self.state()
        if self.publish and record['draft']:
            raise ValueError('The release is still a draft')
        if {n: a['digest'] for n, a in assets.items()} != self.local_hashes():
            raise ValueError('Release assets changed during publication')
        state = json.loads(gh('release', 'view', self.tag, '--json', 'url,isDraft,isImmutable'))
        print(json.dumps(state))
        if self.publish and (state['isDraft'] or not state['isImmutable']):
            raise ValueError('Publication must return an immutable public release.')
        if not state['isDraft'] and state['isImmutable']:
            for delay in (1, 3, 7, 15, 30, 30):
                if subprocess.run(['gh', 'release', 'verify', self.tag]).returncode == 0:
                    break
                time.sleep(delay)
            else:
                subprocess.run(['gh', 'release', 'verify', self.tag], check=True)
            for name in (f'HelloNeedle-{self.version}-Windows-x64-Setup.exe', f'HelloNeedle-{self.version}-macOS-Universal.dmg',
                         f'helloneedle-{self.version}.pkg', f'helloneedle_{self.version}_amd64.deb'):
                subprocess.run(['gh', 'release', 'verify-asset', self.tag, self.dist / name], check=True)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('action', choices=('plan', 'finish'))
    args = parser.parse_args()
    getattr(Release(), args.action)()
