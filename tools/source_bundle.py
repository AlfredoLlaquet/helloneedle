#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Collect the locked Rust dependency notices or make a rebuildable source archive."""

import argparse
import gzip
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
from tempfile import TemporaryDirectory

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "bridge" / "Cargo.toml"


def invoke(args, **kwargs):
    return subprocess.run(args, cwd=ROOT, check=True, **kwargs)


def run_text(*args):
    return invoke(args, stdout=subprocess.PIPE, text=True, encoding="utf-8").stdout.strip()


def notice_files(package):
    directory = Path(package["manifest_path"]).parent
    names = {"license", "licence", "licenses", "licences", "copying", "copyright", "unlicense", "notice", "patents"}
    selected = set()
    if package["license_file"]:
        selected.add(directory / package["license_file"])
    for path in directory.rglob("*"):
        if path.is_dir():
            continue
        relative = path.relative_to(directory)
        # Several crates put individual transitive data notices in a licenses directory.
        lower = path.name.lower()
        if (any(lower == name or lower.startswith((name + ".", name + "-", name + "_")) for name in names)
                or any(part.lower() in ("license", "licenses", "licences") for part in relative.parts[:-1])):
            selected.add(path)
    for path in sorted(selected):
        if path.is_file() and path.resolve().is_relative_to(directory.resolve()):
            yield path.relative_to(directory), path


# These exact crate archives omit standalone license texts. Their manifests and
# source headers declare the licenses below. Keep the upstream evidence alongside
# the standard text; a version change requires reviewing the new crate archive.
ADDITIONAL_NOTICES = {
    "fxhash-0.2.1": ("Apache-2.0", ("Cargo.toml", "lib.rs")),
    "mac-0.1.1": ("Apache-2.0", ("Cargo.toml", "README.md")),
    "match_token-0.35.0": ("Apache-2.0", ("Cargo.toml", "src/lib.rs")),
    "selectors-0.31.0": ("MPL-2.0", ("Cargo.toml", "lib.rs")),
}


def dependency_notices(directory):
    directory.mkdir(parents=True, exist_ok=True)
    manifest = run_text("cargo", "metadata", "--locked", "--format-version", "1", "--manifest-path", str(MANIFEST))
    metadata = json.loads(manifest)
    crates = [p for p in metadata["packages"] if p["source"]]
    crates.sort(key=lambda p: (p["name"], p["version"]))
    rows = []
    for crate in crates:
        name = crate["name"]
        version = crate["version"]
        label = name + "-" + version
        paths = list(notice_files(crate))
        additional = ADDITIONAL_NOTICES.get(label)
        if additional:
            license_id, evidence = additional
            path = ROOT / "LICENSES" / "rust-dependencies" / (license_id + ".txt")
            paths.append((Path(path.name), path))
            root = Path(crate["manifest_path"]).parent
            paths.extend((Path("upstream") / leaf, root / leaf) for leaf in evidence)
        if not paths:
            raise RuntimeError(f"Missing distributable license text for {label}. Inspect the locked source before packaging.")
        for rel, path in paths:
            dest = directory / "crates" / label / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, dest)
        license_id = crate["license"] or "See license file"
        choice = f" (distributed under {additional[0]})" if additional else ""
        rows.append(f"| {name} | {version} | {license_id}{choice} | [Texts](crates/{label}/) |")
    inventory = (
        "# Rust dependency notices\n\n"
        "This inventory covers the locked dependency graph of the Hello Needle search worker, "
        "including platform-specific crates. The complete sources and their individual notices "
        "are also in the corresponding source archive. System Qt, Poppler and other separately "
        "installed programs are identified in the platform packaging instructions.\n\n"
        "| Crate | Locked version | Declared license choices | Original notices |\n"
        "| --- | --- | --- | --- |\n"
    )
    (directory / "DEPENDENCIES.md").write_text(inventory + "\n".join(rows) + "\n", encoding="utf-8")
    print(f"Collected original notices for {len(crates)} locked Rust packages.", flush=True)


def archive_info(name, size, timestamp, executable=False):
    info = tarfile.TarInfo(name)
    info.mode = 0o755 if executable else 0o644
    info.size = size
    info.mtime = timestamp
    return info


def add_tree(archive, root, target, timestamp):
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        if path.is_symlink():
            raise RuntimeError(f"Unexpected dependency symlink {path}")
        destination = target + "/" + path.relative_to(root).as_posix()
        info = archive_info(destination, path.stat().st_size, timestamp, bool(path.stat().st_mode & 0o111))
        with path.open("rb") as file:
            archive.addfile(info, file)


def source_bundle(destination):
    sha = run_text("git", "rev-parse", "HEAD")
    if run_text("git", "status", "--porcelain", "--untracked-files=no"):
        raise RuntimeError("Source bundles require a checkout whose tracked files match HEAD.")
    run_text("git", "cat-file", "-e", f"{sha}:bridge/Cargo.lock")
    timestamp = int(run_text("git", "log", "-1", "--format=%ct", sha))
    prefix = "helloneedle-" + sha[:12]
    destination = destination.resolve()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with TemporaryDirectory(prefix="helloneedle-source-", dir=destination.parent) as temp:
        staging = Path(temp)
        vendor = staging / "vendor"
        invoke(["cargo", "vendor", "--locked", "--versioned-dirs", "--manifest-path", str(MANIFEST), str(vendor)],
               stdout=subprocess.DEVNULL)
        source_tar = staging / "source.tar"
        invoke(["git", "archive", f"--prefix={prefix}/", "--format=tar", f"--output={source_tar}", sha])
        additions = staging / "extras"
        config = additions / ".cargo"
        config.mkdir(parents=True)
        (config / "config.toml").write_text(
            '[source.crates-io]\nreplace-with = "vendored-sources"\n\n'
            '[source.vendored-sources]\ndirectory = "vendor"\n\n[net]\noffline = true\n', encoding="utf-8")
        (additions / "SOURCE-REVISION.txt").write_text(sha + "\n", encoding="ascii")
        (additions / "BUILDING-OFFLINE.txt").write_text(
            "Open a shell in the extracted source directory. The Cargo configuration in .cargo "
            "uses the vendored crate sources in vendor. See README.md for native dependencies.\n"
            "Verify the worker offline with cargo test --locked --manifest-path bridge/Cargo.toml.\n"
            "Configure and build Qt with cmake -S . -B build-qt and cmake --build build-qt.\n", encoding="utf-8")
        dependency_notices(additions / "RUST-NOTICES")
        with tarfile.open(source_tar, "a") as archive:
            add_tree(archive, additions, prefix, timestamp)
            add_tree(archive, vendor, prefix + "/vendor", timestamp)
        compressed = staging / "output.tar.gz"
        with source_tar.open("rb") as src, compressed.open("wb") as raw:
            with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as output:
                shutil.copyfileobj(src, output, 1024 * 1024)
        os.replace(compressed, destination)
    print(f"Wrote {destination.name} for commit {sha}.", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--notices-only", type=Path, metavar="DIR")
    mode.add_argument("--output", type=Path, metavar="SOURCE.tar.gz")
    args = parser.parse_args()
    try:
        if args.notices_only:
            dependency_notices(args.notices_only)
        else:
            source_bundle(args.output)
    except (OSError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Source distribution failed. {error}", file=sys.stderr)
        raise SystemExit(1)


if __name__ == "__main__":
    main()
