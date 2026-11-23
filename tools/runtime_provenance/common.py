"""Content-addressed notices and exact source downloads for standalone installers."""
# SPDX-License-Identifier: GPL-3.0-or-later

import contextlib
import hashlib
import json
import os
import re
import shutil
import subprocess
import tarfile
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile
from pathlib import Path, PurePosixPath

CHUNK = 1024 * 1024


def digest(path, kind="sha256"):
    h = hashlib.new(kind)
    with path.open("rb") as stream:
        while data := stream.read(CHUNK):
            h.update(data)
    return h.hexdigest()


def write_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True, ensure_ascii=False) + "\n", encoding="utf-8")


def command(args, **kwargs):
    return subprocess.check_output([str(x) for x in args], text=True, encoding="utf-8", errors="replace", **kwargs).strip()


def safe_name(name):
    if name in ("", ".", "..") or not re.fullmatch(r"[A-Za-z0-9@+_.-]+", name):
        raise ValueError(f"Invalid component or source filename {name!r}")
    return name


def download(url, destination):
    if urllib.parse.urlparse(url).scheme != "https":
        raise ValueError(f"Source URL must use HTTPS: {url}")
    destination.parent.mkdir(parents=True, exist_ok=True)
    partial = destination.with_name(destination.name + ".partial")
    for attempt in range(3):
        try:
            request = urllib.request.Request(url, headers={"User-Agent": "HelloNeedle-runtime-sources/1"})
            with urllib.request.urlopen(request, timeout=90) as data, partial.open("wb") as out:
                shutil.copyfileobj(data, out, CHUNK)
                size = data.headers.get("Content-Length")
                if size and out.tell() != int(size):
                    raise OSError(f"Incomplete response for {url}")
            os.replace(partial, destination)
            return
        except (OSError, EOFError) as error:
            partial.unlink(missing_ok=True)
            if (isinstance(error, urllib.error.HTTPError) and error.code in (403, 404, 410)) or attempt == 2:
                raise
            time.sleep(2 ** attempt)


def archive_kind(name):
    name = name.lower()
    if name.endswith(".zip"):
        return "zip"
    if name.endswith((".tar.gz", ".tgz", ".tar.xz", ".txz", ".tar.bz2", ".tbz", ".tbz2",
                      ".tar.zst", ".tar.lz", ".tar.lzma", ".tar.z", ".tar")):
        return "tar"
    return None


def notice_name(name):
    parts = PurePosixPath(name).parts
    if not parts:
        return False
    leaf = parts[-1].lower()
    if leaf.endswith((".png", ".svg", ".pdf", ".gif", ".ico", ".jpg", ".so", ".dll", ".ttf", ".otf")):
        return False
    if any(part.lower() in {"licenses", "licences", "copyrights", "legal"} for part in parts[:-1]):
        return True
    return bool(re.match(r"^(?:(?:license|licence|copying|copyright|notice|notices|authors|credits|patents|unlicense|ofl|gpl|lgpl|third[-_ ]party(?:[-_ ]notices)?)(?:$|[._ -])|qt_attribution\.json$)", leaf))


@contextlib.contextmanager
def tar_reader(path):
    decompressor = {".zst": "zstd", ".lz": "lzip", ".z": "gzip"}.get(path.suffix.lower())
    if not decompressor:
        with tarfile.open(path, mode="r|*") as archive:
            yield archive
        return
    tool = shutil.which(decompressor)
    if not tool:
        raise RuntimeError(f"Install {decompressor} to read {path.name}")
    with subprocess.Popen([tool, "-dc", str(path)], stdout=subprocess.PIPE) as proc:
        try:
            with tarfile.open(fileobj=proc.stdout, mode="r|") as archive:
                yield archive
            while proc.stdout.read(CHUNK):
                pass
        finally:
            proc.stdout.close()
        if proc.wait() != 0:
            raise RuntimeError(f"Unable to read compressed source {path.name}")


class Notices:
    def __init__(self, directory):
        self.directory = directory
        self.references = {}

    def add(self, name, data):
        if not data.strip():
            return
        fingerprint = hashlib.sha256(data).hexdigest()
        self.directory.mkdir(parents=True, exist_ok=True)
        filename = fingerprint + ".txt"
        path = self.directory / filename
        if not path.exists():
            path.write_bytes(data)
        self.references[name] = filename

    def installed(self, path):
        if path.is_file() and path.stat().st_size < 16 * CHUNK:
            self.add(str(path), path.read_bytes())

    def archive(self, path, nested=False, prefix="", metadata=None):
        """Read original license files without extracting arbitrary paths."""
        origin = prefix + path.name + "/"
        kind = archive_kind(path.name)
        names = set()
        if kind == "zip":
            with zipfile.ZipFile(path) as archive:
                for item in archive.infolist():
                    if not item.is_dir() and notice_name(item.filename) and item.file_size < 16 * CHUNK:
                        self.add(origin + item.filename, archive.read(item))
                    if not item.is_dir():
                        names.add(item.filename)
        elif kind == "tar":
            with tar_reader(path) as archive:
                for item in archive:
                    if not item.isfile():
                        continue
                    names.add(item.name)
                    if metadata is not None and PurePosixPath(item.name).name == ".SRCINFO" and item.size < CHUNK:
                        metadata[item.name] = archive.extractfile(item).read().decode("utf-8")
                    elif notice_name(item.name) and item.size < 16 * CHUNK:
                        self.add(origin + item.name, archive.extractfile(item).read())
                    elif nested and archive_kind(item.name):
                        with tempfile.TemporaryDirectory(prefix="hello-needle-upstream-") as temp:
                            child = Path(temp) / PurePosixPath(item.name).name
                            with child.open("wb") as stream:
                                shutil.copyfileobj(archive.extractfile(item), stream, CHUNK)
                            self.archive(child, prefix=origin)
        elif notice_name(path.name):
            self.installed(path)
        return names


def manifest_files(manifest, stage, derived=()):
    entries = json.loads(manifest.read_text(encoding="utf-8"))
    if not isinstance(entries, list) or not entries:
        raise ValueError("A runtime provenance manifest must list the files copied into the installer")
    found = {}
    for entry in entries:
        relative = PurePosixPath(entry["installed"])
        if relative.is_absolute() or ".." in relative.parts or not relative.parts or "\\" in str(relative):
            raise ValueError(f"Invalid staged path {relative}")
        source = Path(entry["source"])
        staged = stage.joinpath(*relative.parts)
        if not source.is_absolute() or not source.is_file() or not staged.is_file():
            raise ValueError(f"Missing staged or package-managed original for {relative}: {source}")
        if not staged.resolve().is_relative_to(stage.resolve()):
            raise ValueError(f"Staged runtime link escapes the installer: {relative}")
        key = relative.as_posix()
        if key in found:
            raise ValueError(f"Duplicate staged runtime origin: {key}")
        record = {"installed": key, "source": str(source), "preInstallerSha256": digest(staged)}
        for field in ("originSha256", "archive"):
            if field in entry:
                record[field] = entry[field]
        if "originSha256" in record and not re.fullmatch("[0-9a-f]{64}", record["originSha256"]):
            raise ValueError(f"Invalid original package digest for {key}")
        found[key] = record
    for path in stage.rglob("*"):
        if path.is_symlink() or not path.is_file():
            continue
        relative = path.relative_to(stage).as_posix()
        parts = relative.split("/")
        if "_CodeSignature" in parts or "LICENSES" in parts:
            continue
        native = path.suffix.lower() in (".dll", ".dylib", ".so") or (
            path.suffix.lower() == ".exe" and path.name.casefold() not in
            {"hello needle.exe", "helloneedle.exe", "helloneedle-worker.exe"})
        native = native or relative.startswith(("Contents/Frameworks/", "Contents/PlugIns/"))
        native = native or path.name.lower() in ("pdftotext", "pdftoppm", "antiword")
        if native and relative not in found and relative not in derived:
            raise ValueError(f"Staged third-party runtime file has no package owner mapping: {relative}")
    return found
