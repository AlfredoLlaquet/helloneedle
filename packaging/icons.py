#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Assemble Windows and Finder icons from the same PNGs installed in hicolor."""
from pathlib import Path
import struct
import sys


def write_icons(directory):
    sizes = (16, 24, 32, 48, 64, 128, 256)
    finder = ((b"icp4", 16), (b"icp5", 32), (b"icp6", 64), (b"ic07", 128),
              (b"ic08", 256), (b"ic09", 512), (b"ic10", 1024), (b"ic11", 32),
              (b"ic12", 64), (b"ic13", 256), (b"ic14", 512))
    images = {size: (directory / f"{size}.png").read_bytes() for size in (*sizes, 512, 1024)}
    entries = []
    offset = 6 + 16 * len(sizes)
    for size in sizes:
        entries.append(struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32, len(images[size]), offset))
        offset += len(images[size])
    ico = struct.pack("<HHH", 0, 1, len(sizes)) + b"".join(entries) + b"".join(images[size] for size in sizes)
    (directory / "helloneedle.ico").write_bytes(ico)
    chunks = b"".join(struct.pack(">4sI", kind, 8 + len(images[size])) + images[size] for kind, size in finder)
    (directory / "helloneedle.icns").write_bytes(struct.pack(">4sI", b"icns", 8 + len(chunks)) + chunks)


if __name__ == "__main__":
    write_icons(Path(sys.argv[1]))
