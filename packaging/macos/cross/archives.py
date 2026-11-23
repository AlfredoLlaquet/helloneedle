# SPDX-License-Identifier: GPL-3.0-or-later
"""Stage signed target MacPorts archives without modifying the build host's registry."""

import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import sys
import tarfile
from urllib.parse import quote

sys.path.insert(0, str(Path(__file__).resolve().parents[3] / "tools"))
from runtime_provenance.macports import records

HOST_PREFIX = Path("/opt/local")
MIRRORS = ("https://packages.macports.org", "https://fra.de.packages.macports.org")


def digest(path):
    result = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            result.update(block)
    return result.hexdigest()


def sdk_packages(manifest):
    """Start with the files actually deployed on Intel, plus the PDF build interfaces."""
    origins = {row["source"] for row in json.loads(manifest.read_text())}
    # A pkg-config interface can need other .pc files for its compile flags
    # without adding a dylib to the installed app. Resolve those interfaces too.
    env = dict(os.environ, PKG_CONFIG_PATH="", PKG_CONFIG_LIBDIR=os.pathsep.join(
        str(HOST_PREFIX / path) for path in ("lib/pkgconfig", "share/pkgconfig")))
    pkg = shutil.which("pkg-config") or "pkgconf"
    remaining = ["fontconfig", "freetype2", "libpng", "libjpeg", "libopenjp2", "lcms2", "libtiff-4", "zlib"]
    visited = set()
    while remaining:
        name = remaining.pop()
        if name in visited:
            continue
        visited.add(name)
        query = subprocess.run([pkg, "--variable=pcfiledir", name], env=env, stdout=subprocess.PIPE, text=True)
        if query.returncode or not query.stdout.strip():
            continue
        pcfile = (Path(query.stdout.strip()) / (name + ".pc")).resolve(strict=True)
        pcfile.relative_to(HOST_PREFIX)
        origins.add(str(pcfile))
        dependencies = subprocess.check_output([pkg, "--print-requires", "--print-requires-private", name],
                                               env=env, text=True)
        remaining.extend(line.split()[0] for line in dependencies.splitlines() if line.strip())
    return records(HOST_PREFIX, {path: {"source": path} for path in sorted(origins)})


def filenames(receipt, arch, darwin):
    name = receipt["name"]
    build_id = f"{name}-{receipt['version']}_{receipt['revision']}{receipt['variants'] or ''}"
    host_archive = Path(receipt["location"]).name
    names = []
    if host_archive.startswith(build_id + "."):
        if ".x86_64." in host_archive:
            names.append(host_archive.replace(".x86_64.", "." + arch + "."))
        elif ".noarch." in host_archive:
            names.append(host_archive)
        elif arch in receipt["archs"].split() and host_archive.endswith(".tbz2"):
            names.append(host_archive)
    if receipt["archs"] == "noarch":
        names.extend(f"{build_id}.{platform}.noarch.tbz2" for platform in
                     (f"darwin_{darwin}", "darwin_any", "any_any"))
    else:
        names.append(f"{build_id}.darwin_{darwin}.{arch}.tbz2")
    return list(dict.fromkeys(names))


def download(url, destination):
    return subprocess.run(["curl", "--fail", "--silent", "--show-error", "--location",
                           "--retry", "2", "--connect-timeout", "15",
                           "--output", destination, url], check=False).returncode == 0


def verified_archive(receipt, arch, darwin, cache):
    signer = HOST_PREFIX / "libexec/macports/bin/signify"
    keys = sorted((HOST_PREFIX / "share/macports/keys/archives").glob("*.pub"))
    legacy_key = HOST_PREFIX / "share/macports/macports-pubkey.pem"
    if not signer.is_file() or not keys:
        raise ValueError("MacPorts's installed archive verification program and public keys are required")
    for name in filenames(receipt, arch, darwin):
        path = cache / name
        for mirror in MIRRORS:
            url = mirror + "/" + quote(receipt["name"], safe="") + "/" + quote(name, safe="+")
            sig = cache / (name + ".sig")
            if download(url + ".sig", sig):
                checks = [(key, [signer, "-V", "-p", key, "-x", sig, "-m", path]) for key in keys]
            else:
                # Older MacPorts archives have the project's original RSA signature.
                sig = cache / (name + ".rmd160")
                if not legacy_key.is_file() or not download(url + ".rmd160", sig):
                    continue
                checks = [(legacy_key, ["openssl", "dgst", "-ripemd160", "-verify", legacy_key,
                                        "-signature", sig, path])]
            if not download(url, path):
                continue
            for key, args in checks:
                status = subprocess.run(args, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if status.returncode == 0:
                    return path, url, key.name
            raise ValueError(f"MacPorts archive signature verification failed for {url}")
    raise ValueError(f"No signed {arch} archive matches the active receipt for {receipt['name']}")


def relative_member(name):
    path = PurePosixPath(name)
    if path.is_absolute() or ".." in path.parts:
        raise ValueError(f"Invalid MacPorts member {name}")
    return path.relative_to("opt/local") if path.parts[:2] == ("opt", "local") else None


def unpack(archive, prefix, receipt, arch):
    """Extract into a private /opt/local tree. Rewrite absolute in-prefix links."""
    ownership = []
    hardlinks = []
    contents = {}
    with tarfile.open(archive, "r|*") as tar:
        for member in tar:
            if PurePosixPath(member.name).parts == ("+CONTENTS",):
                lines = tar.extractfile(member).read().decode("utf-8").splitlines()
                for line in lines:
                    if line.startswith("@"):
                        key, _, value = line.partition(" ")
                        if key in {"@name", "@portname", "@portversion", "@portrevision", "@portepoch", "@archs"}:
                            contents[key] = value
                continue
            rel = relative_member(member.name)
            if rel is None or not rel.parts:
                continue
            path = prefix / rel
            # No member or symlink may write outside the isolated SDK.
            path.parent.resolve().relative_to(prefix)
            path.parent.mkdir(parents=True, exist_ok=True)
            if member.isdir():
                path.mkdir(exist_ok=True)
                continue
            canonical = str(HOST_PREFIX / rel)
            ownership.append(canonical)
            if member.issym():
                target = Path(member.linkname)
                if target.is_absolute():
                    target = Path(os.path.relpath(prefix / target.relative_to(HOST_PREFIX), path.parent))
                (path.parent / target).resolve().relative_to(prefix)
                if path.is_symlink() and os.readlink(path) == str(target):
                    continue
                path.symlink_to(target)
            elif member.islnk():
                target = relative_member(member.linkname)
                if target is None:
                    raise ValueError(f"External hardlink in {archive.name}: {member.linkname}")
                hardlinks.append((path, prefix / target))
            elif member.isfile():
                with path.open("xb") as dest:
                    shutil.copyfileobj(tar.extractfile(member), dest)
                path.chmod(0o755 if member.mode & 0o111 else 0o644)
            else:
                raise ValueError(f"Unexpected archive member {member.name}")
    for path, target in hardlinks:
        target.resolve(strict=True).relative_to(prefix)
        os.link(target, path)
    expected = {"@name": f"{receipt['name']}-{receipt['version']}_{receipt['revision']}{receipt['variants'] or ''}",
                "@portname": receipt["name"], "@portversion": str(receipt["version"]),
                "@portrevision": str(receipt["revision"]), "@portepoch": str(receipt["epoch"] or 0)}
    architectures = contents.get("@archs", "").split()
    if any(contents.get(k) != v for k, v in expected.items()) or not ({arch, "noarch"} & set(architectures)):
        raise ValueError(f"Target archive receipt does not match the requested package: {contents}, {expected}")
    return ownership


def stage(work, darwin, manifest):
    sysroot = work / "sysroot"
    prefix = sysroot / "opt/local"
    cache = work / "downloads"
    cache.mkdir()
    prefix.mkdir(parents=True)
    origins = {"archives": {}, "paths": {}}
    for receipt in sdk_packages(manifest):
        name = receipt["name"]
        print(f"Resolving signed Apple Silicon MacPorts archive for {name}", flush=True)
        archive, url, key = verified_archive(receipt, "arm64", darwin, cache)
        origins["archives"][name] = {"url": url, "sha256": digest(archive), "signingKey": key}
        for canonical in unpack(archive, prefix, receipt, "arm64"):
            previous = origins["paths"].setdefault(canonical, name)
            if previous != name:
                raise ValueError(f"Conflicting MacPorts package ownership at {canonical}")
        archive.unlink()
    (sysroot / "archive-origins.json").write_text(json.dumps(origins, sort_keys=True) + "\n")
    return prefix
