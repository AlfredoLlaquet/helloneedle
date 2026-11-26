# SPDX-License-Identifier: GPL-3.0-or-later
"""Check the file worker pool through the installed worker's public protocol."""
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
import json
import os
from pathlib import Path
import shlex
import shutil
import signal
import subprocess
import sys
from tempfile import TemporaryFile
import time

from document_fixtures import write_documents


def verify_equivalence(worker, folder, exchange, retained_matches):
    write_documents(folder)
    binary_file = folder / "Needle.unknown"
    binary_file.write_bytes(b"Needle\n" + b"plain text\n" * 30000 + b"\0")
    (folder / "broken.docx").write_bytes(b"broken")
    (folder / "archive.7z").write_bytes(b"Needle")
    responses = []
    for workers in (1, 4, 0):
        request = {"folder": str(folder), "query": "Needle", "filenames": True,
                   "searchWorkers": workers}
        events = exchange(worker, "search", request, allow_errors=True)
        by_file = defaultdict(list)
        for hit in retained_matches(events):
            by_file[hit["path"]].append(hit)
        errors = [(e["path"], e["code"]) for e in events if e["type"] == "error"]
        assert len(errors) == 1 and Path(errors[0][0]).name == "broken.docx", errors
        # Rust and MSYS2 Python can spell the same Windows path with different separators.
        binary_hits = [hit["kind"] for path, hits in by_file.items()
                       if binary_file.samefile(path) for hit in hits]
        assert binary_hits == ["filename"], (binary_file, binary_hits)
        done = events[-1]
        assert done["count"] == sum(map(len, by_file.values())) == 16, done
        assert done["files"] == 16 and done["documentsScanned"] == 13, done
        # Different files may finish in any order. All events for each individual
        # file must retain the same occurrences, text ranges, and reading order.
        responses.append((by_file, errors, done))
    assert responses[0] == responses[1] == responses[2], "Serial/parallel results differ"


def verify_all_occurrences(worker, folder):
    folder.mkdir()
    expected = {f"{number}.txt": 1_000 for number in range(4)}
    for name, count in expected.items():
        (folder / name).write_bytes(b"Needle\n" * count)
    sentinel = "zz-sentinel.txt"
    # This distinct hit follows 96,000 others in its own file regardless of
    # filesystem traversal order or concurrent scheduling.
    (folder / sentinel).write_bytes(b"Needle\n" * 96_000 + b"Needle at the end of the last-sorted file.\n")
    expected[sentinel] = 96_001
    for workers in (1, 4):
        request = {"folder": str(folder), "query": "Needle", "searchWorkers": workers}
        seen = defaultdict(int)
        found_sentinel = False
        done = None
        # A large test should not need space in Python's heap for every JSON object.
        with TemporaryFile() as stream:
            subprocess.run([worker, "search"], input=json.dumps(request).encode(),
                           stdout=stream, check=True, timeout=180)
            stream.seek(0)
            for line in stream:
                event = json.loads(line)
                kind = event["type"]
                assert done is None, "Events followed the completion record"
                if kind == "match":
                    name = event["displayPath"]
                    assert name in expected and event["location"]["index"] == seen[name], event
                    seen[name] += 1
                    if name == sentinel and seen[name] == expected[name]:
                        assert event["text"] == "Needle at the end of the last-sorted file.", event
                        found_sentinel = True
                elif kind == "done":
                    done = event
                else:
                    assert kind == "progress", event
        assert seen == expected, seen
        assert done == {"type": "done", "count": 100_001, "errors": 0, "files": 5,
                        "documentsScanned": 5, "truncated": False}, done
        assert found_sentinel


def verify_all_errors(worker, folder, exchange):
    folder.mkdir()
    expected = {f"bad-{i:03}.docx" for i in range(1, 491)}
    for name in expected:
        (folder / name).write_bytes(b"incomplete")
    (folder / "zz-readable.txt").write_bytes(b"Needle")
    events = exchange(worker, "search", {"folder": str(folder), "query": "Needle"}, allow_errors=True)
    errors = [Path(event["path"]).name for event in events if event["type"] == "error"]
    assert len(errors) == 490 and set(errors) == expected
    assert [event["displayPath"] for event in events if event["type"] == "match"] == ["zz-readable.txt"]
    assert events[-1] == {"type": "done", "errors": 490, "count": 1, "files": 491,
                          "documentsScanned": 1, "truncated": False}, events[-1]


def wait_for(predicate, description, timeout=15):
    deadline = time.monotonic() + timeout
    while not predicate():
        if time.monotonic() >= deadline:
            raise AssertionError(f"Timed out waiting for {description}")
        time.sleep(0.02)


def controlled_decoder(args):
    """Each helper runs until the test authorizes it to complete its synthetic PDF."""
    gate = Path(os.environ["HN_TEST_PDF_GATE"])
    name = Path(args[-2]).stem
    (gate / f"{name}.start").write_text(f"{time.monotonic_ns()} {os.getpid()}")
    line = '<line><word xMin="0" yMin="0" xMax="8" yMax="8">Needle</word></line>'
    start_page = '<doc><page width="100" height="100">'
    if name in ("flood", "refill"):
        prerequisite = "blocked.start" if name == "flood" else "begin-refill"
        wait_for(lambda: (gate / prerequisite).exists(), "the competing PDF")
        count = 20_000 if name == "flood" else 10_009
        print(start_page + line * count + "</page></doc>", flush=True)
    else:
        # A closed page makes a hit visible before the converter has finished.
        print(start_page + line + "</page>", flush=True)
        wait_for(lambda: (gate / "release").exists(), "the PDF gate", timeout=40)
        if name.endswith("-bad"):
            print("controlled extraction failure", file=sys.stderr)
            (gate / f"{name}.finish").write_text(str(time.monotonic_ns()))
            return 2
        print("</doc>")
    (gate / f"{name}.finish").write_text(str(time.monotonic_ns()))
    return 0


@contextmanager
def live_search(worker, folder, gate, close_after_matches=None, **options):
    gate.mkdir()
    with subprocess.Popen([str(worker), "search"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          start_new_session=True, env=dict(os.environ, HN_TEST_PDF_GATE=str(gate))) as process:
        process.stdin.write(json.dumps({"folder": str(folder), "query": "Needle", **options}).encode() + b"\n")
        process.stdin.close()
        events = []

        def collect():
            count = 0
            for line in process.stdout:
                event = json.loads(line)
                events.append(event)
                if event["type"] == "match":
                    count += 1
                    if count == close_after_matches:
                        process.stdout.close()
                        return

        with ThreadPoolExecutor(max_workers=1) as reader:
            result = reader.submit(collect)
            try:
                yield process, events
            finally:
                # Always release temporary helpers, including when a test fails.
                (gate / "release").touch()
                if process.poll() is None:
                    try:
                        process.wait(timeout=3)
                    except subprocess.TimeoutExpired:
                        os.killpg(process.pid, signal.SIGKILL)
                        process.wait()
                result.result(timeout=5)


def check_pdf_slots(worker, folder, gate, workers, slots, retained_matches):
    with live_search(worker, folder, gate, searchWorkers=workers, pdfConversions=slots,
                     filenames=True) as (process, events):
        expected = min(workers, slots or 2)
        wait_for(lambda: len(list(gate.glob("*.start"))) >= expected, "concurrent PDF conversions")
        if workers > expected:
            wait_for(lambda: any(e.get("kind") == "text" for e in events), "the ordinary text match")
        wait_for(lambda: any(e.get("kind") == "pdf" for e in events), "a provisional PDF hit")
        assert len(list(gate.glob("*.start"))) == expected
        (gate / "release").touch()
        assert process.wait(timeout=15) == 0
    timeline = []
    for doc in folder.glob("*.pdf"):
        start = (gate / f"{doc.stem}.start").read_text().split()[0]
        finish = (gate / f"{doc.stem}.finish").read_text()
        timeline.extend([(int(start), 1), (int(finish), -1)])
    active = 0
    for _, change in sorted(timeline):
        active += change
        assert 0 <= active <= expected, "PDFs exceeded the configured cap"
    matches = retained_matches(events)
    assert any(e.get("kind") == "pdf" and e["path"].endswith("-bad.pdf") for e in events)
    surviving_bad = [e for e in matches if e["path"].endswith("-bad.pdf")]
    assert [e["kind"] for e in surviving_bad] == ["filename"] and events[-1]["errors"] == 1, events[-1]
    assert events[-1]["count"] == len(matches) == events[-1]["files"] == 7, events[-1]


def check_closed_output_cancels(worker, root):
    folder = root / "active cancellation"
    gate = root / "cancellation gate"
    folder.mkdir()
    for name in ("blocked.pdf", "flood.pdf"):
        (folder / name).touch()
    with live_search(worker, folder, gate, close_after_matches=10_001,
                     searchWorkers=2, pdfConversions=2) as (process, events):
        # Closing the pipe while another converter waits should reap both readers.
        assert process.wait(timeout=30) == 0, "Worker did not finish after its client disconnected"
        pid = int((gate / "blocked.start").read_text().split()[1])
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            pass
        else:
            raise AssertionError("The search left its blocked Poppler subprocess behind")
    assert sum(e["type"] == "match" for e in events) == 10_001
    assert events[-1]["type"] == "match", events[-1]


def check_changed_during_large_search(worker, root, retained_matches):
    folder = root / "late revision change"
    gate = root / "revision gate"
    folder.mkdir()
    changed = folder / "Needle-changing.pdf"
    changed.touch()
    (folder / "refill.pdf").touch()
    with live_search(worker, folder, gate, searchWorkers=2, pdfConversions=2,
                     filenames=True) as (process, events):
        wait_for(lambda: any(e.get("kind") == "pdf" and e["path"] == str(changed) for e in events),
                 "the first revision's provisional hit")
        # Both readers must complete even after one file's provisional hit is withdrawn.
        changed.write_bytes(b"changed revision")
        (gate / "begin-refill").touch()
        (gate / "release").touch()
        assert process.wait(timeout=15) == 0
    hits = retained_matches(events)
    assert [(e["code"], e["path"]) for e in events if e["type"] == "error"] == [("changed", str(changed))]
    assert [h["kind"] for h in hits if h["path"] == str(changed)] == ["filename"]
    valid_lines = [h["location"]["index"] for h in hits if h["kind"] == "pdf"]
    assert valid_lines == list(range(10_009)), "Missing, repeated, or reordered hits in the other file"
    done = events[-1]
    assert (done["count"], done["truncated"], done["files"], done["documentsScanned"]) == (10_010, False, 2, 1), done


def verify_parallel(worker, root, exchange, retained_matches):
    root.mkdir()
    verify_equivalence(worker, root / "equivalence", exchange, retained_matches)
    verify_all_occurrences(worker, root / "large result set")
    verify_all_errors(worker, root / "diagnostics", exchange)
    print("Serial, automatic and manual searches agree on every occurrence, file/error counts and rollback.")
    print("100,001 occurrences and the final file verified in serial and parallel; 490 distinct errors retained.")
    if os.name != "posix":
        return
    available = int(subprocess.check_output([worker, "--parallelism"], text=True))
    folder = root / "pdf queue"
    folder.mkdir()
    for number in range(6):
        filename = "00-Needle-bad.pdf" if number == 0 else f"{number:02}.pdf"
        (folder / filename).touch()
    (folder / "z.txt").write_text("Needle", encoding="utf-8")
    bindir = root / "tools"
    bindir.mkdir()
    worker = Path(shutil.copy2(worker, bindir))
    helper = bindir / "pdftotext"
    helper.write_text("#!/bin/sh\nexec " + shlex.quote(sys.executable) + " "
                      + shlex.quote(str(Path(__file__).resolve())) + ' --decoder "$@"\n')
    helper.chmod(0o755)
    workers = min(4, available)
    check_pdf_slots(worker, folder, root / "one slot", workers, 1, retained_matches)
    check_pdf_slots(worker, folder, root / "automatic slots", workers, 0, retained_matches)
    if workers >= 2:
        check_closed_output_cancels(worker, root)
        check_changed_during_large_search(worker, root, retained_matches)
    print(f"Isolated PDF processes checked with {workers} available search worker(s).")


if __name__ == "__main__" and sys.argv[1:2] == ["--decoder"]:
    raise SystemExit(controlled_decoder(sys.argv[2:]))
