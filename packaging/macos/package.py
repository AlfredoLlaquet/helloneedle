#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Produce, combine and test standalone Hello Needle drag-to-Applications disk images."""
import argparse
from contextlib import contextmanager
import filecmp
import json
import os
from pathlib import Path
import platform
import plistlib
import shutil
import subprocess
import sys
import tempfile

from macho import ARCHS, Bundle, mach_files, run, sign, slices, verify
import poppler

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[1]
NAME = "Hello Needle.app"


def metadata(app):
    with (app / "Contents/Info.plist").open("rb") as file:
        return plistlib.load(file)


def disk(app, dest, arch):
    version = metadata(app)["CFBundleShortVersionString"]
    dest.mkdir(parents=True, exist_ok=True)
    image = dest / f"HelloNeedle-{version}-macOS-{arch}.dmg"
    with tempfile.TemporaryDirectory(prefix=".helloneedle-disk-", dir=dest) as tmp:
        payload = Path(tmp) / "Hello Needle"
        candidate = Path(tmp) / image.name
        payload.mkdir()
        run("ditto", app, payload / NAME)
        (payload / "Applications").symlink_to("/Applications")
        shutil.copy2(ROOT / "LICENSE", payload / "LICENSE")
        shutil.copy2(HERE.parent / "MACOS.txt", payload / "Read Me.txt")
        if int(platform.mac_ver()[0].split(".")[0]) >= 27:
            # Golden Gate replaces hdiutil's create -srcfolder with diskutil image.
            try:
                run("diskutil", "image", "create", "from", "--format", "UDZO",
                    "--volumeName", "Hello Needle", payload, candidate)
            except subprocess.CalledProcessError:
                print("Retrying disk image creation with an APFS volume.", flush=True)
                run("hdiutil", "create", "-ov", "-fs", "APFS", "-format", "UDZO",
                    "-volname", "Hello Needle", "-srcfolder", payload, candidate)
        else:
            run("hdiutil", "create", "-ov", "-fs", "HFS+", "-format", "UDZO",
                "-volname", "Hello Needle", "-srcfolder", payload, candidate)
        run("hdiutil", "verify", candidate)
        candidate.replace(image)
    return image


@contextmanager
def mounted(image):
    with tempfile.TemporaryDirectory(prefix="Hello Needle mount ") as tmp:
        volume = Path(tmp) / "Mounted Image"
        volume.mkdir()
        run("hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", volume, image)
        try:
            if os.readlink(volume / "Applications") != "/Applications":
                raise ValueError("Disk needs its drag-to-Applications shortcut")
            if not filecmp.cmp(volume / "LICENSE", ROOT / "LICENSE", shallow=False):
                raise ValueError("Disk license differs from the project license")
            yield volume / NAME
        finally:
            run("hdiutil", "detach", "-quiet", volume)


def build(args):
    original, deploy, dist = args.app.resolve(), args.work.resolve(), args.output.resolve()
    sources, prefix = args.sources.resolve(), args.prefix.resolve(strict=True)
    arch = args.arch
    if not (original / "Contents/MacOS/helloneedle").is_file():
        raise ValueError(f"Missing staged application: {original}")
    deploy.mkdir(parents=True, exist_ok=True)
    stage = deploy / "payload" / NAME
    if original == stage:
        raise ValueError("The source and deployment copy must have different paths")
    if stage.exists():
        shutil.rmtree(stage)
    run("ditto", original, stage)
    docs = stage / "Contents/Resources/licenses"
    notices = docs / "LICENSES/native"
    tools = poppler.build(deploy, sources, docs / "LICENSES", prefix, arch)
    manifest = deploy / "runtime-manifest.json"
    package = Bundle(stage, prefix, arch)
    package.deploy(manifest, tools)
    if prefix != Path("/opt/local"):
        # Registry ownership and receipts identify canonical paths. The compiler
        # read the equivalent signed ARM archives in a private build prefix.
        import hashlib
        entries = json.loads(manifest.read_text(encoding="utf-8"))
        arm_origins = prefix.parents[1] / "archive-origins.json"
        archive_origins = json.loads(arm_origins.read_text(encoding="utf-8"))
        for row in entries:
            src = Path(row["source"])
            canonical = "/opt/local/" + src.relative_to(prefix).as_posix()
            row["source"] = canonical
            row["originSha256"] = hashlib.sha256(src.read_bytes()).hexdigest()
            owner = archive_origins["paths"][canonical]
            origin = archive_origins["archives"][owner]
            row["archive"] = origin["url"]
            row["archiveSha256"] = origin["sha256"]
        manifest.write_text(json.dumps(entries, indent=2) + "\n", encoding="utf-8")
        shutil.copy2(arm_origins, sources / "macports-arm64-archive-origins.json")
    derived = deploy / "derived-sources.json"
    poppler_component = f"hello-needle-poppler-{poppler.VERSION}"
    derived.write_text(json.dumps([{
        "name": poppler_component, "installed": ["Contents/MacOS/pdftotext", "Contents/MacOS/pdftoppm"],
        "sourceDirectory": str(sources / poppler_component), "license": "GPL-2.0-or-later",
    }], indent=2) + "\n", encoding="utf-8")
    imports = json.dumps(package.system_imports, sort_keys=True, indent=2) + "\n"
    (sources / f"apple-system-imports-{arch}.json").write_text(imports, encoding="utf-8")
    shutil.copy2(manifest, sources / f"runtime-manifest-{arch}.json")
    run(sys.executable, ROOT / "tools/runtime_sources.py", "--platform", "macports",
        "--manifest", manifest, "--derived-manifest", derived, "--stage", stage, "--notices", notices, "--output", sources)
    (notices / "APPLE-SYSTEM-IMPORTS.json").write_text(imports, encoding="utf-8")
    shutil.copy2(manifest, notices / "RUNTIME-FILES.json")
    sign(stage)
    verify(stage, (arch,))
    print("Disk image:", disk(stage, dist, arch), flush=True)


def cleaned_environment():
    env = os.environ.copy()
    for key in list(env):
        if key.startswith(("DYLD_", "QT_", "QML", "HN_QA_")) or key in (
            "HELLONEEDLE_WORKER", "HELLO_NEEDLE_WORKER", "ANTIWORDHOME", "POPPLER_DATADIR",
            "FONTCONFIG_PATH", "FONTCONFIG_FILE", "CMAKE_PREFIX_PATH", "PKG_CONFIG_PATH",
        ):
            env.pop(key, None)
    env["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
    return env


def install(args):
    image = args.image.resolve(strict=True)
    prefix = args.prefix.resolve()
    prefix.mkdir(parents=True, exist_ok=True)
    target = prefix / NAME
    if target.exists():
        raise ValueError(f"The installation test needs an empty target: {target}")
    # Detaching before first launch proves the installed app doesn't need the image.
    run("hdiutil", "verify", image)
    with mounted(image) as source:
        run("ditto", source, target)
    executables = target / "Contents/MacOS"
    gui, worker = executables / "helloneedle", executables / "helloneedle-worker"
    expected_archs = slices(gui)
    verify(target, expected_archs)
    if platform.machine() not in expected_archs:
        raise ValueError(f"No native {platform.machine()} slice for installed launch")
    env = cleaned_environment()
    with tempfile.TemporaryDirectory(prefix="Hello Needle clean launch ") as tmp:
        work = Path(tmp)
        # Only platform directories are on PATH. Native sources live beside the
        # release, not inside the installed application or in its search path.
        version = subprocess.check_output([gui, "--version"], cwd=work, env=env, text=True).strip()
        if metadata(target)["CFBundleShortVersionString"] not in version:
            raise ValueError(f"Unexpected installed application version: {version}")
        run(gui, "--smoke-test", cwd=work, env=env, timeout=60)
        run("open", "-W", "-n", "-a", target, "--args", "--smoke-test", cwd=work, env=env, timeout=70)
        encodings = subprocess.check_output([executables / "pdftotext", "-listenc"], env=env, cwd=work, text=True)
        if "EUC-JP" not in {line.strip() for line in encodings.splitlines()}:
            raise ValueError("Installed Poppler could not find its external Japanese encoding map")
        # This original two-character PDF names Adobe's UniJIS CMap and omits
        # ToUnicode. Extraction therefore exercises the bundled CJK resources.
        japanese = subprocess.check_output([executables / "pdftotext", "-enc", "UTF-8",
                                            HERE / "japanese-cmap.pdf", "-"], env=env, cwd=work)
        if japanese.decode("utf-8").strip() != "日本":
            raise ValueError("Installed Poppler failed to decode the bundled Japanese CMap fixture")
        qa = ROOT / "tests/installer/runtime-check.py"
        documents, inventory = work / "Carpeta de prueba", work / "fixture-hashes.json"
        evidence = args.evidence.resolve() if args.evidence else work / "document-evidence.json"
        evidence.parent.mkdir(parents=True, exist_ok=True)
        run(sys.executable, qa, "prepare", "--documents", documents, "--inventory", inventory)
        run(sys.executable, qa, "verify", "--documents", documents, "--inventory", inventory, "--worker", worker,
            "--pdf-renderer", executables / "pdftoppm", "--evidence", evidence, cwd=work, env=env)
        run(sys.executable, qa, "unchanged", "--documents", documents, "--inventory", inventory)
    print(f"Installed {target.name}: {', '.join(expected_archs)}; native {platform.machine()} GUI, PDF and Word passed.")


def merge(arm, intel, output):
    # Flavors must contain the same program and the same runtime ABI. Every
    # code-bearing file, including all Qt plugins and converters, gets both slices.
    left = {p.relative_to(arm) for p in mach_files(arm)}
    right = {p.relative_to(intel) for p in mach_files(intel)}
    if left != right:
        raise ValueError(f"Mac runtime sets differ: {sorted(left.symmetric_difference(right))}")
    ainfo, iinfo = metadata(arm), metadata(intel)
    for key in ("CFBundleIdentifier", "CFBundleShortVersionString", "CFBundleVersion"):
        if ainfo[key] != iinfo[key]:
            raise ValueError(f"Cannot merge unequal {key}")
    with tempfile.TemporaryDirectory(prefix="Hello Needle universal ") as tmp:
        scratch = Path(tmp)
        target = scratch / NAME
        run("ditto", arm, target)
        for relative in sorted(left):
            inputs = []
            for cpu, source in (("arm64", arm), ("x86_64", intel)):
                path = source / relative
                if slices(path) != [cpu]:
                    thin = scratch / f"thin-{cpu}"
                    run("lipo", path, "-thin", cpu, "-output", thin)
                    path = thin
                inputs.append(path)
            run("lipo", "-create", *inputs, "-output", target / relative)
            (target / relative).chmod(0o755)

        # Framework resources and data must be interchangeable between CPUs.
        # Build receipts and package inventories legitimately differ, so retain
        # both complete native license trees under architecture-specific names.
        legal = Path("Contents/Resources/licenses")
        for relative in (p.relative_to(intel) for p in intel.rglob("*") if p.is_file() and not p.is_symlink()):
            if relative in left or relative.is_relative_to(legal) or "_CodeSignature" in relative.parts or relative == Path("Contents/Info.plist"):
                continue
            if not (arm / relative).is_file() or not filecmp.cmp(arm / relative, intel / relative, shallow=False):
                raise ValueError(f"Universal data differs between architectures: {relative}")
        for cpu, source in (("arm64", arm), ("x86_64", intel)):
            base = target / legal / "architectures" / cpu
            base.mkdir(parents=True)
            for name in ("LICENSES/native", "BUILD-ENVIRONMENT.json", "NATIVE-COMPONENTS.md"):
                item = source / legal / name
                if item.is_dir():
                    shutil.copytree(item, base / "native")
                elif item.is_file():
                    shutil.copy2(item, base / item.name)
        duplicates = target / legal / "LICENSES/native"
        shutil.rmtree(duplicates)
        minima = {cpu: metadata(source)["LSMinimumSystemVersion"] for cpu, source in (("arm64", arm), ("x86_64", intel))}
        common = min(minima.values(), key=lambda value: tuple(map(int, value.split("."))))
        ainfo.update(LSMinimumSystemVersion=common, LSMinimumSystemVersionByArchitecture=minima)
        (target / "Contents/Info.plist").write_bytes(plistlib.dumps(ainfo))
        sign(target)
        verify(target, ARCHS)
        return disk(target, output.resolve(), "Universal")


@contextmanager
def app_from_disk(path):
    path = path.resolve(strict=True)
    if path.is_dir():
        yield path
    else:
        with mounted(path) as app:
            yield app


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    command = commands.add_parser("build")
    for key in ("app", "work", "output", "sources"):
        command.add_argument("--" + key, type=Path, required=True)
    command.add_argument("--prefix", type=Path, default=Path("/opt/local"))
    command.add_argument("--arch", choices=ARCHS, default=platform.machine())
    command = commands.add_parser("install")
    command.add_argument("--image", type=Path, required=True)
    command.add_argument("--prefix", type=Path, required=True)
    command.add_argument("--evidence", type=Path)
    command = commands.add_parser("universal")
    for key in ("arm", "intel", "output"):
        command.add_argument("--" + key, type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("Disk image creation, installation and signing require macOS.")
    if args.command == "build":
        build(args)
    elif args.command == "install":
        install(args)
    else:
        with app_from_disk(args.arm) as arm, app_from_disk(args.intel) as intel:
            print("Universal disk image:", merge(arm, intel, args.output))


if __name__ == "__main__":
    main()
