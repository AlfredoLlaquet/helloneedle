#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Collect corresponding-source archives and produce portable installer checksum lists."""
import argparse
import hashlib
from pathlib import Path
import re
import tarfile


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def read_checksums(path):
    entries = {}
    for line in path.read_text(encoding="ascii").splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([A-Za-z0-9][A-Za-z0-9_.+-]*)", line)
        if not match or match[2] in entries:
            raise SystemExit(f"Malformed or duplicate filename in {path}")
        entries[match[2]] = match[1]
    return entries


def write_checksums(path, entries):
    lines = (f"{entries[name]}  {name}\n" for name in sorted(entries))
    path.write_bytes("".join(lines).encode("ascii"))


class ArchiveParts:
    """Stream a TAR once, staying below GitHub's 2 GiB release asset limit."""

    def __init__(self, output, limit=1800 * 1024 * 1024):
        self.output = output
        self.limit = limit
        self.paths = [output]
        self.written = 0
        self.file = output.open("xb")

    def write(self, data):
        remaining = memoryview(data)
        while remaining:
            if self.written == self.limit:
                self.file.close()
                if len(self.paths) == 1:
                    first = self.output.with_name(self.output.name + ".part001")
                    self.output.rename(first)
                    self.paths[0] = first
                path = self.output.with_name(f"{self.output.name}.part{len(self.paths) + 1:03d}")
                self.paths.append(path)
                self.file = path.open("xb")
                self.written = 0
            count = min(len(remaining), self.limit - self.written)
            self.file.write(remaining[:count])
            self.written += count
            remaining = remaining[count:]
        return len(data)


def package_sources(args):
    folder, output = args.source.resolve(), args.output.resolve()
    if not (folder / "RUNTIME-SOURCES.json").is_file() or not (folder / "SHA256SUMS").is_file():
        raise SystemExit("Missing runtime source inventory or checksums.")
    if output.is_relative_to(folder):
        raise SystemExit("Choose an output outside the source collection.")
    output.parent.mkdir(parents=True, exist_ok=True)
    parts = ArchiveParts(output)
    try:
        with tarfile.open(fileobj=parts, mode="w|") as archive:
            archive.add(folder, arcname=output.stem)
    finally:
        parts.file.close()
    write_checksums(output.parent / "SHA256SUMS", {p.name: sha256(p) for p in parts.paths})


def installer_manifest(args):
    installer = args.installer
    checked = {installer.name: sha256(installer)}
    for path in args.checksums:
        for name, digest in read_checksums(path).items():
            # A universal DMG uses the sources listed with both original slices.
            if name.endswith((".dmg", ".exe")) and name != installer.name:
                continue
            if name in checked and checked[name] != digest:
                raise SystemExit(f"Conflicting digests for {name}")
            checked[name] = digest
    if not any("-source-" in n for n in checked) or not any("Runtime-Sources" in n for n in checked):
        raise SystemExit("Installer manifests must cover the project and its bundled runtime sources.")
    write_checksums(args.output, checked)


def verify_installer(args):
    if read_checksums(args.checksums).get(args.installer.name) != sha256(args.installer):
        raise SystemExit(f"Installer checksum differs: {args.installer.name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(required=True)
    sources = commands.add_parser("sources")
    sources.add_argument("--source", type=Path, required=True)
    sources.add_argument("--output", type=Path, required=True)
    sources.set_defaults(run=package_sources)
    manifest = commands.add_parser("manifest")
    manifest.add_argument("--installer", type=Path, required=True)
    manifest.add_argument("--output", type=Path, required=True)
    manifest.add_argument("--checksums", type=Path, nargs="+", required=True)
    manifest.set_defaults(run=installer_manifest)
    verify = commands.add_parser("verify")
    verify.add_argument("--installer", type=Path, required=True)
    verify.add_argument("--checksums", type=Path, required=True)
    verify.set_defaults(run=verify_installer)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
