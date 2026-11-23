"""Exercise the installed Rust worker over the same JSON protocol used by Qt."""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
from tempfile import TemporaryDirectory

from document_fixtures import write_documents


def exchange(executable, operation, payload, allow_errors=False, env=None):
    result = subprocess.run([str(executable), operation], input=json.dumps(payload).encode() + b"\n",
                            capture_output=True, timeout=45, env=env)
    events = [json.loads(line) for line in result.stdout.splitlines() if line.strip()]
    assert events, (operation, result.returncode, result.stderr.decode(errors="replace"))
    if not allow_errors:
        assert result.returncode == 0, (operation, events[-3:], result.stderr)
        assert not [event for event in events if event["type"] == "error"], (operation, events)
    return events


def search(executable, folder, query, *, allow_errors=False, **options):
    payload = {"folder": str(folder), "query": query, "mode": "document", **options}
    events = exchange(executable, "search", payload, allow_errors)
    matches = [event for event in events if event["type"] == "match"]
    if not allow_errors:
        assert events[-1]["type"] == "done" and events[-1]["count"] == len(matches), events[-1]
    return matches, [event for event in events if event["type"] == "error"]


def utf16_slice(string, start, length):
    assert start >= 0 and length >= 0, (start, length)
    data = string.encode("utf-16-le")
    assert (start + length) * 2 <= len(data), (string, start, length)
    return data[start * 2:(start + length) * 2].decode("utf-16-le")


def preview(executable, hit, mode="document"):
    request = {"path": hit["path"], "mode": mode, "location": hit["location"]}
    if "pathBytes" in hit:
        request["pathBytes"] = hit["pathBytes"]
    events = exchange(executable, "preview", request)
    return next(e for e in events if e["type"] == "preview")


def validate_occurrence(executable, hit, expected):
    assert utf16_slice(hit["text"], hit["start"], hit["length"]) == expected, hit
    view = preview(executable, hit)
    selected = view["blocks"][view["index"]]
    assert selected["location"]["index"] == hit["location"]["index"], (hit, selected)
    actual = utf16_slice(selected["text"], hit["blockStart"], hit["blockLength"])
    assert actual == expected, (hit, selected)
    return view


def hashes(root):
    return {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in root.iterdir() if p.is_file()}


def verify_documents(worker, root):
    write_documents(root)
    originals = hashes(root)
    hits, errors = search(worker, root, "Needle", allow_errors=True)
    per_file = {}
    for hit in hits:
        per_file.setdefault(Path(hit["path"]).name, []).append(hit)
    expected = set(originals)
    assert not errors, errors
    assert set(per_file) == expected, (set(per_file), expected, errors)
    for matches in per_file.values():
        for hit in matches:
            validate_occurrence(worker, hit, "Needle")

    line = per_file["notes.txt"]
    assert sorted(hit["blockStart"] for hit in line) == [3, 15], line
    assert all(hit["location"]["line"] == 3 for hit in line), line
    pdf = per_file["report.pdf"]
    assert len(pdf) == 2 and all(hit["location"]["page"] == 2 for hit in pdf), pdf
    assert len({hit["blockStart"] for hit in pdf}) == 2, pdf
    pdf_view = preview(worker, pdf[0])
    assert pdf_view["blocks"][pdf_view["index"]]["location"].get("words"), "PDF word coordinates missing"
    for name in ("letter.docx", "letter.odt", "letter.fodt"):
        blocks = [b["text"] for b in preview(worker, per_file[name][0])["blocks"]]
        index = blocks.index("Needle in the margin.")
        assert blocks[index - 1] == "Before the passage." and blocks[index + 1] == "After the passage.", (name, blocks)
    for name in ("sheet.xlsx", "sheet.ods"):
        loc = per_file[name][0]["location"]
        assert loc.get("sheet") == "Quarterly" and loc.get("cell") == "C7", (name, loc)
    for name in ("talk.pptx", "talk.odp"):
        assert per_file[name][0]["location"].get("slide") == 2, (name, per_file[name])
    assert hashes(root) == originals, "Search or preview modified a fixture"
    print(f"{len(hits)} precise occurrences searched and previewed across {len(per_file)} formats")


def verify_small_word(worker, root):
    root.mkdir()
    sample = root / "búsqueda – carta breve.doc"
    shutil.copyfile(Path(__file__).with_name("fixtures") / "short-word97.doc", sample)
    # Keep native Windows DLLs discoverable beside the worker. No converter on
    # PATH is needed to decode a Word 97 compound file with short streams.
    environment = dict(os.environ, PATH="")
    events = exchange(worker, "search", {"folder": str(root), "query": "agulla 🧵"}, env=environment)
    hits = [e for e in events if e["type"] == "match"]
    assert len(hits) == 1, events
    hit = hits[0]
    assert utf16_slice(hit["text"], hit["start"], hit["length"]) == "agulla 🧵"
    answer = exchange(worker, "preview", {"path": str(sample), "location": hit["location"]}, env=environment)
    blocks = next(event["blocks"] for event in answer if event["type"] == "preview")
    assert [block["text"] for block in blocks] == [
        "Un café en la plaça.", "L’àvia escriu amb una agulla 🧵.", "Mañana habrá otra página."
    ], blocks
    print("Small Word 97 stream, accented paragraphs and Unicode match passed with an empty converter PATH")


def verify_modes(worker, root):
    def find(term, **options):
        # Unavailable legacy readers report a per-file diagnostic. Restrict to markup for these checks.
        return search(worker, root, term, **options)[0]
    root.mkdir()
    with TemporaryDirectory() as temporary:
        source = Path(temporary)
        write_documents(source)
        for name in ("readme.md", "page.html"):
            shutil.copy2(source / name, root / name)
    assert {Path(h["path"]).name for h in find("The Latest Batch")} == {"page.html", "readme.md"}
    assert not find("The Latest Batch", mode="source")
    source = find("<strong>", mode="source")
    assert len(source) == 1 and source[0]["location"]["line"] == 1, source
    assert not find("<strong>")
    assert not find("forbiddenScriptMarker")
    assert len(find("forbiddenScriptMarker", mode="source")) == 1
    # The rich rendition keeps a local bold span and excludes script material.
    rich = preview(worker, find("The Latest Batch")[0])
    rendered = " ".join(b.get("html") or "" for b in rich["blocks"]).lower()
    assert ("<strong" in rendered or "<b>" in rendered) and "<script" not in rendered, rendered
    print("Document and Source searches, hidden script text, and inline formatted passages passed")


def verify_options_and_recovery(worker, root):
    root.mkdir()
    (root / "ordinary.txt").write_text("needle\nNeedle\nend", encoding="utf-8")
    (root / ".concealed.txt").write_text("hidden-marker", encoding="utf-8")
    (root / ".ignore").write_text("ignored.txt\n", encoding="utf-8")
    (root / "ignored.txt").write_text("ignore-marker", encoding="utf-8")
    assert len(search(worker, root, "needle")[0]) == 2
    assert len(search(worker, root, "needle", caseSensitive=True)[0]) == 1
    assert len(search(worker, root, "^n(e{2})dle$", regex=True)[0]) == 2
    assert not search(worker, root, "hidden-marker")[0]
    assert len(search(worker, root, "hidden-marker", hidden=True)[0]) == 1
    assert not search(worker, root, "ignore-marker")[0]
    assert len(search(worker, root, "ignore-marker", ignored=True)[0]) == 1
    assert not search(worker, root, "needle", pathPattern="*.pdf")[0]
    assert len(search(worker, root, "needle", pathPattern="*.txt")[0]) == 2
    hits, _ = search(worker, root, "ordinary", filenames=True)
    assert len(hits) == 1 and hits[0]["kind"] == "filename", hits
    assert not search(worker, root, "absent")[0]
    assert search(worker, root, "[abc", regex=True, allow_errors=True)[1]
    (root / "broken.docx").write_bytes(b"This is not a zip archive.")
    hits, errors = search(worker, root, "needle", allow_errors=True)
    assert len(hits) == 2 and errors and "broken.docx" in str(errors), (hits, errors)
    print("Case, regex, hidden and ignored files, filename, path filter, no matches and corrupt-file recovery passed")


def verify_native_names(worker, root):
    if os.name != "posix":
        return
    root.mkdir()
    raw = os.fsencode(root) + b"/example_\xff.txt"
    with open(raw, "wb") as output:
        output.write(b"File identity survives a complete round trip.")
    hits, errors = search(worker, root, "identity")
    assert not errors and len(hits) == 1
    assert base64.b64decode(hits[0]["pathBytes"]) == raw
    validate_occurrence(worker, hits[0], "identity")
    print("Non-UTF-8 Unix filename round trip passed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--require-doc", action="store_true", help="also verify genuine Word 97 short streams with an empty converter PATH")
    args = parser.parse_args()
    worker = args.worker.resolve(strict=True)
    with TemporaryDirectory(prefix="Hello Needle test ") as temp:
        root = Path(temp)
        verify_documents(worker, root / "all formats")
        if args.require_doc:
            verify_small_word(worker, root / "small binary Word")
        verify_modes(worker, root / "markup")
        verify_options_and_recovery(worker, root / "search controls")
        verify_native_names(worker, root / "native names")


if __name__ == "__main__":
    main()
