#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check the two installers, their GitHub provenance, and every corresponding-source file."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import tempfile


def run(*args):
    subprocess.run(args, check=True)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def download(repo, run_id, name, dest):
    run('gh', 'run', 'download', str(run_id), '--repo', repo, '--name', name, '--dir', str(dest))


def checksums(path):
    result = {}
    for line in path.read_text(encoding='ascii').splitlines():
        match = re.fullmatch(r'([a-f0-9]{64})  ([A-Za-z0-9][A-Za-z0-9_.+-]*)', line)
        if not match or match[2] in result:
            raise ValueError(f'Invalid checksum entry in {path.name}: {line}')
        result[match[2]] = match[1]
    return result


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
    # Include both installers, source parts, detached proofs and their signed manifests.
    sums = ''.join(f'{digest(p)}  {p.name}\n' for p in sorted(out.iterdir()))
    (out / 'SHA256SUMS').write_text(sums, encoding='ascii')


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', required=True)
    parser.add_argument('--run', type=int, required=True)
    parser.add_argument('--revision', required=True)
    parser.add_argument('--version', required=True)
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    reported = json.loads(subprocess.check_output(['gh', 'api', f'repos/{args.repo}/actions/runs/{args.run}']))
    if (reported['head_sha'] != args.revision or reported['head_repository']['full_name'] != args.repo
            or reported['event'] not in ('push', 'workflow_dispatch')
            or reported['path'] != '.github/workflows/helloneedle.yml'):
        raise SystemExit('The selected run must build this release revision in its own trusted native workflow.')
    gather(args.repo, args.run, args.revision, args.version, args.out)
