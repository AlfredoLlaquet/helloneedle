#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Collect corresponding source and offline notices for the actual native installer payload."""

import argparse
import json
import os
import platform as host_platform
import shutil
import tempfile
from pathlib import Path, PurePosixPath

if __package__:
    from .runtime_provenance import macports, msys2
    from .runtime_provenance.common import Notices, archive_kind, digest, manifest_files, notice_name, safe_name, write_json
else:
    from runtime_provenance import macports, msys2
    from runtime_provenance.common import Notices, archive_kind, digest, manifest_files, notice_name, safe_name, write_json


def derived_sources(manifest, stage, texts, output):
    packages, owned = {}, {}
    rows = json.loads(manifest.read_text(encoding="utf-8")) if manifest else []
    for row in rows:
        key = safe_name(row["name"])
        src = Path(row["sourceDirectory"]).resolve(strict=True)
        destination = output / key
        if src != destination.resolve():
            shutil.copytree(src, destination, dirs_exist_ok=True)
        notices = Notices(texts)
        entries = []
        for member in sorted(destination.rglob("*")):
            if member.is_symlink():
                raise ValueError(f"Derived source input must be a regular file: {member}")
            if member.is_file():
                entries.append({"file":member.relative_to(output).as_posix(), "sha256":digest(member)})
                if archive_kind(member.name):
                    notices.archive(member)
                elif notice_name(member.name):
                    notices.installed(member)
        if not any(archive_kind(x["file"]) for x in entries) or not any(
                Path(x["file"]).suffix.lower() in (".sh", ".py", ".cmake") for x in entries):
            raise RuntimeError(f"Complete sources and an executable build recipe are required for {key}")
        if not row["installed"]:
            raise ValueError(f"Missing installed paths for derived component {key}")
        for name in row["installed"]:
            rel = PurePosixPath(name)
            file = stage.joinpath(*rel.parts)
            if rel.is_absolute() or ".." in rel.parts or not file.is_file() or not file.resolve().is_relative_to(stage.resolve()):
                raise ValueError(f"Invalid derived component path: {name}")
            if name in owned:
                raise ValueError(f"Duplicate derived runtime path: {name}")
            owned[name] = {"installed":name, "derivedFrom":key, "preInstallerSha256":digest(file)}
        packages[key] = {"binaryPackages":[{"name":key, "licenses":row["license"]}], "files":sorted(row["installed"]),
                         "sources":entries, "notices":notices.references}
    return packages, owned


def write_records(manager, files, packages, notices, output):
    for key, item in sorted(packages.items()):
        references = item.pop("notices")
        if not references:
            raise RuntimeError(f"No original copyright or license text was found for {key}")
        index = notices / "packages" / safe_name(key) / "NOTICES.json"
        write_json(index, references)
        item["noticeIndex"] = index.relative_to(notices).as_posix()
    assigned = [name for p in packages.values() for name in p["files"]]
    if len(assigned) != len(set(assigned)) or set(assigned) != set(files):
        raise RuntimeError("The native package map must account for each staged runtime file exactly once")
    # The host can differ from the target when MacPorts archives supply another architecture.
    index = {"schema":1, "manager":manager, "collectorHost":host_platform.platform(),
             "files":[files[k] for k in sorted(files)], "packages":dict(sorted(packages.items())),
             "sourceIndex":"RUNTIME-SOURCES.json"}
    write_json(notices / "PACKAGES.json", index)
    (notices / "README.md").write_text(
        "# Native runtime components\n\n"
        "PACKAGES.json identifies the original distribution package or local build for each bundled runtime file. "
        "Each component’s NOTICES.json maps original notice filenames to byte-for-byte copies in the texts directory. "
        "Hashes of installer-stage files precede the final code signatures. Source filenames refer to the native runtime "
        "source companion provided beside this installer. Its SHA256SUMS verifies every archive, recipe and index.\n",
        encoding="utf-8")
    shutil.copyfile(notices / "PACKAGES.json", output / "RUNTIME-SOURCES.json")
    (output / "BUILDING.txt").write_text(
        "Start with the Hello Needle source archive named in the installed BUILD-ENVIRONMENT.json. "
        "Use its installer scripts to reproduce the application and gather the same runtime for packaging.\n\n"
        + ("Each signed MSYS2 all-source archive contains the upstream inputs, PKGBUILD and local patches. "
           "Verify its adjacent signature with pacman-key --verify, extract the archive, then run makepkg-mingw "
           "in the UCRT64 environment in the directory containing PKGBUILD. The inventory records split binary package names and versions.\n"
           if manager == "msys2" else
           "Extract the MacPorts recipe tree, generate its PortIndex with portindex, and configure that local "
           "file:// ports tree as the first source in MacPorts sources.conf. The inventory identifies the exact Portfile, "
           "PortGroups, versions and installed variants. Place each component's distfiles in the locations shown in "
           "port distfiles for those variants, then run port -s install with the same variants. Derived components have "
           "their own checked source archive, patches and build instructions.\n"),
        encoding="utf-8")
    paths = sorted(p for p in output.rglob("*") if p.is_file() and p.name != "SHA256SUMS")
    sums = "".join(f"{digest(p)}  {p.relative_to(output).as_posix()}\n" for p in paths)
    (output / "SHA256SUMS.partial").write_text(sums, encoding="utf-8")
    os.replace(output / "SHA256SUMS.partial", output / "SHA256SUMS")


def collect(platform, manifest, stage, notices, output, derived_manifest=None):
    stage, notices, output = (Path(p).resolve() for p in (stage, notices, output))
    if not notices.is_relative_to(stage) or output.is_relative_to(stage):
        raise ValueError("Keep installed notices inside the stage and runtime sources outside the installer")
    output.mkdir(parents=True, exist_ok=True)
    # A failed retry must never leave a success checksum file for a partial collection.
    (output / "SHA256SUMS").unlink(missing_ok=True)
    (output / "SHA256SUMS.partial").unlink(missing_ok=True)
    notices.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="helloneedle-native-notices-", dir=notices.parent) as temp:
        new_notices = Path(temp) / "native"
        new_notices.mkdir()
        derived, generated = derived_sources(Path(derived_manifest) if derived_manifest else None,
                                              stage, new_notices / "texts", output)
        files = manifest_files(Path(manifest), stage, generated)
        packages = {"msys2":msys2.collect, "macports":macports.collect}[platform](files, new_notices, output)
        if set(derived).intersection(packages) or set(generated).intersection(files):
            raise RuntimeError("Overlapping origins for package-managed and locally built components")
        for name in packages.keys() | derived.keys():
            safe_name(name)
        write_records(platform, {**files, **generated}, {**packages, **derived}, new_notices, output)
        if notices.exists():
            if not (notices / "PACKAGES.json").is_file():
                raise RuntimeError(f"Unexpected preexisting native notice directory: {notices}")
            backup = Path(temp) / "previous"
            notices.rename(backup)
            try:
                new_notices.rename(notices)
            except OSError:
                backup.rename(notices)
                raise
        else:
            new_notices.rename(notices)
    print(f"Verified {len(files)+len(generated)} native runtime files from {len(packages)+len(derived)} components.", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--platform", required=True, choices=("msys2","macports"))
    for name in ("manifest", "stage", "notices", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--derived-manifest", type=Path)
    collect(**vars(parser.parse_args()))


if __name__ == "__main__":
    main()
