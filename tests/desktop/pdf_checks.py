# SPDX-License-Identifier: GPL-3.0-or-later
"""Exercise PDF search and selected-page preview through the installed worker."""
import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys


def exchange(worker, operation, request, env=None):
    reply = subprocess.run([worker, operation], input=json.dumps(request), env=env,
                           stdout=subprocess.PIPE, encoding="utf-8", check=True, timeout=240)
    return [json.loads(line) for line in reply.stdout.splitlines()]


def search(worker, document, query, env=None):
    return exchange(worker, "search", {"folder": str(document.parent),
                    "pathPattern": document.name, "query": query}, env)


def preview(worker, hit, env=None):
    events = exchange(worker, "preview", {"path": hit["path"], "location": hit["location"],
                      "blockStart": hit["blockStart"], "revision": hit["revision"]}, env)
    event = next(e for e in events if e["type"] == "preview")
    block = event["blocks"][event["index"]]
    assert block["location"]["index"] == hit["location"]["index"]
    data = block["text"].encode("utf-16-le")
    offset = hit["blockStart"] - block["location"].get("textOffset", 0)
    selected = data[2 * offset:2 * (offset + hit["blockLength"])].decode("utf-16-le")
    return block, selected


def synthetic_pdf(path, pages):
    # Use shared content streams. Even the book whose bounding boxes exceed
    # 32 MiB is a small generated file, with a functioning last-page cross-ref.
    rows = b"\n".join(b"(" + b"word " * 40 + b") Tj T*" for _ in range(32))
    streams = {
        "dense": b"BT /F 6 Tf 10 TL 10 720 Td\n" + rows + b"\nET\n",
        "blank": b"",
        "final": b"BT /F 10 Tf 10 720 Td (LastNeedle LastNeedle) Tj ET\n",
    }
    kids = " ".join(f"{7+i} 0 R" for i in range(len(pages)))
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>",
               f"<< /Type /Pages /Count {len(pages)} /Kids [{kids}] >>".encode(),
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    for stream in streams.values():
        objects.append(f"<< /Length {len(stream)} >>\nstream\n".encode() + stream + b"endstream")
    for kind in pages:
        stream_id = 4 + list(streams).index(kind)
        objects.append((f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 800 800] "
                        f"/Resources << /Font << /F 3 0 R >> >> /Contents {stream_id} 0 R >>").encode())
    with path.open("wb") as out:
        out.write(b"%PDF-1.4\n")
        offsets = [0]
        for i, obj in enumerate(objects, 1):
            offsets.append(out.tell())
            out.write(f"{i} 0 obj\n".encode() + obj + b"\nendobj\n")
        xref = out.tell()
        out.write(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
        for offset in offsets[1:]:
            out.write(f"{offset:010d} 00000 n \n".encode())
        out.write(f"trailer\n<< /Root 1 0 R /Size {len(offsets)} >>\nstartxref\n{xref}\n%%EOF\n".encode())


def mock_decoder(args):
    # Test only the parent process's handling of an abrupt converter failure.
    # Disable core dumps so the temporary test leaves no diagnostic binary.
    import resource
    resource.setrlimit(resource.RLIMIT_CORE, (0, 0))
    first = int(args[args.index("-f") + 1]) if "-f" in args else None
    if "-bbox-layout" in args:
        line = ('<line><word xMin="0" yMin="0" xMax="10" yMax="10">Needle</word></line>')
        page = '<page width="100" height="100">' + line + '</page>'
        if first is not None:
            sys.stdout.write("<doc>" + page + "</doc>")
            return 0
        sys.stdout.write("<doc>" + page + '<page width="100" height="100">' + line)
        sys.stdout.flush()
        print("mock layout failure", file=sys.stderr, flush=True)
        os.abort()
    if os.environ.get("HN_TEST_FAIL_PLAIN"):
        print("mock plain failure", file=sys.stderr)
        return 2
    pages = ["Needle", "Prefix Needle plus Needle", ""]
    selected = [pages[first - 1]] if first else pages
    sys.stdout.write("".join(p + "\n\f" for p in selected))
    return 0


def verify_fallback(worker, root):
    # Copy the worker so that sibling-tool discovery also exercises this
    # wrapper on a native installation with pdftotext in the normal bin dir.
    tool_dir = root / "isolated"
    tool_dir.mkdir()
    worker = shutil.copy2(worker, tool_dir)
    wrapper = tool_dir / "pdftotext"
    wrapper.write_text("#!/bin/sh\nexec " + shlex.quote(sys.executable) + " "
                       + shlex.quote(str(Path(__file__).resolve())) + ' --mock "$@"\n')
    wrapper.chmod(0o755)
    sample = root / "partial.pdf"
    synthetic_pdf(sample, ["final", "final", "blank"])
    events = search(worker, sample, "Needle")
    hits = [e for e in events if e["type"] == "match"]
    assert events[-1]["count"] == 3 and events[-1]["errors"] == 0, events[-1]
    assert [h["location"]["page"] for h in hits] == [1, 2, 2]
    assert [h["location"].get("pdfPlain", False) for h in hits] == [False, True, True]
    view, selected = preview(worker, hits[2])
    assert selected == "Needle" and view["location"]["pdfPlain"] and "words" not in view["location"]
    assert hits[1]["blockStart"] != hits[2]["blockStart"]
    failed = search(worker, sample, "missing", dict(os.environ, HN_TEST_FAIL_PLAIN="1"))
    errors = [e for e in failed if e["type"] == "error"]
    assert len(errors) == 1 and failed[-1]["errors"] == 1, failed
    for phrase in ("SIGABRT", "mock layout failure", "code 2", "mock plain failure"):
        assert phrase in errors[0]["message"], errors
    print("Partial-page layout crash, unduplicated plain retry, preview, and both diagnostics passed")


def verify_pdf_streaming(worker, root):
    root.mkdir()
    # Ficha.pdf is the public form Alfredo authorized for this regression.
    ficha = Path(__file__).with_name("fixtures") / "Ficha.pdf"
    events = search(worker, ficha, "fundación")
    hits = [e for e in events if e["type"] == "match"]
    assert len(hits) == 4 and events[-1]["errors"] == 0, events[-1]
    view, selected = preview(worker, hits[-1])
    assert selected.casefold() == "fundación" and view["location"]["page"] == 1
    print("Public Ficha.pdf searched and previewed using",
          "plain-text recovery" if view["location"].get("pdfPlain") else "word geometry")
    empty = root / "empty.pdf"
    synthetic_pdf(empty, ["blank", "blank"])
    assert search(worker, empty, "absent")[-1]["errors"] == 0
    for dense in (79, 400):
        path = root / f"book-{dense}.pdf"
        synthetic_pdf(path, ["dense"] * dense + ["blank", "final"])
        events = search(worker, path, "LastNeedle")
        hits = [e for e in events if e["type"] == "match"]
        assert len(hits) == 2 and events[-1]["errors"] == 0, events[-1]
        assert all(h["location"]["page"] == dense + 2 for h in hits)
        view, selected = preview(worker, hits[-1])
        assert selected == "LastNeedle" and view["location"]["pageCount"] == dense + 2
        assert len(view["location"]["words"]) == 2 and hits[0]["blockStart"] != hits[1]["blockStart"]
    print("PDFs with 101,120 and 512,000 body words, trailing matches, and empty pages passed")
    if os.name == "posix":
        verify_fallback(worker, root)


if __name__ == "__main__" and sys.argv[1:2] == ["--mock"]:
    raise SystemExit(mock_decoder(sys.argv[2:]))
