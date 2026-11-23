# SPDX-License-Identifier: GPL-3.0-or-later
"""Build Finder-relocatable PDF tools and stage their exact corresponding source."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile

VERSION = "26.02.0"
ARCHIVE = f"poppler-{VERSION}.tar.xz"
DIGEST = "dded8621f7b2f695c91063aab1558691c8418374cd583501e89ed39487e7ab77"
URL = f"https://poppler.freedesktop.org/{ARCHIVE}"
HERE = Path(__file__).resolve().parent


def build(work, sources, notices, prefix, cpu):
    recipe = sources / f"hello-needle-poppler-{VERSION}"
    recipe.mkdir(parents=True, exist_ok=True)
    archive = recipe / ARCHIVE
    if not archive.exists():
        with tempfile.TemporaryDirectory(dir=recipe) as temp:
            downloaded = Path(temp) / ARCHIVE
            subprocess.run(["curl", "--fail", "--location", "--retry", "3", "--output", downloaded, URL], check=True)
            if hashlib.sha256(downloaded.read_bytes()).hexdigest() != DIGEST:
                raise ValueError("Poppler source archive failed SHA-256 verification")
            shutil.move(downloaded, archive)
    if hashlib.sha256(archive.read_bytes()).hexdigest() != DIGEST:
        raise ValueError("Invalid cached Poppler source")
    src = work / f"poppler-{VERSION}"
    if src.exists():
        shutil.rmtree(src)
    # The tar is an exact digest-pinned upstream source distribution.
    with tarfile.open(archive, "r:xz") as packed:
        if not all((member.isdir() or member.isfile()) and ".." not in Path(member.name).parts and
                   (member.name.startswith(f"poppler-{VERSION}/") or member.name == f"poppler-{VERSION}")
                   for member in packed):
            raise ValueError("Unexpected source archive layout")
        for member in packed.getmembers():
            path = work / member.name
            if member.isdir():
                path.mkdir(parents=True, exist_ok=True)
            else:
                path.parent.mkdir(parents=True, exist_ok=True)
                with packed.extractfile(member) as original, path.open("wb") as target:
                    shutil.copyfileobj(original, target)
                path.chmod(member.mode & 0o755)
    patch = HERE / "poppler-bundle-data.patch"
    subprocess.run(["patch", "-p1", "-i", patch], cwd=src, check=True)
    out = work / "poppler-target"
    settings = ["-DENABLE_QT5=OFF", "-DENABLE_QT6=OFF", "-DENABLE_CPP=OFF", "-DENABLE_GLIB=OFF",
                "-DENABLE_BOOST=OFF", "-DENABLE_GOBJECT_INTROSPECTION=OFF", "-DENABLE_NSS3=OFF",
                "-DENABLE_GPGME=OFF", "-DENABLE_LIBCURL=OFF", "-DBUILD_SHARED_LIBS=OFF",
                "-DBUILD_GTK_TESTS=OFF", "-DBUILD_CPP_TESTS=OFF", "-DBUILD_QT5_TESTS=OFF",
                "-DBUILD_QT6_TESTS=OFF", "-DBUILD_MANUAL_TESTS=OFF", "-DENABLE_UTILS=ON",
                "-DCMAKE_BUILD_TYPE=Release", "-DCMAKE_EXE_LINKER_FLAGS=-Wl,-headerpad_max_install_names",
                f"-DCMAKE_OSX_ARCHITECTURES={cpu}", f"-DCMAKE_PREFIX_PATH={prefix}",
                "-DCMAKE_FIND_USE_PACKAGE_REGISTRY=OFF"]
    if prefix != Path("/opt/local"):
        # The private MacPorts prefix provides target libraries. The C++
        # standard library headers and Apple frameworks come from Xcode's SDK.
        sdk = os.environ.get("SDKROOT") or subprocess.check_output(
            ["xcrun", "--sdk", "macosx", "--show-sdk-path"], text=True).strip()
        settings.append(f"-DCMAKE_OSX_SYSROOT={sdk}")
    if os.environ.get("CMAKE_OSX_DEPLOYMENT_TARGET"):
        settings.append("-DCMAKE_OSX_DEPLOYMENT_TARGET=" + os.environ["CMAKE_OSX_DEPLOYMENT_TARGET"])
    subprocess.run(["cmake", "-S", src, "-B", out, "-G", "Ninja", *settings], check=True)
    subprocess.run(["cmake", "--build", out, "--target", "pdftotext", "pdftoppm", "--parallel", "3"], check=True)
    for item in (patch, Path(__file__).resolve(), HERE / "macho.py", HERE / "package.py"):
        shutil.copy2(item, recipe / item.name)
    # The complete input, downstream patch and command flags accompany each native slice.
    (recipe / "BUILD.txt").write_text(f"Original source: {URL}\nSHA-256: {DIGEST}\n"
        f"Architecture: {cpu}\ncmake settings:\n" + "\n".join(settings) +
        "\nBuild pdftoppm and pdftotext with bundled-library closure as in package.py.\n", encoding="utf-8")
    licenses = notices / f"poppler-tools-{VERSION}"
    licenses.mkdir(parents=True, exist_ok=True)
    for name in ("COPYING", "COPYING3", "AUTHORS"):
        shutil.copy2(src / name, licenses / name)
    shutil.copy2(src / "cmake/modules/COPYING-CMAKE-SCRIPTS", recipe / "COPYING-CMAKE-SCRIPTS")
    (licenses / "MODIFICATIONS.txt").write_text(
        f"The PDF tools incorporate Poppler {VERSION} and its contributors' work.\n"
        "On 2026-11-23 Hello Needle changed the macOS PDF converter data lookup to find "
        "Resources/share/poppler beside its installed executable. The source, patch and build recipe "
        "are included in the native runtime source archive distributed with this disk image.\n", encoding="utf-8")
    return out / "utils"
