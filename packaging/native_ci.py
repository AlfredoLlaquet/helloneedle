#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Build a dependency-managed Hello Needle package or test a separately installed copy."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
SYSTEM = platform.system()
EXE = ".exe" if SYSTEM == "Windows" else ""
PROJECT_URL = "https://github.com/AlfredoLlaquet/helloneedle"
MAINTAINER = "13706619+AlfredoLlaquet@users.noreply.github.com"


def run(args, **kwargs):
    print("+", " ".join(str(a) for a in args), flush=True)
    return subprocess.run(args, check=True, **kwargs)


def output(args, **kwargs):
    return subprocess.check_output(args, text=True, encoding="utf-8", errors="replace", **kwargs).strip()


def layout(prefix):
    if SYSTEM == "Darwin":
        contents = prefix / "Hello Needle.app" / "Contents"
        return contents / "MacOS", contents / "Resources" / "licenses"
    return prefix / "bin", prefix / "share" / "doc" / "helloneedle"


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def files_under(directory):
    return sorted(path for path in directory.rglob("*") if path.is_file())


def packaged_icons(work, prefix):
    sizes = (16, 32, 48, 64, 128, 256)
    if SYSTEM == "Windows":
        # windres preserves the original PNG frames in each executable's icon resource.
        containers = [prefix / "Hello Needle.exe", prefix / "bin/helloneedle.exe"]
    elif SYSTEM == "Darwin":
        containers = [prefix / "Hello Needle.app/Contents/Resources/helloneedle.icns"]
        sizes = (16, 32, 64, 128, 256, 512, 1024)
    else:
        containers = []
        sizes = (16, 22, 24, 32, 48, 64, 96, 128, 256, 512, 1024)
    contents = {path: path.read_bytes() for path in containers}
    fingerprints = {}
    for size in sizes:
        png = (work / "icons" / f"{size}.png").read_bytes()
        if contents:
            for path, binary in contents.items():
                if png not in binary:
                    raise SystemExit(f"Missing {size}px application icon in {path}")
        else:
            installed = prefix / f"share/icons/hicolor/{size}x{size}/apps/helloneedle.png"
            if installed.read_bytes() != png:
                raise SystemExit(f"Installed application icon differs: {installed}")
        fingerprints[str(size)] = hashlib.sha256(png).hexdigest()
    return {"master_sha256": sha256(ROOT / "desktop/qt/helloneedle.png"), "frames_sha256": fingerprints}


def system_packages():
    if SYSTEM == "Linux":
        return output(["dpkg-query", "-W", "-f=${binary:Package} ${Version}\\n",
                       "qt6-base-dev", "qt6-qpa-plugins", "qt6-translations-l10n", "poppler-utils", "antiword"])
    if SYSTEM == "FreeBSD":
        return output(["pkg", "query", "%n %v", "qt6-base", "qt6-translations", "poppler-utils", "antiword"])
    if SYSTEM == "Darwin":
        ports = ["qt6-qtbase", "qt6-qttranslations", "poppler", "antiword"]
        installed = output(["/opt/local/bin/port", "-q", "installed", *ports])
        # MacPorts keeps inactive revisions after upgrades. Record the versions in use.
        active = {line.split()[0]: line.strip() for line in installed.splitlines() if line.endswith(" (active)")}
        missing = [name for name in ports if name not in active]
        if missing:
            raise SystemExit("MacPorts has no active installation for " + ", ".join(missing))
        return "\n".join(active[name] for name in ports)
    managed = ["mingw-w64-ucrt-x86_64-" + name for name in ("qt6-base", "qt6-translations", "poppler", "antiword")]
    # MSYS2 has both combined and split GCC runtime packages. Ask about the actual DLLs.
    runtime = output(["pacman", "-Qqo", "/ucrt64/bin/libgcc_s_seh-1.dll", "/ucrt64/bin/libstdc++-6.dll",
                      "/ucrt64/bin/libwinpthread-1.dll"]).splitlines()
    return output(["pacman", "-Q", *dict.fromkeys(managed + runtime)])


def converter_versions():
    result = {}
    for name, flag in (("pdftotext", "-v"), ("pdftoppm", "-v"), ("antiword", "-h")):
        executable = shutil.which(name)
        if not executable:
            raise SystemExit(f"Missing converter: {name}")
        # Antiword's -h includes the version and exits with its usage status.
        banner = subprocess.run([executable, flag], stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                text=True, encoding="utf-8", errors="replace", timeout=15).stdout
        version = next((line.strip() for line in banner.splitlines() if "version" in line.lower()), None)
        if not version:
            raise SystemExit(f"Could not read {name} version: {banner}")
        result[name] = {"executable": executable, "version": version}
    return result


def os_description():
    if SYSTEM == "Windows":
        powershell = shutil.which("powershell.exe") or Path(os.environ["SystemRoot"]) / "System32/WindowsPowerShell/v1.0/powershell.exe"
        return output([powershell, "-NoProfile", "-Command",
                       "(Get-CimInstance Win32_OperatingSystem).Caption"])
    if SYSTEM == "Darwin":
        return output(["sw_vers"])
    if SYSTEM == "FreeBSD":
        return output(["freebsd-version", "-ku"])
    return next((line.split("=", 1)[1].strip('"') for line in Path("/etc/os-release").read_text().splitlines()
                 if line.startswith("PRETTY_NAME=")), platform.platform())


def environment(revision, version, bindir):
    commands = {"Linux": ["ldd"], "FreeBSD": ["ldd"], "Darwin": ["otool", "-L"],
                "Windows": ["objdump", "-p"]}
    libraries = {}
    binaries = [bindir / (name + EXE) for name in ("helloneedle", "helloneedle-worker")]
    if SYSTEM == "Windows":
        binaries.append(bindir.parent / "Hello Needle.exe")
    for binary in binaries:
        linked = output(commands[SYSTEM] + [binary])
        if SYSTEM == "Windows":
            linked = "\n".join(line.strip() for line in linked.splitlines() if "DLL Name:" in line)
            if binary.name == "Hello Needle.exe" and any(name in linked.lower() for name in ("qt6", "libgcc", "libstdc++", "libwinpthread")):
                raise SystemExit("The Windows launcher must start using Windows system libraries alone.")
        libraries[binary.name] = linked
    return {
        "commit": revision, "version": version, "platform": platform.platform(), "operating_system": os_description(),
        "machine": platform.machine(), "managed_packages": system_packages().splitlines(),
        "runtime_linkage": libraries, "converters": converter_versions(),
        "corresponding_source": f"helloneedle-{version}-source-{revision}.tar.gz",
    }


def debian(stage, bindir, dist, version):
    # dpkg calculates ABI-specific minimum versions for the ELF binaries on this host.
    with tempfile.TemporaryDirectory() as temp:
        metadata = Path(temp) / "debian"
        metadata.mkdir()
        (metadata / "control").write_text(
            f"Source: helloneedle\nSection: utils\nPriority: optional\nMaintainer: Alfredo Llaquet-Alsina <{MAINTAINER}>\n"
            "\nPackage: helloneedle\nArchitecture: any\nDescription: Local desktop document search\n",
            encoding="utf-8")
        shlibs = output(["dpkg-shlibdeps", "-O", bindir / "helloneedle", bindir / "helloneedle-worker"],
                        cwd=temp)
    depends = next(line.partition("=")[2] for line in shlibs.splitlines() if line.startswith("shlibs:Depends="))
    arch = output(["dpkg", "--print-architecture"])
    control = stage / "DEBIAN"
    control.mkdir(exist_ok=True)
    (control / "control").write_text(
        f"Package: helloneedle\nVersion: {version}\nArchitecture: {arch}\nSection: utils\nPriority: optional\n"
        f"Maintainer: Alfredo Llaquet-Alsina <{MAINTAINER}>\nHomepage: {PROJECT_URL}\n"
        f"Depends: {depends}, qt6-qpa-plugins, qt6-translations-l10n, poppler-utils, antiword\n"
        "Description: Search and preview local documents with Hello Needle\n",
        encoding="utf-8")
    archive = dist / f"helloneedle_{version}_{arch}.deb"
    run(["dpkg-deb", "--root-owner-group", "--build", stage, archive])
    return archive


def freebsd(stage, dist, work, version):
    deps = {}
    for name in ("qt6-base", "qt6-translations", "poppler-utils", "antiword"):
        origin, installed = output(["pkg", "query", "%o %v", name]).split()
        deps[name] = {"origin": origin, "version": installed}
    package_files = {}
    for path in files_under(stage):
        package_files["/" + path.relative_to(stage).as_posix()] = {
            "uname": "root", "gname": "wheel", "perm": format(path.stat().st_mode & 0o777, "04o")}
    manifest = {
        "name": "helloneedle", "version": version, "origin": "deskutils/helloneedle", "prefix": "/usr/local",
        "abi": output(["pkg", "config", "ABI"]), "comment": "Search and preview local documents",
        "desc": "Desktop document search with a Qt interface and a Rust search worker.",
        "maintainer": MAINTAINER, "www": PROJECT_URL, "categories": ["deskutils"],
        "licenses": ["GPLv3+"], "licenselogic": "single", "deps": deps, "files": package_files,
    }
    spec = work / "freebsd-manifest.json"
    spec.write_text(json.dumps(manifest), encoding="utf-8")
    run(["pkg", "create", "-M", spec, "-r", stage, "-o", dist])
    return dist / f"helloneedle-{version}.pkg"


def build(args):
    work, dist = args.work.resolve(), args.output.resolve()
    work.mkdir(parents=True, exist_ok=True)
    dist.mkdir(parents=True, exist_ok=True)
    stage = work / "package-root"
    if stage.exists():
        shutil.rmtree(stage)
    prefix = stage / {"Linux": "usr", "FreeBSD": "usr/local", "Darwin": "", "Windows": ""}[SYSTEM]
    install_prefix = {"Linux": "/usr", "FreeBSD": "/usr/local", "Darwin": "/Applications",
                      "Windows": str(prefix)}[SYSTEM]
    run(["cmake", "-S", ROOT, "-B", work, "-G", "Ninja", "-DCMAKE_BUILD_TYPE=Release",
         "-DCMAKE_INSTALL_PREFIX=" + install_prefix])
    run(["cmake", "--build", work, "--parallel", "2"])
    run(["cargo", "test", "--locked", "--release", "--manifest-path", ROOT / "bridge/Cargo.toml",
         "--target-dir", work / "cargo"])
    run(["cmake", "--install", work, "--prefix", prefix])
    bindir, docs = layout(prefix)
    revision = output(["git", "rev-parse", "HEAD"], cwd=ROOT)
    cache = (work / "CMakeCache.txt").read_text(encoding="utf-8")
    version = next(line.partition("=")[2] for line in cache.splitlines() if line.startswith("CMAKE_PROJECT_VERSION:"))
    worker_version = output([bindir / ("helloneedle-worker" + EXE), "--version"])
    if worker_version != f"helloneedle-worker {version}":
        raise SystemExit(f"Worker reports {worker_version!r} for desktop version {version}.")
    metadata = environment(revision, version, bindir)
    metadata["application_icon"] = packaged_icons(work, prefix)
    (docs / "BUILD-ENVIRONMENT.json").write_text(json.dumps(metadata, indent=2) + "\n", encoding="utf-8")
    if SYSTEM == "Linux":
        archive = debian(stage, bindir, dist, version)
    elif SYSTEM == "FreeBSD":
        archive = freebsd(stage, dist, work, version)
    elif SYSTEM == "Darwin":
        bundle = prefix / "Hello Needle.app"
        run(["codesign", "--force", "--sign", "-", bindir / "helloneedle-worker"])
        run(["codesign", "--force", "--sign", "-", bundle])
        run(["codesign", "--verify", "--deep", "--strict", bundle])
        shutil.copy2(ROOT / "packaging/MACOS.txt", stage / "MACOS.txt")
        archive = dist / f"HelloNeedle-{version}-macOS-{platform.machine()}.zip"
        run(["ditto", "-c", "-k", stage, archive])
    else:
        archive = dist / f"HelloNeedle-{version}-Windows-x64-UCRT.zip"
        with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as package:
            for path in files_under(stage):
                package.write(path, path.relative_to(stage).as_posix())
    if not archive.is_file():
        raise SystemExit("Expected package was not created.")
    inventory = dist / "CONTENTS.sha256"
    inventory.write_text("".join(f"{sha256(p)}  {p.relative_to(stage).as_posix()}\n"
                                 for p in files_under(stage) if p.relative_to(stage).parts[0] != "DEBIAN"), encoding="utf-8")
    (dist / "SHA256SUMS").write_text(f"{sha256(archive)}  {archive.name}\n{sha256(inventory)}  {inventory.name}\n",
                                      encoding="utf-8")


def verify(args):
    bindir, docs = layout(args.prefix.resolve())
    worker, app = bindir / ("helloneedle-worker" + EXE), bindir / ("helloneedle" + EXE)
    evidence, tester = args.evidence.resolve(), args.tester.resolve()
    evidence.mkdir(parents=True, exist_ok=True)
    build_env = json.loads((docs / "BUILD-ENVIRONMENT.json").read_text(encoding="utf-8"))
    current = output(["git", "rev-parse", "HEAD"], cwd=ROOT)
    if SYSTEM == "Windows":
        app = args.prefix.resolve() / "Hello Needle.exe"
    if build_env["commit"] != current:
        raise SystemExit(f"Installed build {build_env['commit']} differs from test checkout {current}.")
    for path in [ROOT / "LICENSE", ROOT / "NOTICE.md", *files_under(ROOT / "LICENSES")]:
        if sha256(path) != sha256(docs / path.relative_to(ROOT)):
            raise SystemExit(f"Installed notice mismatch: {path.name}")
    if not (docs / "LICENSES/rust/DEPENDENCIES.md").is_file():
        raise SystemExit("Missing Rust inventory.")
    (evidence / "host.json").write_text(json.dumps(
        {"commit": current, "version": build_env["version"], "host": os_description(), "machine": platform.machine(),
         "platform": platform.platform(), "packages": system_packages().splitlines(), "converters": converter_versions()},
        indent=2) + "\n", encoding="utf-8")
    checks = {
        "installed-app": [app, "--smoke-test"],
        "document-search": [sys.executable, ROOT / "tests/desktop/worker_suite.py", "--worker", worker, "--require-doc"],
        "native-desktop": [sys.executable, ROOT / "tests/desktop/gui_matrix.py",
                           "--tester", tester, "--worker", worker, "--output", evidence],
    }
    failures = []
    with tempfile.TemporaryDirectory(prefix="Hello Needle installed test ") as temp:
        for name, command in checks.items():
            logfile = evidence / (name + ".log")
            env = os.environ.copy()
            if name == "installed-app" and SYSTEM == "Windows":
                # A double-click from Explorer has no MSYS2 shell or developer PATH.
                prefix = Path(shutil.which("pdftotext")).parent.parent
                default = Path(os.environ["SystemDrive"] + "/msys64/ucrt64")
                env.pop("HELLONEEDLE_UCRT64", None)
                if prefix != default:
                    env["HELLONEEDLE_UCRT64"] = str(prefix)
                env["PATH"] = os.pathsep.join([str(Path(os.environ["SystemRoot"]) / "System32"), os.environ["SystemRoot"]])
                env.pop("QT_PLUGIN_PATH", None)
            elif name == "installed-app" and SYSTEM == "Darwin":
                env["PATH"] = "/usr/bin:/bin:/usr/sbin:/sbin"
            try:
                with logfile.open("w", encoding="utf-8") as log:
                    run(command, stdout=log, stderr=subprocess.STDOUT, cwd=temp, timeout=1200, env=env)
            except (subprocess.SubprocessError, OSError) as exc:
                failures.append(name)
                print(f"{name}: {exc}", file=sys.stderr)
            if logfile.exists():
                print(logfile.read_text(encoding="utf-8", errors="replace"), flush=True)
    if failures:
        raise SystemExit("Failed installed checks: " + ", ".join(failures))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    actions = parser.add_subparsers(required=True)
    builder = actions.add_parser("build")
    builder.add_argument("--work", default=Path("build-qt"), type=Path)
    builder.add_argument("--output", default=Path("native-out/packages"), type=Path)
    builder.set_defaults(func=build)
    validator = actions.add_parser("verify")
    validator.add_argument("--prefix", required=True, type=Path)
    validator.add_argument("--tester", required=True, type=Path)
    validator.add_argument("--evidence", default=Path("native-out/evidence"), type=Path)
    validator.set_defaults(func=verify)
    args = parser.parse_args()
    args.func(args)


if __name__ == "__main__":
    main()
