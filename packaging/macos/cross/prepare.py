# SPDX-License-Identifier: GPL-3.0-or-later
"""Cross-build the Apple Silicon application skeleton on an Intel macOS CI host.

The original release build supplies moc/rcc output, artwork and the exact CMake
compile and link commands. Apple Clang recompiles the same translation units
against a signed arm64 MacPorts SDK. The application is never executed here.
"""

import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import subprocess

from archives import HOST_PREFIX, stage


ROOT = Path(__file__).resolve().parents[3]
CPU = "arm64"
RUST_TARGET = "aarch64-apple-darwin"


def output(*args, cwd=None):
    return subprocess.check_output(args, cwd=cwd, text=True).strip()


def require_cpus(path, expected):
    actual = set(output("xcrun", "lipo", "-archs", path).split())
    if actual != set(expected):
        raise ValueError(f"{path} contains {actual}; expected {set(expected)}")


def target_flags(args, prefix, deployment):
    result = []
    i = 0
    while i < len(args):
        arg = args[i]
        i += 1
        if arg in ("-arch", "-target", "--target"):
            i += 1
            continue
        if arg.startswith(("-mmacosx-version-min=", "--target=")):
            continue
        # Prefix paths can be separate arguments (-isystem, -iframework) or
        # joined options (-I, -L, -F). Preserve Apple SDK and source paths.
        for flag in ("", "-I", "-L", "-F", "-isystem", "-iframework"):
            if arg.startswith(flag + str(HOST_PREFIX) + "/"):
                arg = flag + str(prefix / Path(arg[len(flag):]).relative_to(HOST_PREFIX))
                break
        result.append(arg)
    return ["-arch", CPU, f"-mmacosx-version-min={deployment}", *result]


def link_args(build, ninja, name):
    target = str(Path("bin") / name)
    commands = output(ninja, "-C", build, "-t", "commands", target)
    for line in reversed(commands.splitlines()):
        parts = shlex.split(line)
        for i in range(len(parts) - 1):
            if parts[i] != "-o" or (build / parts[i + 1]).resolve() != (build / target).resolve():
                continue
            start = max((j + 1 for j in range(i) if parts[j] == "&&"), default=0)
            end = next((j for j in range(i + 2, len(parts)) if parts[j] == "&&"), len(parts))
            return parts[start + 1:end]
    raise ValueError(f"Missing native CMake linker invocation for {target}")


def cross_cpp(build, prefix, work, target_names, deployment):
    database = build / "compile_commands.json"
    if not database.is_file():
        subprocess.run(["cmake", "-S", ROOT, "-B", build, "-DCMAKE_EXPORT_COMPILE_COMMANDS=ON"], check=True)
    entries = json.loads(database.read_text())
    clang = output("xcrun", "--find", "clang++")
    obj_root = work / "objects"
    objects = {}
    lib_objects = []
    jobs = []
    for entry in entries:
        args = entry.get("arguments") or shlex.split(entry["command"])
        directory = Path(entry["directory"]).resolve()
        index = args.index("-o")
        native = (directory / args[index + 1]).resolve()
        relative = native.relative_to(build)
        if not (relative.parts[:2] == ("desktop", "qt") or
                (relative.parts[:2] == ("tests", "desktop") and "helloneedle-gui-tests.dir" in relative.parts)):
            continue
        if not any(part in ("helloneedle.dir", "helloneedle-desktop.dir", "helloneedle-gui-tests.dir")
                   or part.startswith("helloneedle-desktop_resources_") for part in relative.parts):
            continue
        dest = obj_root / relative
        dest.parent.mkdir(parents=True, exist_ok=True)
        args[index + 1] = str(dest)
        # A cross compilation must never replace the native dependency file.
        if "-MF" in args:
            args[args.index("-MF") + 1] = str(dest) + ".d"
        jobs.append(([clang, *target_flags(args[1:], prefix, deployment)], directory))
        objects[native] = dest
        if "helloneedle-desktop.dir" in relative.parts:
            lib_objects.append(dest)
    if not lib_objects:
        raise ValueError("CMake did not provide the Hello Needle desktop translation units")
    def compile_object(job):
        subprocess.run(job[0], cwd=job[1], check=True)
    with concurrent.futures.ThreadPoolExecutor(max_workers=2) as workers:
        list(workers.map(compile_object, jobs))
    library = work / "libhelloneedle-desktop.a"
    subprocess.run(["xcrun", "libtool", "-static", "-o", library, *lib_objects], check=True)
    (work / "bin").mkdir()
    ninja = next((line.partition("=")[2].strip() for line in (build / "CMakeCache.txt").read_text().splitlines()
                  if line.startswith("CMAKE_MAKE_PROGRAM:")), "ninja")
    for name in target_names:
        native = link_args(build, ninja, name)
        native[native.index("-o") + 1] = str(work / "bin" / name)
        for i, arg in enumerate(native):
            if arg.endswith(".o"):
                native[i] = str(objects[(build / arg).resolve()])
            elif Path(arg).name == library.name:
                native[i] = str(library)
            elif arg.startswith("-Wl,-dependency_info,"):
                native[i] = "-Wl,-dependency_info," + str(work / f"{name}.link.d")
        subprocess.run([clang, *target_flags(native, prefix, deployment)], cwd=build, check=True)
        require_cpus(work / "bin" / name, [CPU])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--build", type=Path, required=True, help="finished native CMake/Ninja build")
    parser.add_argument("--intel-app", type=Path, required=True, help="original installed application skeleton")
    parser.add_argument("--work", type=Path, required=True, help="new private cross-build directory")
    parser.add_argument("--output", type=Path, required=True, help="new arm64 application skeleton")
    args = parser.parse_args()
    if platform.system() != "Darwin" or platform.machine() != "x86_64":
        raise ValueError("This fallback requires an Intel macOS build host")
    build, intel, work, dest = (path.resolve() for path in (args.build, args.intel_app, args.work, args.output))
    require_cpus(intel / "Contents/MacOS/helloneedle", ["x86_64"])
    if dest.exists() or work.exists():
        raise ValueError("Specify new cross-build and output directories")
    darwin = platform.release().split(".")[0]
    deployment = platform.mac_ver()[0].split(".")[0] + ".0"
    work.mkdir(parents=True)
    prefix = stage(work, darwin)
    target_names = ["helloneedle", "helloneedle-gui-tests"]
    cross_cpp(build, prefix, work, target_names, deployment)

    subprocess.run(["rustup", "target", "add", RUST_TARGET], check=True)
    rust_dir = build / "cargo"
    env = os.environ.copy()
    env["MACOSX_DEPLOYMENT_TARGET"] = deployment
    env["CARGO_TARGET_AARCH64_APPLE_DARWIN_LINKER"] = output("xcrun", "--find", "clang")
    subprocess.run(["cargo", "build", "--locked", "--release", "--target", RUST_TARGET,
                    "--manifest-path", ROOT / "bridge/Cargo.toml", "--target-dir", rust_dir], env=env, check=True)
    worker = rust_dir / RUST_TARGET / "release/helloneedle-worker"
    require_cpus(worker, [CPU])
    dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copytree(intel, dest, symlinks=True, ignore=shutil.ignore_patterns("_CodeSignature"))
    binaries = dest / "Contents/MacOS"
    shutil.copy2(work / "bin/helloneedle", binaries / "helloneedle")
    shutil.copy2(worker, binaries / "helloneedle-worker")
    metadata = dest / "Contents/Resources/licenses/BUILD-ENVIRONMENT.json"
    info = json.loads(metadata.read_text())
    info.update(machine=CPU, platform=f"macOS-{deployment}-arm64", operating_system=f"macOS {deployment} target",
                build_host=platform.platform(), cross_compiled=True,
                runtime_linkage={path.name: output("xcrun", "otool", "-arch", CPU, "-L", path)
                                 for path in (binaries / "helloneedle", binaries / "helloneedle-worker")})
    metadata.write_text(json.dumps(info, indent=2) + "\n")
    print(f"Apple Silicon binaries prepared in {dest}. Runtime SDK: {prefix}", flush=True)


if __name__ == "__main__":
    main()
