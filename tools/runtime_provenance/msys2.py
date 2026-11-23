"""Resolve deployed UCRT64 files and preserve the distributor's signed all-source packages."""
# SPDX-License-Identifier: GPL-3.0-or-later

from concurrent.futures import ThreadPoolExecutor
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import urllib.error
import urllib.parse

from .common import Notices, archive_kind, digest, download, notice_name, safe_name


def fields(path):
    result = {}
    for block in path.read_text(encoding="utf-8").strip().split("\n\n"):
        if block:
            header, *values = block.splitlines()
            result[header.strip("%")] = values
    return result


def registry_for(files):
    original = Path(next(iter(files.values()))["source"])
    for candidate in original.parents:
        if (candidate / "var/lib/pacman/local").is_dir():
            return candidate
    raise RuntimeError(f"No active MSYS2 pacman database above {original}")


def owners(root, files):
    needed = {}
    for relative, file in files.items():
        native = Path(file["source"]).relative_to(root).as_posix().casefold()
        needed.setdefault(native, []).append(relative)
    groups = {}
    claimed = set()
    for info in sorted((root / "var/lib/pacman/local").glob("*/desc")):
        listing = fields(info.parent / "files").get("FILES", [])
        used = [p for p in listing if p.casefold() in needed]
        if not used:
            continue
        metadata = fields(info)
        name = metadata["NAME"][0]
        version = metadata["VERSION"][0]
        base = metadata.get("BASE", [name])[0].replace("mingw-w64-ucrt-x86_64-", "mingw-w64-", 1)
        key = safe_name(f"{base}-{version.split(':')[-1]}")
        group = groups.setdefault(key, {"binaryPackages": [], "origins": [], "notices": set()})
        group["binaryPackages"].append({"name": name, "base": base, "version": version,
                                        "licenses": metadata.get("LICENSE", []), "buildDate": metadata.get("BUILDDATE", [])})
        for path in used:
            native = path.casefold()
            if native in claimed:
                raise RuntimeError(f"More than one MSYS2 package owns {path}")
            claimed.add(native)
            group["origins"].extend(needed[native])
        group["notices"].update(root / p for p in listing if
                                "/share/licenses/" in p or ("/share/doc/" in p and notice_name(p)))
    missing = set(needed) - claimed
    if missing:
        raise RuntimeError("Unowned MSYS2 runtime files: " + ", ".join(sorted(missing)))
    return groups


def verify_signature(root, signature, archive):
    bash = root / "usr/bin/bash.exe"
    if not bash.exists():
        bash = Path(shutil.which("bash") or "bash")
    check = 'exec /usr/bin/pacman-key --verify "$(/usr/bin/cygpath -au "$1")" "$(/usr/bin/cygpath -au "$2")"'
    subprocess.run([str(bash), "-c", check, "verify-runtime-source", str(signature.resolve()), str(archive.resolve())],
                   check=True)


def signed_source_inputs(key, metadata, members):
    if len(metadata) != 1:
        raise RuntimeError(f"Missing unique signed .SRCINFO in {key}")
    path, description = next(iter(metadata.items()))
    values = {}
    for line in description.splitlines():
        field, sep, value = line.strip().partition(" = ")
        if sep:
            values.setdefault(field, []).append(value)
    identity = f"{values['pkgbase'][0]}-{values['pkgver'][0]}-{values['pkgrel'][0]}"
    if identity != key:
        raise RuntimeError(f"Source package {identity} does not match installed {key}")
    directory = PurePosixPath(path).parent
    shipped = {str(PurePosixPath(m).relative_to(directory)).split("/", 1)[0] for m in members}
    sources = sorted({s for k, vs in values.items() if k == "source" or k.startswith("source_") for s in vs})
    for source in sources:
        alias, sep, url = source.partition("::")
        if sep:
            filename = alias
        else:
            filename = PurePosixPath(urllib.parse.urlsplit(source).path).name
        if filename not in shipped and filename.removesuffix(".git") not in shipped:
            raise RuntimeError(f"Signed all-source package {key} is missing its declared input {source}")
    return sources


def collect(files, notices_root, output):
    root = registry_for(files)
    groups = owners(root, files)

    def package(item):
        key, group = item
        destination = output / "sources"
        for extension in ("zst", "xz", "gz"):
            filename = f"{key}.src.tar.{extension}"
            url = f"https://repo.msys2.org/mingw/sources/{filename}"
            try:
                download(url, destination / filename)
                break
            except urllib.error.HTTPError as err:
                if err.code != 404:
                    raise
        else:
            raise RuntimeError(f"No exact MSYS2 source package for {key}")
        archive = destination / filename
        signature = destination / (filename + ".sig")
        download(url + ".sig", signature)
        verify_signature(root, signature, archive)
        notices = Notices(notices_root / "texts")
        for path in sorted(group.pop("notices")):
            notices.installed(path)
        metadata = {}
        members = notices.archive(archive, nested=True, metadata=metadata)
        if not any(PurePosixPath(m).name == "PKGBUILD" for m in members):
            raise RuntimeError(f"PKGBUILD missing in {filename}")
        inputs = signed_source_inputs(key, metadata, members)
        return key, {"binaryPackages": group["binaryPackages"], "files": sorted(group["origins"]),
                     "sources": [{"file": archive.relative_to(output).as_posix(), "url": url,
                                  "sha256": digest(archive), "signature": signature.relative_to(output).as_posix(),
                                  "recipeInputs": inputs}],
                     "notices": notices.references}

    with ThreadPoolExecutor(max_workers=3) as executor:
        return dict(executor.map(package, sorted(groups.items())))
