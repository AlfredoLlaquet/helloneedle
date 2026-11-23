#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build a per-user Inno Setup installer from the staged UCRT64 application."""

import argparse
from collections import deque
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile


HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
# Windows supplies these graphics APIs. A developer's UCRT64 prefix can also
# contain Wine, DXVK or Mesa replacements with the same public DLL names.
WINDOWS_GRAPHICS = {"d3d8.dll", "d3d9.dll", "d3d10.dll", "d3d10_1.dll", "d3d10core.dll",
                    "d3d11.dll", "d3d12.dll", "dxgi.dll", "opengl32.dll"}


def file_hash(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def deploy_runtime(payload, prefix, system_dir):
    """Copy the selected runtime and record the source of every distributor-owned file."""
    origins = []
    pending = deque(payload / name for name in
                    ("Hello Needle.exe", "bin/helloneedle.exe", "bin/helloneedle-worker.exe"))

    def install(original, relative):
        target = payload / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(original, target)
        origins.append({"installed": relative.as_posix(), "source": str(original.absolute())})
        return target

    def install_tree(relative):
        source_dir = prefix / relative
        files = sorted(f for f in source_dir.rglob("*") if f.is_file())
        if not files:
            raise FileNotFoundError(f"Missing MSYS2 runtime data under {source_dir}")
        for file in files:
            install(file, file.relative_to(prefix))

    bindir = prefix / "bin"
    for name in ("pdftotext.exe", "pdftoppm.exe", "antiword.exe"):
        pending.append(install(bindir / name, Path("bin") / name))
    for name in ("platforms/qwindows.dll", "styles/qmodernwindowsstyle.dll",
                 "imageformats/qico.dll", "imageformats/qjpeg.dll"):
        pending.append(install(prefix / "share/qt6/plugins" / name, Path("plugins") / name))
    install(prefix / "share/qt6/translations/qtbase_es.qm", Path("translations/qtbase_es.qm"))
    # The Qt configuration belongs beside helloneedle.exe. The prefix is its parent.
    (payload / "bin/qt.conf").write_text(
        "[Paths]\nPrefix=..\nPlugins=plugins\nTranslations=translations\n", encoding="ascii")
    for directory in ("share/antiword", "share/poppler"):
        install_tree(Path(directory))
    for required in ("share/antiword/UTF-8.txt", "share/poppler/cidToUnicode/Adobe-Japan1"):
        if not (payload / required).is_file():
            raise FileNotFoundError(required)

    # Follow PE imports recursively, including the Qt plugins and both PDF utilities.
    candidates = {p.name.casefold(): p for p in bindir.glob("*.dll")}
    inspected = set()
    system_imports = set()
    objdump = bindir / "objdump.exe"
    while pending:
        binary = pending.popleft()
        listing = subprocess.check_output([objdump, "-p", binary], text=True, encoding="utf-8", errors="replace")
        if "file format pei-x86-64" not in listing:
            raise ValueError(f"Expected a Windows x64 PE image: {binary}")
        imports = re.findall(r"^\s*DLL Name:\s*([^\s]+)\s*$", listing, re.MULTILINE)
        if binary.name.casefold() == "nss3.dll":
            # NSS loads its cryptography and trust modules at runtime.
            imports.extend(("softokn3.dll", "freebl3.dll", "nssckbi.dll"))
        for name in imports:
            key = name.casefold()
            if key in inspected:
                continue
            inspected.add(key)
            if key in WINDOWS_GRAPHICS and (system_dir / name).is_file():
                system_imports.add(key)
                continue
            source = candidates.get(key)
            if source is not None:
                pending.append(install(source, Path("bin") / source.name))
            elif key.startswith(("api-ms-", "ext-ms-")) or (system_dir / name).is_file():
                system_imports.add(key)
            else:
                raise RuntimeError(f"{binary.name} requires missing DLL {name}")
    if "libfontconfig-1.dll" in inspected:
        install_tree(Path("etc/fonts"))
        install_tree(Path("share/fontconfig/conf.avail"))
    return sorted(origins, key=lambda item: item["installed"]), sorted(system_imports)


def inno_compiler():
    for variable in ("ProgramFiles(x86)", "ProgramFiles"):
        folder = os.environ.get(variable)
        if folder:
            candidate = Path(folder) / "Inno Setup 6/ISCC.exe"
            if candidate.is_file():
                return candidate
    found = shutil.which("ISCC.exe")
    if not found:
        raise FileNotFoundError("Install Inno Setup 6 or put ISCC.exe on PATH.")
    return Path(found).resolve()


def build(stage, icon, output, prefix):
    docs = Path("share/doc/helloneedle")
    metadata = json.loads((stage / docs / "BUILD-ENVIRONMENT.json").read_text(encoding="utf-8"))
    version = metadata["version"]
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        raise ValueError(f"Invalid application version {version!r}")
    expected = "helloneedle-worker " + version
    worker = subprocess.check_output([stage / "bin/helloneedle-worker.exe", "--version"], text=True).strip()
    if worker != expected:
        raise ValueError(f"Expected {expected!r}, received {worker!r}")

    compiler = inno_compiler()
    output.mkdir(parents=True, exist_ok=True)
    stem = f"HelloNeedle-{version}-Windows-x64"
    with tempfile.TemporaryDirectory(prefix="hn-win-", dir=output.parent) as temp:
        work = Path(temp)
        payload = work / "app"
        shutil.copytree(stage, payload)
        origins, system_imports = deploy_runtime(payload, prefix, Path(os.environ["SystemRoot"]) / "System32")
        runtime_files = work / "files.json"
        runtime_files.write_text(json.dumps(origins, indent=2) + "\n", encoding="utf-8")
        notices = payload / docs / "LICENSES/native"
        source_dir = work / "sources"
        subprocess.run([sys.executable, ROOT / "tools/runtime_sources.py", "--platform", "msys2",
                        "--manifest", runtime_files, "--stage", payload, "--notices", notices,
                        "--output", source_dir], check=True)
        for required in (notices / "PACKAGES.json", source_dir / "RUNTIME-SOURCES.json", source_dir / "SHA256SUMS"):
            if not required.is_file():
                raise FileNotFoundError(required)
        engine_notice = notices / "InnoSetup/LICENSE.txt"
        engine_notice.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(compiler.parent / "license.txt", engine_notice)
        source_zip = work / f"{stem}-Runtime-Sources.zip"
        with zipfile.ZipFile(source_zip, "w", compression=zipfile.ZIP_STORED, allowZip64=True) as archive:
            for f in sorted(source_dir.rglob("*")):
                if f.is_file():
                    archive.write(f, f.relative_to(source_dir).as_posix())
        metadata["runtime_source_archive"] = source_zip.name
        metadata["runtime_package_inventory"] = "LICENSES/native/PACKAGES.json"
        metadata["windows_system_imports"] = system_imports
        (payload / docs / "BUILD-ENVIRONMENT.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
        setup = work / f"{stem}-Setup.exe"
        subprocess.run([compiler, f"/DAppVersion={version}", f"/DPayloadDir={payload}",
                        f"/DSetupIcon={icon}", f"/DOutputFolder={work}", f"/DInstallerName={setup.stem}",
                        HERE / "setup.iss"], check=True)
        checksum = work / "SHA256SUMS"
        checksum.write_text("".join(f"{file_hash(p)}  {p.name}\n" for p in (setup, source_zip)), encoding="ascii")
        for artifact in (setup, source_zip, checksum):
            artifact.replace(output / artifact.name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--stage", type=Path, required=True, help="Existing Windows CMake package root")
    parser.add_argument("--icon", type=Path, required=True, help="Cow icon generated by CMake")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--prefix", type=Path, default=Path(sys.executable).resolve().parents[1],
                        help="MSYS2 UCRT64 prefix, defaults to the native Python installation")
    args = parser.parse_args()
    build(args.stage.resolve(), args.icon.resolve(), args.output.resolve(), args.prefix.resolve())
