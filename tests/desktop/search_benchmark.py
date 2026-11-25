# SPDX-License-Identifier: GPL-3.0-or-later
"""Measure first match, completion and concurrent memory use on a generated corpus."""
import argparse
from concurrent.futures import ThreadPoolExecutor
import json
import os
from pathlib import Path
import platform
import signal
import subprocess
from tempfile import TemporaryDirectory
import time

from document_fixtures import DOC_NS, write_ooxml
from pdf_checks import synthetic_pdf


def make_corpus(folder, batches):
    folder.mkdir()
    row = b"A sufficiently ordinary line of searchable ASCII text.\n"
    item = b'"a compact JSON string containing ordinary text",'
    paragraph = "<w:p><w:r><w:t>Another documented example.</w:t></w:r></w:p>"
    main = "word/document.xml"
    document = (f'<w:document xmlns:w="{DOC_NS}"><w:body>' + paragraph * 3000
                + "<w:p><w:r><w:t>LastNeedle</w:t></w:r></w:p></w:body></w:document>")
    for number in range(batches):
        synthetic_pdf(folder / f"{number:02}a.pdf", ["dense"] * 12 + ["final"])
        (folder / f"{number:02}b.txt").write_bytes(row * 40000 + b"LastNeedle\n")
        (folder / f"{number:02}c.json").write_bytes(b"[" + item * 44000 + b'"LastNeedle"]')
        write_ooxml(folder / f"{number:02}d.docx", main, {main: document},
                    {main: "wordprocessingml.document.main"})


def group_rss_kib(group):
    # The search and all of its converter children are in a dedicated process group.
    # BSD and Linux ps both express RSS in KiB. A shared page counts in each process.
    rows = subprocess.check_output(["ps", "-ax", "-o", "pgid=,rss="], text=True, timeout=5)
    group = str(group)
    return sum(int(row[1]) for line in rows.splitlines()
               if len(row := line.split()) == 2 and row[0] == group and row[1].isdigit())


def timed_search(worker, folder, workers, expected):
    started = time.monotonic()
    with subprocess.Popen([str(worker), "search"], stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                          start_new_session=True) as process:
        request = {"folder": str(folder), "query": "LastNeedle", "searchWorkers": workers}
        process.stdin.write(json.dumps(request).encode() + b"\n")
        process.stdin.close()

        def collect_times():
            first = None
            for line in process.stdout:
                event = json.loads(line)
                if event["type"] == "match" and first is None:
                    first = time.monotonic() - started
                elif event["type"] == "done":
                    assert event["count"] == expected and event["errors"] == 0, event
                    assert first is not None
                    return first, time.monotonic() - started
            raise AssertionError("Missing search completion")

        peak = 0
        with ThreadPoolExecutor(max_workers=1) as observer:
            times = observer.submit(collect_times)
            try:
                while True:
                    peak = max(peak, group_rss_kib(process.pid))
                    if times.done():
                        break
                    if time.monotonic() - started > 120:
                        raise TimeoutError("Benchmark search exceeded two minutes")
                    time.sleep(0.025)
                first, total = times.result()
            finally:
                if not times.done() and process.poll() is None:
                    os.killpg(process.pid, signal.SIGKILL)
        assert process.wait(timeout=5) == 0
    return {"first_match_ms": round(first * 1000, 2), "total_ms": round(total * 1000, 2),
            "sampled_peak_rss_kib": peak}


def benchmark(worker, root, batches=6, settings=(1, 2, 4, 0)):
    root.mkdir()
    available = int(subprocess.check_output([worker, "--parallelism"], text=True))
    print(f"Search benchmark on {platform.system()} {platform.release()}, {available} available logical CPUs.")
    print(f"Per run: {batches} each of PDFs, DOCX files, multiline text lists and single-line JSON. "
          "Each setting starts with an identically generated corpus and repeats it immediately. "
          "OS caches are uncontrolled. Sampled RSS sums worker and live converter RSS at ~25 ms intervals "
          "and may count shared pages more than once.")
    for workers in settings:
        folder = root / f"workers-{workers}"
        make_corpus(folder, batches)
        effective = min(workers or max(1, available // 2), available)
        for run in ("first", "repeat"):
            result = timed_search(worker, folder, workers, batches * 5)
            print("MT2 benchmark " + json.dumps({"setting": workers or "automatic", "workers": effective,
                                                  "run": run, **result}), flush=True)


if __name__ == "__main__":
    args = argparse.ArgumentParser(description=__doc__)
    args.add_argument("--worker", type=Path, required=True)
    args.add_argument("--batches", type=int, default=6, help="number of files of each of the four formats")
    options = args.parse_args()
    if options.batches < 1:
        args.error("--batches must be positive")
    with TemporaryDirectory(prefix="Hello Needle benchmark ") as temp:
        benchmark(options.worker.resolve(strict=True), Path(temp) / "corpora", options.batches)
