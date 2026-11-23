"""Resolve deployed UCRT64 files and preserve the distributor's signed all-source packages."""
# SPDX-License-Identifier: GPL-3.0-or-later

from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from pathlib import Path, PurePosixPath
import os
import re
import subprocess
import tempfile
import urllib.error
import urllib.parse

from .common import Notices, digest, download, notice_name, safe_name


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


def msys_paths(root, *paths):
    args = [str(path.resolve()) for path in paths]
    if os.name == "nt":
        result = subprocess.run([str(root / "usr/bin/cygpath.exe"), "-au", *args],
                                check=True, capture_output=True, text=True)
        return result.stdout.splitlines()
    return args


@contextmanager
def signing_keys(root):
    # The distributor ships public keys and a separate list of disabled fingerprints.
    # gpgv needs only these inputs; pacman's writable administrator keyring is unnecessary.
    distribution_keys = root / "usr/share/pacman/keyrings"
    keys = (distribution_keys / "msys2.gpg").read_bytes()
    revoked = {line.split("#", 1)[0].strip().upper() for line in
               (distribution_keys / "msys2-revoked").read_text(encoding="ascii").splitlines()}
    revoked.discard("")
    if any(not re.fullmatch(r"[0-9A-F]{40}|[0-9A-F]{64}", fpr) for fpr in revoked):
        raise ValueError("Invalid fingerprint in MSYS2's revoked-key list")
    binaries = root / "usr/bin"
    gpg = str(binaries / "gpg.exe") if os.name == "nt" else "gpg"
    gpgv = str(binaries / "gpgv.exe") if os.name == "nt" else "gpgv"
    with tempfile.TemporaryDirectory(prefix="helloneedle-msys2-keys-") as temporary:
        home = Path(temporary)
        ring = home / "distribution.gpg"
        home_arg, ring_arg = msys_paths(root, home, ring)
        if b"-----BEGIN PGP PUBLIC KEY BLOCK-----" in keys:
            keys = subprocess.run([gpg, "--no-options", "--batch", "--homedir", home_arg, "--dearmor"],
                                  input=keys, stdout=subprocess.PIPE, check=True).stdout
        ring.write_bytes(keys)
        yield [gpgv, "--homedir", home_arg, "--keyring", ring_arg, "--status-fd", "1"], revoked


def verified_signer(status, revoked):
    records = [line.split()[1:] for line in status.splitlines() if line.startswith("[GNUPG:] ")]
    failure = {"BADSIG", "ERRSIG", "NO_PUBKEY", "EXPKEYSIG", "EXPSIG", "REVKEYSIG", "KEYREVOKED", "SIGEXPIRED"}
    if any(entry[0] in failure for entry in records):
        raise ValueError("MSYS2 archive has an invalid or expired signature")
    valid = [entry for entry in records if entry[0] == "VALIDSIG"]
    if len(valid) != 1 or len(valid[0]) < 10:
        raise ValueError("Expected one verified MSYS2 archive signature")
    signer = valid[0][1].upper()
    primary = valid[0][10].upper() if len(valid[0]) > 10 else signer
    if {signer, primary} & revoked:
        raise ValueError(f"MSYS2 revoked the archive signer {primary}")
    return {"signingFingerprint": signer, "primaryFingerprint": primary}


def verify_signature(root, verifier, signature, archive):
    command, revoked = verifier
    result = subprocess.run([*command, "--", *msys_paths(root, signature, archive)],
                            check=True, stdout=subprocess.PIPE, text=True, encoding="utf-8", errors="replace")
    return verified_signer(result.stdout, revoked)


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
    sources = sorted({s for k in ("source", "source_any", "source_x86_64", "source_ucrt64") for s in values.get(k, [])})
    for source in sources:
        alias, sep, url = source.partition("::")
        if sep:
            filename = alias
        else:
            filename = urllib.parse.unquote(PurePosixPath(urllib.parse.urlsplit(source).path).name)
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
        signer = verify_signature(root, verifier, signature, archive)
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
                                  "recipeInputs": inputs, **signer}],
                     "notices": notices.references}

    with signing_keys(root) as verifier, ThreadPoolExecutor(max_workers=3) as executor:
        return dict(executor.map(package, sorted(groups.items())))
