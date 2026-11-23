#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Regenerate the committed Windows and Finder icons from the application's SVG with Inkscape."""
from pathlib import Path
from tempfile import TemporaryDirectory
import struct
import subprocess

directory = Path(__file__).resolve().parent
vector = directory.parent / "desktop" / "qt" / "helloneedle.svg"
with TemporaryDirectory() as temp:
    images = {}
    for size in (16, 32, 48, 64, 128, 256, 512, 1024):
        image = Path(temp) / f"{size}.png"
        subprocess.run(["inkscape", str(vector), "--export-width", str(size), "--export-height",
                        str(size), "--export-filename", str(image)], check=True)
        images[size] = image.read_bytes()

sizes = [16, 32, 48, 64, 128, 256]
entries = []
offset = 6 + 16 * len(sizes)
for size in sizes:
    entries.append(struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(images[size]), offset))
    offset += len(images[size])
(directory / "helloneedle.ico").write_bytes(struct.pack("<HHH", 0, 1, len(sizes)) +
                                              b"".join(entries) + b"".join(images[size] for size in sizes))
chunks = [(b"ic07", 128), (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024),
          (b"ic11", 32), (b"ic12", 64), (b"ic13", 256), (b"ic14", 512)]
payload = b"".join(struct.pack(">4sI", kind, 8 + len(images[size])) + images[size] for kind, size in chunks)
(directory / "helloneedle.icns").write_bytes(struct.pack(">4sI", b"icns", 8 + len(payload)) + payload)
