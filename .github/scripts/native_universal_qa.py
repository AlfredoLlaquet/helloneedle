#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Verify the producing run, original installers and every corresponding-source file."""
import argparse
import json
from pathlib import Path
import re
import subprocess

from installer_artifacts import read_checksums, sha256

REPO = "AlfredoLlaquet/helloneedle"
REVISION = "3ec29a5166022d262ae4b31a7b9456501dc96e98"
RUN = 45282805114
STEM = "HelloNeedle-1.0.0-macOS"
PROJECT = "helloneedle-corresponding-source"
PINS = {
    PROJECT: (14869212779, "b2be3420ac20efa7e0fdc8e16fa2218349f5f09639f89bab65c8380957f1d9a5"),
    "helloneedle-macos-x86_64-installer": (14871860267, "6ff7ca01ed168bf434d28c5b7fa38eb2976f04eb3329a7ddc2302e1e78f618ed"),
    "helloneedle-macos-arm64-installer": (14870884280, "528bb10378a6e263c17764bbe1fb687e856969672cffcc73f7bc8fa5fa9e8a0a"),
    "helloneedle-macos-x86_64-runtime-sources": (14871955283, "3aa3ebac9a13f99c839300546fd71d1e08e62a0aaf482f8e6bbcca7a6a9d3aac"),
    "helloneedle-macos-arm64-runtime-sources": (14871441834, "f63c6e83cb6ac60783716a77376fe1a7eb49a5d721f2e497d3664392dc9c51f9"),
}


def api(path):
    url = f"repos/{REPO}/actions/runs/{RUN}{path}"
    return json.loads(subprocess.check_output(["gh", "api", url]))


def identity():
    run = api("")
    if (run["head_sha"] != REVISION or run["head_repository"]["full_name"] != REPO or
            run["event"] != "push" or run["head_branch"] != "hello-needle-installers" or
            run["path"] != ".github/workflows/helloneedle.yml"):
        raise ValueError("The source run does not match the native installer candidate.")
    artifacts = {row["name"]: row for row in api("/artifacts?per_page=100")["artifacts"]}
    for name, (artifact_id, digest) in PINS.items():
        actual = artifacts[name]
        if actual["id"] != artifact_id or actual["digest"] != "sha256:" + digest or actual["expired"]:
            raise ValueError(f"Unexpected artifact identity: {name}")
    return {"repository": REPO, "revision": REVISION, "run": RUN, "artifactIdsAndSha256": PINS}


def checked(folder):
    expected = read_checksums(folder / "SHA256SUMS")
    if not expected or {p.name for p in folder.iterdir()} != set(expected) | {"SHA256SUMS"}:
        raise ValueError(f"Unexpected archive contents in {folder}")
    for name, digest in expected.items():
        if (folder / name).is_symlink() or sha256(folder / name) != digest:
            raise ValueError(f"Source digest differs: {name}")
    return expected


def contents(folder):
    sources = checked(folder / PROJECT)
    if set(sources) != {f"helloneedle-1.0.0-source-{REVISION}.tar.gz"}:
        raise ValueError("Unexpected corresponding-source revision")
    hashes = dict(sources)
    for cpu in ("arm64", "x86_64"):
        native = "helloneedle-macos-" + cpu
        runtime = checked(folder / (native + "-runtime-sources"))
        prefix = f"{STEM}-{cpu}"
        if not all(re.fullmatch(re.escape(prefix) + r"-Runtime-Sources\.tar(?:\.part[0-9]{3})?", name) for name in runtime):
            raise ValueError(f"Unexpected {cpu} runtime source filename")
        product = folder / (native + "-installer")
        dmg = product / (prefix + ".dmg")
        sums = product / (prefix + "-SHA256SUMS")
        if set(product.iterdir()) != {dmg, sums} or any(p.is_symlink() for p in (dmg, sums)):
            raise ValueError(f"Unexpected {cpu} installer artifact content")
        declared = read_checksums(sums)
        measured = {**sources, **runtime, dmg.name: sha256(dmg)}
        if declared != measured:
            raise ValueError(f"{cpu} installer disagrees with its native source or image bytes")
        hashes.update(measured)
    return hashes


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("identity", "contents"))
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--inputs", type=Path, default=Path("inputs"))
    args = parser.parse_args()
    result = identity() if args.action == "identity" else contents(args.inputs)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
