# SPDX-License-Identifier: GPL-3.0-or-later
"""Copy the MacPorts runtime closure and rewrite all dyld references into the bundle."""
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess

SYSTEM = ("/System/Library/", "/usr/lib/")
LOADS = {"LC_LOAD_DYLIB", "LC_LOAD_WEAK_DYLIB", "LC_LOAD_UPWARD_DYLIB", "LC_REEXPORT_DYLIB", "LC_LAZY_LOAD_DYLIB"}
MAGIC = {bytes.fromhex(x) for x in ("feedface", "cefaedfe", "feedfacf", "cffaedfe",
                                     "cafebabe", "bebafeca", "cafebabf", "bfbafeca")}
ARCHS = ("arm64", "x86_64")


def run(*args, **options):
    return subprocess.run(args, check=True, **options)


def stdout(*args):
    return subprocess.check_output(args, text=True).strip()


def is_mach(path):
    if path.is_symlink() or not path.is_file():
        return False
    with path.open("rb") as binary:
        return binary.read(4) in MAGIC


def mach_files(app):
    return sorted(p for p in app.rglob("*") if is_mach(p))


def commands(binary, arch, minimum_versions=None):
    """Include weak/re-export loads, exclude the binary's own LC_ID_DYLIB."""
    libs, paths = [], []
    command = identifier = None
    for line in stdout("otool", "-arch", arch, "-l", binary).splitlines():
        key, _, value = line.strip().partition(" ")
        if key == "cmd":
            command = value.strip()
        elif key == "name":
            value = value.rsplit(" (offset ", 1)[0]
            if command in LOADS:
                libs.append(value)
            elif command == "LC_ID_DYLIB":
                identifier = value
        elif key == "path" and command == "LC_RPATH":
            paths.append(value.rsplit(" (offset ", 1)[0])
        elif minimum_versions is not None and ((command == "LC_BUILD_VERSION" and key == "minos") or
                                               (command == "LC_VERSION_MIN_MACOSX" and key == "version")):
            minimum_versions.add(tuple(int(piece) for piece in value.split(".")))
    return libs, paths, identifier


def expand(name, loader, executable):
    for token, path in (("@loader_path", loader.parent), ("@executable_path", executable.parent)):
        if name == token or name.startswith(token + "/"):
            return path / name[len(token):].lstrip("/")
    return Path(name)


def slices(path):
    return stdout("lipo", "-archs", path).split()


class Bundle:
    def __init__(self, app, prefix, cpu):
        self.app = app
        self.prefix = prefix
        self.qt = prefix / "libexec/qt6"
        self.cpu = cpu
        self.contents = app / "Contents"
        self.executables = self.contents / "MacOS"
        self.frameworks = self.contents / "Frameworks"
        self.resources = self.contents / "Resources"
        self.origins = {}
        self.system_imports = {}
        self.minimum_versions = set()
        self.work = []
        self.copied_frameworks = set()

    def copy(self, original, target):
        original = original.resolve(strict=True)
        if not original.is_relative_to(self.prefix):
            raise ValueError(f"Unaccounted non-MacPorts runtime file: {original}")
        if target in self.origins:
            if original != self.origins[target]:
                raise ValueError(f"Runtime filename collision: {target}, {original}, {self.origins[target]}")
            return
        self.origins[target] = original
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(original, target)
        if is_mach(target):
            target.chmod(0o755)
            archs = slices(target)
            if self.cpu not in archs:
                raise ValueError(f"{original} needs {self.cpu}, has {archs}")
            if len(archs) > 1:
                run("lipo", original, "-thin", self.cpu, "-output", target)
            self.work.append((original, target))

    def copy_data(self, source, dest):
        if not source.is_dir():
            raise ValueError(f"Missing runtime data directory {source}")
        for entry in sorted(source.rglob("*")):
            if entry.is_file() and "_CodeSignature" not in entry.parts:
                self.copy(entry, dest / entry.relative_to(source))

    def framework_target(self, original):
        framework = next((p for p in original.parents if p.suffix == ".framework"), None)
        if not framework:
            return self.frameworks / original.name
        # Preserve framework version and bundle metadata (Cocoa uses the
        # framework's Resources and signatures), without SDK headers/modules.
        relative = original.relative_to(framework)
        if len(relative.parts) != 3 or relative.parts[0] != "Versions":
            raise ValueError(f"Unknown framework layout: {original}")
        version, leaf = relative.parts[1:]
        target = self.frameworks / framework.name
        if target not in self.copied_frameworks:
            self.copied_frameworks.add(target)
            root = target / "Versions" / version
            root.mkdir(parents=True)
            (target / "Versions/Current").symlink_to(version)
            (target / leaf).symlink_to("Versions/Current/" + leaf)
            resources = original.parent / "Resources"
            self.copy_data(resources, root / "Resources")
            (target / "Resources").symlink_to("Versions/Current/Resources")
        return target / relative

    def original(self, dependency, loader, paths):
        def mapped(path):
            if path.is_relative_to("/opt/local"):
                return self.prefix / path.relative_to("/opt/local")
            return path

        if dependency.startswith("@rpath/"):
            rel = dependency[7:]
            # A dylib can inherit the load chain's run paths. Use the same
            # registered Qt/ports locations if its own LC_RPATH list is empty.
            dirs = [expand(p, loader, self.executables / "helloneedle") for p in paths]
            dirs += [self.qt / "lib", self.prefix / "lib", self.prefix / "Library/Frameworks", loader.parent]
            candidates = [mapped(p) / rel for p in dirs]
        else:
            candidates = [mapped(expand(dependency, loader, self.executables / "helloneedle"))]
        for path in candidates:
            if path.is_file():
                file = path.resolve()
                if file.is_relative_to(self.prefix):
                    return file
        raise ValueError(f"Unresolved non-system dependency {dependency} in {loader}: {candidates}")

    def deploy(self, manifest, pdf_tools):
        self.frameworks.mkdir(exist_ok=True)
        self.copy(self.prefix / "bin/antiword", self.executables / "antiword")
        for tool in ("pdftoppm", "pdftotext"):
            original, staged = pdf_tools / tool, self.executables / tool
            shutil.copy2(original, staged)
            self.work.append((original, staged))
        for plugin in ("platforms/libqcocoa.dylib", "styles/libqmacstyle.dylib"):
            self.copy(self.qt / "plugins" / plugin, self.contents / "PlugIns" / plugin)
        self.copy(self.qt / "translations/qtbase_es.qm", self.resources / "translations/qtbase_es.qm")
        self.copy_data(self.prefix / "share/antiword", self.resources / "share/antiword")
        self.copy_data(self.prefix / "share/poppler", self.resources / "share/poppler")
        (self.resources / "qt.conf").write_text("[Paths]\nPlugins=PlugIns\nTranslations=Resources/translations\n", encoding="ascii")
        config = self.resources / "fonts/fonts.conf"
        config.parent.mkdir(exist_ok=True)
        config.write_text('<?xml version="1.0"?><fontconfig>\n'
                          '  <dir>/System/Library/Fonts</dir>\n  <dir>/Library/Fonts</dir>\n'
                          '  <dir prefix="xdg">fonts</dir>\n  <cachedir prefix="xdg">fontconfig</cachedir>\n'
                          '</fontconfig>\n', encoding="ascii")

        plist = self.contents / "Info.plist"
        data = plistlib.loads(plist.read_bytes())
        data.pop("LSEnvironment", None)
        plist.write_bytes(plistlib.dumps(data))
        for name in ("helloneedle", "helloneedle-worker"):
            exe = self.executables / name
            self.work.append((exe, exe))
        while self.work:
            orig, binary = self.work.pop()
            loads, rpaths, identifier = commands(binary, self.cpu, self.minimum_versions)
            self.system_imports[binary.relative_to(self.app).as_posix()] = sorted(
                dep for dep in set(loads) if dep.startswith(SYSTEM))
            edits = []
            for dep in dict.fromkeys(loads):
                if dep.startswith(SYSTEM):
                    continue
                source = self.original(dep, orig, rpaths)
                target = self.framework_target(source)
                self.copy(source, target)
                # All executables and plugins can resolve every load independently
                # of the caller's PATH, inherited runpath stack or launcher.
                link = "@loader_path/" + os.path.relpath(target, binary.parent)
                if link != dep:
                    edits += ["-change", dep, link]
            if identifier:
                if binary.is_relative_to(self.frameworks):
                    key = "@rpath/" + binary.relative_to(self.frameworks).as_posix()
                else:
                    key = "@loader_path/" + binary.name
                if key != identifier:
                    edits += ["-id", key]
            for rpath in dict.fromkeys(rpaths):
                edits += ["-delete_rpath", rpath]
            if edits:
                run("install_name_tool", *edits, binary)
        manifest.write_text(json.dumps([{"installed": p.relative_to(self.app).as_posix(), "source": str(orig)}
                             for p, orig in sorted(self.origins.items())], ensure_ascii=False, indent=2) + "\n")
        if not self.minimum_versions:
            raise ValueError("No minimum macOS version found in bundled Mach-O load commands")
        minimum = ".".join(map(str, max(self.minimum_versions)))
        data["LSMinimumSystemVersion"] = minimum
        data["LSMinimumSystemVersionByArchitecture"] = {self.cpu: minimum}
        plist.write_bytes(plistlib.dumps(data))
        print(f"Mach-O closure and data: {len(self.origins)} source files indexed", flush=True)


def sign(app):
    contents = app / "Contents"
    info = plistlib.loads((contents / "Info.plist").read_bytes())
    main = contents / "MacOS" / info["CFBundleExecutable"]
    for binary in mach_files(app):
        # Signing the main executable implicitly processes its enclosing app.
        # Framework executables likewise receive their signature with their
        # framework. Sign ordinary nested code and helpers first.
        if binary != main and not any(p.suffix == ".framework" for p in binary.relative_to(app).parents):
            run("codesign", "--force", "--sign", "-", "--timestamp=none", binary)
    frameworks = (contents / "Frameworks").rglob("*.framework")
    for framework in sorted(frameworks, key=lambda path: len(path.parts), reverse=True):
        run("codesign", "--force", "--sign", "-", "--timestamp=none", framework)
    run("codesign", "--force", "--sign", "-", "--timestamp=none", app)


def verify(app, archs):
    # Temporary volumes may be reached through macOS's /var -> /private/var.
    # Compare canonical paths on both sides when checking bundle boundaries.
    app = app.resolve(strict=True)
    contents = app / "Contents"
    plist = plistlib.loads((contents / "Info.plist").read_bytes())
    if plist.get("CFBundleIdentifier") != "org.thecrumbs.HelloNeedle" or plist.get("LSEnvironment"):
        raise ValueError("Incorrect bundle identity or stale development environment")
    required = ["MacOS/helloneedle", "MacOS/helloneedle-worker", "MacOS/antiword", "MacOS/pdftotext", "MacOS/pdftoppm",
                "PlugIns/platforms/libqcocoa.dylib", "PlugIns/styles/libqmacstyle.dylib", "Resources/helloneedle.icns",
                "Resources/qt.conf", "Resources/translations/qtbase_es.qm", "Resources/share/antiword/UTF-8.txt",
                "Resources/share/antiword/fontnames", "Resources/share/poppler/cMap/Adobe-Japan1", "Resources/fonts/fonts.conf",
                "Resources/licenses/LICENSE", "Resources/licenses/NOTICE.md"]
    for item in required:
        if not (contents / item).exists():
            raise ValueError(f"Missing bundled runtime component: {item}")
    for path in app.rglob("*"):
        if path.is_symlink() and (not path.exists() or not path.resolve().is_relative_to(app)):
            raise ValueError(f"Escaping or broken bundle link: {path}")
    for binary in mach_files(app):
        if not set(archs).issubset(slices(binary)):
            raise ValueError(f"Missing {archs} slice in {binary}")
        for arch in archs:
            libraries, rpaths, _ = commands(binary, arch)
            for name in libraries:
                if name.startswith(SYSTEM):
                    continue
                path = expand(name, binary, contents / "MacOS/helloneedle").resolve()
                if not name.startswith("@loader_path/") or not path.is_file() or not path.is_relative_to(app):
                    raise ValueError(f"External or missing load command in {binary}: {name}")
            if rpaths:
                raise ValueError(f"Unexpected load paths in {binary}: {rpaths}")
    run("codesign", "--verify", "--deep", "--strict", "--all-architectures", "--verbose=2", app)
