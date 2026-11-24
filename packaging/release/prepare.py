#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Verify the four native packages, installer attestations and matching sources."""
import argparse
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib

from native_packages import add_packages, checksums, digest


def run(*args):
    subprocess.run(args, check=True)


def download(repo, run_id, name, dest):
    run('gh', 'run', 'download', str(run_id), '--repo', repo, '--name', name, '--dir', str(dest))


def gather(repo, run_id, revision, version, out):
    if not re.fullmatch(r'[0-9a-f]{40}', revision) or not re.fullmatch(r'[0-9]+\.[0-9]+\.[0-9]+', version):
        raise ValueError('Invalid release revision or version')
    out.mkdir(parents=True, exist_ok=False)
    expected = {}
    source_name = f'helloneedle-{version}-source-{revision}.tar.gz'
    platforms = [('windows', f'HelloNeedle-{version}-Windows-x64', '-Setup.exe'),
                 ('macos-universal', f'HelloNeedle-{version}-macOS-Universal', '.dmg')]
    with tempfile.TemporaryDirectory(dir=out.parent) as temp:
        scratch = Path(temp)
        for key, base, ext in platforms:
            pkg, proof = scratch / (key + '-installer'), scratch / (key + '-proof')
            download(repo, run_id, f'helloneedle-{key}-installer', pkg)
            download(repo, run_id, f'helloneedle-{key}-provenance', proof)
            sums, bundle, file = base + '-SHA256SUMS', base + '.sigstore.json', base + ext
            if (pkg / sums).read_bytes() != (proof / sums).read_bytes():
                raise ValueError(f'Checksum manifest differs from the signed copy for {file}')
            entries = checksums(pkg / sums)
            if entries.get(file) != digest(pkg / file) or source_name not in entries:
                raise ValueError(f'Manifest for {file} does not match this installer and source revision')
            for asset in (file, sums):
                run('gh', 'attestation', 'verify', str(pkg / asset), '--repo', repo, '--bundle', str(proof / bundle),
                    '--signer-workflow', repo + '/.github/workflows/helloneedle.yml', '--source-digest', revision)
            for name, sha in entries.items():
                if expected.setdefault(name, sha) != sha:
                    raise ValueError(f'Conflicting signed checksum for {name}')
            for path in (pkg / file, pkg / sums, proof / bundle):
                shutil.move(path, out / path.name)

        folders = ['helloneedle-corresponding-source', 'helloneedle-windows-runtime-sources',
                   'helloneedle-macos-x86_64-runtime-sources', 'helloneedle-macos-universal-arm64-runtime-sources']
        for artifact in folders:
            with tempfile.TemporaryDirectory(dir=scratch) as current:
                directory = Path(current)
                download(repo, run_id, artifact, directory)
                files = [p for p in directory.iterdir() if p.name != 'SHA256SUMS']
                if not files:
                    raise ValueError(f'Missing sources in {artifact}')
                for path in files:
                    if path.is_symlink() or not path.is_file() or path.stat().st_size == 0:
                        raise ValueError(f'Invalid source artifact {path.name}')
                    if digest(path) != expected.get(path.name):
                        raise ValueError(f'Source bytes do not match the signed installer manifest: {path.name}')
                    if (out / path.name).exists():
                        raise ValueError(f'Duplicate asset {path.name}')
                    shutil.move(path, out / path.name)
    absent = set(expected) - {p.name for p in out.iterdir()}
    if absent:
        raise ValueError(f'Missing attested assets: {sorted(absent)}')
    add_packages(repo, run_id, revision, version, out)
    # The release workflow signs this complete four-platform list.
    sums = ''.join(f'{digest(p)}  {p.name}\n' for p in sorted(out.iterdir()))
    (out / 'SHA256SUMS').write_text(sums, encoding='ascii')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', required=True)
    parser.add_argument('--run', type=int, required=True)
    parser.add_argument('--workflow-revision', required=True)
    parser.add_argument('--github-env', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    reported = json.loads(subprocess.check_output(['gh', 'api', f'repos/{args.repo}/actions/runs/{args.run}']))
    revision = reported['head_sha']
    if (not re.fullmatch(r'[0-9a-f]{40}', revision) or reported['head_repository']['full_name'] != args.repo
            or reported['event'] not in ('push', 'workflow_dispatch') or reported['conclusion'] != 'success'
            or reported['path'] != '.github/workflows/helloneedle.yml'):
        raise SystemExit('Select a successful native build from this repository.')
    run('git', 'merge-base', '--is-ancestor', revision, args.workflow_revision)
    manifest = subprocess.check_output(['git', 'show', revision + ':bridge/Cargo.toml'], text=True)
    version = tomllib.loads(manifest)['package']['version']
    gather(args.repo, args.run, revision, version, args.out)
    with args.github_env.open('a', encoding='utf-8') as env:
        print(f'SOURCE_REVISION={revision}', f'VERSION={version}', sep='\n', file=env)
