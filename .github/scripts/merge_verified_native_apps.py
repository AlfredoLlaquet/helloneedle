#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Normalize the verified cow icons of this exact native pair and run the original Universal merger."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "packaging/macos"))
from package import app_from_disk, verify

MASTER = "530e7daf8cf7e59c14caa6941cbd8936539bdcacf7d747904fb5ea2c6bbe0363"
ICON_HASHES = {
    "arm64": "da5eb5c7fd685b39cbd3c8081fed3c729730b4da19bc67fd8d26f3277fa8a63c",
    "x86_64": "08f611691a60e18ace5f41b120a528145cb5fc264b4d7e980fb80b3bbade8331",
}
ICON = Path("Contents/Resources/helloneedle.icns")
FRAMES = {b"icp4": 16, b"icp5": 32, b"icp6": 64, b"ic07": 128, b"ic08": 256, b"ic09": 512,
          b"ic10": 1024, b"ic11": 32, b"ic12": 64, b"ic13": 256, b"ic14": 512}


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def frames(icon):
    data = icon.read_bytes()
    if data[:4] != b"icns" or int.from_bytes(data[4:8], "big") != len(data):
        raise ValueError(f"Invalid ICNS header in {icon}")
    pictures = {}
    offset = 8
    while offset < len(data):
        kind, length = struct.unpack_from(">4sI", data, offset)
        if kind in pictures or length < 32 or offset + length > len(data):
            raise ValueError(f"Invalid ICNS entry in {icon}")
        png = data[offset + 8:offset + length]
        if not png.startswith(b"\x89PNG\r\n\x1a\n") or png[12:16] != b"IHDR":
            raise ValueError(f"Expected a PNG frame in {icon}")
        width, height = struct.unpack_from(">II", png, 16)
        if (width, height) != (FRAMES.get(kind), FRAMES.get(kind)):
            raise ValueError(f"Unexpected ICNS frame size in {icon}")
        pictures[kind] = (width, height)
        offset += length
    if pictures.keys() != FRAMES.keys():
        raise ValueError(f"Incomplete standard and Retina ICNS frames in {icon}")


def merge(args):
    master = ROOT / "desktop/qt/helloneedle.png"
    if digest(master) != MASTER:
        raise ValueError("The committed master differs from the approved cow artwork for this run.")
    with tempfile.TemporaryDirectory(prefix="Native CPU application copies ") as temp:
        work = Path(temp)
        apps = {}
        for cpu, image in (("arm64", args.arm), ("x86_64", args.intel)):
            with app_from_disk(image) as original:
                verify(original, [cpu])
                if digest(original / ICON) != ICON_HASHES[cpu]:
                    raise ValueError(f"Unknown input icon in the {cpu} native product")
                frames(original / ICON)
                copy = work / cpu / "Hello Needle.app"
                copy.parent.mkdir()
                subprocess.run(["ditto", original, copy], check=True)
                apps[cpu] = copy
        # These exact native frames were independently decoded and compared against
        # the committed master's Qt renderings. Their maximum RGBA difference was
        # one 8-bit level. Regenerate a single Finder asset from that master.
        sizes = work / "master-icon"
        sizes.mkdir()
        for size in (16, 24, 32, 48, 64, 128, 256, 512, 1024):
            subprocess.run(["/usr/bin/sips", "--resampleHeightWidth", str(size), str(size), "-s", "format",
                            "png", master, "--out", sizes / f"{size}.png"], check=True, stdout=subprocess.DEVNULL)
        subprocess.run([sys.executable, ROOT / "packaging/icons.py", sizes], check=True)
        canonical = sizes / "helloneedle.icns"
        frames(canonical)
        for copy in apps.values():
            shutil.copyfile(canonical, copy / ICON)
        args.evidence.parent.mkdir(parents=True, exist_ok=True)
        args.evidence.write_text(json.dumps({"sourceMasterSha256": MASTER, "inputIconSha256": ICON_HASHES,
            "universalIconSha256": digest(canonical), "frames": {k.decode(): n for k, n in FRAMES.items()}},
            indent=2) + "\n", encoding="utf-8")
        subprocess.run([sys.executable, "-u", ROOT / "packaging/macos/package.py", "universal", "--arm",
                        apps["arm64"], "--intel", apps["x86_64"], "--output", args.output], check=True)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("arm", "intel", "output", "evidence"):
        parser.add_argument("--" + name, type=Path, required=True)
    merge(parser.parse_args())
