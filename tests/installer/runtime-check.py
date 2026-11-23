#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check document searches and preview helpers in a finished desktop installation."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import zlib

DESKTOP_TESTS = Path(__file__).resolve().parents[1] / "desktop"
sys.path.insert(0, str(DESKTOP_TESTS))
from document_fixtures import write_pdf
from worker_suite import search, validate_occurrence


def document_hashes(directory):
    return {p.relative_to(directory).as_posix(): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in sorted(directory.rglob("*")) if p.is_file()}


def save_json(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


def prepare(directory, inventory):
    directory.mkdir(parents=True, exist_ok=True)
    if any(directory.iterdir()):
        raise ValueError("Installer fixtures require an empty test documents folder")
    write_pdf(directory / "report.pdf")
    shutil.copyfile(DESKTOP_TESTS / "fixtures/sample.doc", directory / "regular-word97.doc")
    shutil.copyfile(DESKTOP_TESTS / "fixtures/short-word97.doc", directory / "carta breu.doc")
    # Word for DOS 5.x stores the text after its 128-byte file header, uses a
    # little-endian end-of-text offset at 0x0e, and encodes the text as CP437.
    # This original fixture cannot take the worker's compound-file Word 97 path.
    text = "Archive note.\rThe legacy converter found a needle: café.\r".encode("cp437")
    header = bytearray(128)
    header[:6] = bytes.fromhex("31 be 00 00 00 ab")
    header[0x74] = 4
    struct.pack_into("<I", header, 0x0e, len(header) + len(text))
    (directory / "word-for-dos.doc").write_bytes((header + text).ljust(1024, b"\0"))
    save_json(inventory, document_hashes(directory))


def unchanged(directory, inventory):
    if document_hashes(directory) != json.loads(inventory.read_text(encoding="utf-8")):
        raise ValueError("The installation, search, or removal changed a test document")


def checked_hit(worker, docs, filename, term):
    hits, _ = search(worker, docs, term, pathPattern=filename)
    if not hits or any(Path(hit["path"]).name != filename for hit in hits):
        raise ValueError(f"Expected a match for {term!r} in {filename}: {hits!r}")
    for hit in hits:
        validate_occurrence(worker, hit, term)
    return hits


def has_page_text(png):
    """Count drawn glyph pixels in the text area of this specific PDF fixture."""
    if not png.startswith(b"\x89PNG\r\n\x1a\n"):
        return False
    cursor, packed = 8, bytearray()
    width = height = channels = 0
    while cursor + 12 <= len(png):
        size, kind = struct.unpack_from(">I4s", png, cursor)
        data = png[cursor + 8:cursor + 8 + size]
        if len(data) != size:
            return False
        if kind == b"IHDR":
            width, height, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", data)
            channels = {0: 1, 2: 3, 6: 4}.get(color, 0)
            if (width, height) != (320, 240) or depth != 8 or not channels or interlace:
                return False
        elif kind == b"IDAT":
            packed.extend(data)
        elif kind == b"IEND":
            break
        cursor += size + 12
    if not width or not channels or not packed:
        return False
    stride = width * channels
    raw = zlib.decompress(packed)
    if len(raw) != height * (stride + 1):
        return False
    previous = bytearray(stride)
    glyph_pixels = paper_pixels = 0
    for y in range(height):
        pos = y * (stride + 1)
        filter_kind, line = raw[pos], bytearray(raw[pos + 1:pos + stride + 1])
        if filter_kind > 4:
            return False
        for i in range(stride):
            left = line[i - channels] if i >= channels else 0
            up = previous[i]
            corner = previous[i - channels] if i >= channels else 0
            if filter_kind == 4:
                predicted = left + up - corner
                sides = (left, up, corner)
                correction = min(sides, key=lambda v: abs(predicted - v))
            else:
                correction = (0, left, up, (left + up) // 2)[filter_kind]
            line[i] = (line[i] + correction) & 255
        if 45 <= y < 90:
            for x in range(24, 300):
                pixel = line[x * channels:x * channels + min(3, channels)]
                glyph_pixels += max(pixel) < 140
        elif 120 <= y < 160:
            for x in range(24, 300):
                pixel = line[x * channels:x * channels + min(3, channels)]
                paper_pixels += min(pixel) > 245
        previous = line
    return 80 <= glyph_pixels < 2200 and paper_pixels >= 10_800


def verify(args):
    docs, worker = args.documents.resolve(), args.worker.resolve(strict=True)
    unchanged(docs, args.inventory)
    hits = checked_hit(worker, docs, "report.pdf", "Needle")
    if len(hits) != 2 or any(hit["location"].get("page") != 2 for hit in hits):
        raise ValueError("Poppler did not find both words on the second page")
    renderer = args.pdf_renderer.resolve(strict=True)
    env = os.environ.copy()
    if sys.platform == "win32" or sys.platform == "darwin":
        # Use the installed font configuration used by the desktop's PDF pane.
        prefix = renderer.parents[1]
        config = prefix / ("Resources/fonts/fonts.conf" if sys.platform == "darwin" else "etc/fonts/fonts.conf")
        if config.is_file():
            env.update(FONTCONFIG_FILE=str(config), FONTCONFIG_PATH=str(config.parent))
    rendered = subprocess.run([str(renderer), "-f", "2", "-l", "2",
                               "-singlefile", "-scale-to", "320", "-png", str(docs / "report.pdf")],
                              check=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=45, env=env)
    image = rendered.stdout
    if not has_page_text(image):
        raise ValueError("Bundled PDF renderer did not paint the expected text within the 320 by 240 page")
    args.evidence.parent.mkdir(parents=True, exist_ok=True)
    args.evidence.with_suffix(".png").write_bytes(image)
    checked_hit(worker, docs, "regular-word97.doc", "Needle in the margin")
    checked_hit(worker, docs, "carta breu.doc", "agulla 🧵")
    checked_hit(worker, docs, "word-for-dos.doc", "needle: café")
    unchanged(docs, args.inventory)
    save_json(args.evidence, {"worker": str(worker), "renderer": str(args.pdf_renderer),
                              "pdfPage": 2, "pdfMatches": 2, "pdfPng": [320, 240],
                              "word97": ["ordinary stream", "short stream"],
                              "wordForDos": "CP437 cafe with accent decoded through packaged antiword",
                              "originalDocuments": document_hashes(docs)})
    print("Installed PDF search and page rendering, native Word 97 and converted Word for DOS passed.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="action", required=True)
    for action in ("prepare", "verify", "unchanged"):
        command = commands.add_parser(action)
        command.add_argument("--documents", type=Path, required=True)
        command.add_argument("--inventory", type=Path, required=True)
        if action == "verify":
            command.add_argument("--worker", type=Path, required=True)
            command.add_argument("--pdf-renderer", type=Path, required=True)
            command.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args()
    if args.action == "prepare":
        prepare(args.documents, args.inventory)
    elif args.action == "unchanged":
        unchanged(args.documents, args.inventory)
    else:
        verify(args)


if __name__ == "__main__":
    main()
