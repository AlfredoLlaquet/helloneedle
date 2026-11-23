#!/usr/bin/env python3
"""Exercise the native Hello Needle window and assemble reviewable visual evidence."""

import argparse
import ctypes
import hashlib
import json
import os
import platform
import shutil
import subprocess
import sys
import tempfile
from contextlib import contextmanager
from pathlib import Path
from xml.etree import ElementTree

from document_fixtures import write_documents

FULL = [
    "firstLaunch", "optionsAndMenus", "unicodeAndRepeat", "wordAndOfficePreviews",
    "markupModes", "pdfNavigation", "optionsAndCancellation", "languageMenu", "helpAndCredits",
]
SCALED = ["firstLaunch", "optionsAndMenus", "unicodeAndRepeat"]


def native_plugin():
    return {"Windows": "windows", "Darwin": "cocoa"}.get(platform.system(), "xcb")


def scenarios():
    yield "system", "system", "system", "1", [*FULL, "nativeDirectoryPicker"]
    for language in ("en", "es"):
        for appearance in ("light", "dark"):
            for factor, suffix in (("1", "100"), ("1.5", "150")):
                yield f"{language}-{appearance}-{suffix}", language, appearance, factor, (
                    FULL if factor == "1" else SCALED
                )
    yield "directory-selection", "es", "system", "1", ["directorySelection"]


@contextmanager
def hosted_theme(appearance):
    """Native OS settings are touched only on a disposable GitHub-hosted runner."""
    hosted = os.environ.get("GITHUB_ACTIONS") == "true" and os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted"
    if not hosted or appearance == "system":
        yield
        return
    if platform.system() == "Windows":
        import winreg

        name = "AppsUseLightTheme"
        path = r"Software\Microsoft\Windows\CurrentVersion\Themes\Personalize"
        with winreg.CreateKeyEx(winreg.HKEY_CURRENT_USER, path, 0, winreg.KEY_QUERY_VALUE | winreg.KEY_SET_VALUE) as key:
            try:
                previous = winreg.QueryValueEx(key, name)
            except FileNotFoundError:
                previous = None

            def broadcast():
                # Windows' setting-change signal lets shell widgets re-read ImmersiveColorSet.
                send = ctypes.windll.user32.SendMessageTimeoutW
                send.argtypes = [ctypes.c_void_p, ctypes.c_uint, ctypes.c_size_t, ctypes.c_void_p,
                                 ctypes.c_uint, ctypes.c_uint, ctypes.c_void_p]
                message = ctypes.c_wchar_p("ImmersiveColorSet")
                send(0xffff, 0x001A, 0, ctypes.cast(message, ctypes.c_void_p), 2, 120, None)

            winreg.SetValueEx(key, name, 0, winreg.REG_DWORD, int(appearance == "light"))
            broadcast()
            try:
                yield
            finally:
                if previous is None:
                    winreg.DeleteValue(key, name)
                else:
                    winreg.SetValueEx(key, name, 0, previous[1], previous[0])
                broadcast()
        return
    if platform.system() == "Darwin":
        command = ["/usr/bin/defaults"]
        key = "AppleInterfaceStyle"
        previous = subprocess.run(command + ["read", "-g", key], capture_output=True, text=True)
        change = ["write", "-g", key, "-string", "Dark"] if appearance == "dark" else ["delete", "-g", key]
        subprocess.run(command + change, capture_output=True, check=appearance == "dark")
        try:
            yield
        finally:
            reset = (["write", "-g", key, "-string", previous.stdout.strip()]
                     if previous.returncode == 0 else ["delete", "-g", key])
            subprocess.run(command + reset, capture_output=True, check=previous.returncode == 0)
        return
    yield


def run_case(tester, worker, docs, settings, directory, spec, plugin):
    name, language, appearance, factor, slots = spec
    directory.mkdir(parents=True, exist_ok=True)
    settings.mkdir()
    env = os.environ.copy()
    # Explicit X11 on Unix enables testing under Xvfb without an ambient Wayland session.
    env.update(QT_QPA_PLATFORM=plugin, QT_SCALE_FACTOR=factor,
               HELLONEEDLE_WORKER=str(worker), HN_QA_LANGUAGE=language, HN_QA_APPEARANCE=appearance,
               HN_QA_DOCUMENTS=str(docs), HN_QA_SETTINGS=str(settings), HN_QA_OUTPUT=str(directory),
               HN_QA_MASTER_ICON=str(Path(__file__).resolve().parents[2] / "desktop/qt/helloneedle.png"))
    if plugin == "offscreen":
        env["HN_QA_SUPPLEMENTARY"] = "1"
        slots = [s for s in slots if s != "nativeDirectoryPicker"]
    if name == "directory-selection":
        env["HN_QA_WIDGET_PICKER"] = "1"
    result_file = directory / "results.xml"
    cmd = [str(tester), *slots, "-o", f"{result_file},junitxml", "-o", "-,txt"]
    print(f"Testing {name} using {plugin}", flush=True)
    try:
        with hosted_theme(appearance):
            proc = subprocess.run(cmd, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                                  encoding="utf-8", errors="replace", timeout=135)
        log, code = proc.stdout, proc.returncode
    except subprocess.TimeoutExpired as exc:
        log = exc.stdout or b""
        if isinstance(log, bytes):
            log = log.decode("utf-8", "replace")
        log += "\nNative GUI scenario timed out.\n"
        code = 124
    (directory / "test.log").write_text(log, encoding="utf-8")
    if code:
        print(log, flush=True)
    cases = []
    if result_file.exists():
        for test in ElementTree.parse(result_file).iter("testcase"):
            status = "failed" if test.find("failure") is not None or test.find("error") is not None else "passed"
            if test.find("skipped") is not None:
                status = "skipped"
            cases.append({"name": test.get("name"), "result": status})
    evidence = directory / "appearance.json"
    screens = {p.name: hashlib.sha256(p.read_bytes()).hexdigest() for p in sorted(directory.glob("*.png"))}
    return {"scenario": name, "language": language, "appearance": appearance, "scale": factor,
            "returnCode": code, "cases": cases, "screenshots": screens,
            "native": json.loads(evidence.read_text(encoding="utf-8")) if evidence.exists() else {}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tester", type=Path, required=True)
    parser.add_argument("--worker", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scenario", action="append", choices=[s[0] for s in scenarios()],
                        help="Limit a local verification run to the named scenarios")
    parser.add_argument("--supplementary-offscreen", action="store_true",
                        help="Development check for machines without a display, excluded from native evidence")
    args = parser.parse_args()
    plugin = "offscreen" if args.supplementary_offscreen else native_plugin()
    tester, worker, output = (p.resolve() for p in (args.tester, args.worker, args.output))
    for binary in (tester, worker):
        if not binary.is_file():
            parser.error(f"Executable missing: {binary}")
    if plugin == "xcb" and not os.environ.get("DISPLAY"):
        parser.error("A native X11 display is required. Run the matrix inside Xvfb on a headless runner.")
    output.mkdir(parents=True, exist_ok=True)
    cases = []
    review = None
    with tempfile.TemporaryDirectory(prefix="helloneedle-gui-") as temp:
        # Windows TEMP can spell the user profile with an 8.3 alias. The
        # native dialog returns the long path when a folder is selected.
        scratch = Path(temp).resolve(strict=True)
        docs = scratch / "sample documents"
        write_documents(docs)
        (docs / ".ignore").write_text("ignored.txt\n", encoding="utf-8")
        (docs / ".private.txt").write_text("HiddenFixture391\n", encoding="utf-8")
        (docs / "ignored.txt").write_text("IgnoredFixture924\n", encoding="utf-8")
        digest = lambda: {p.name: hashlib.sha256(p.read_bytes()).hexdigest()
                          for p in docs.iterdir() if p.is_file()}
        before = digest()
        for spec in scenarios():
            if args.scenario and spec[0] not in args.scenario:
                continue
            # Capture into a new directory. Earlier pictures cannot turn a failed
            # run into a pass, and unrelated files in the output path stay intact.
            directory = scratch / "screens" / spec[0]
            case = run_case(tester, worker, docs, scratch / ("settings-" + spec[0]), directory, spec, plugin)
            destination = output / spec[0]
            destination.mkdir(exist_ok=True)
            for produced in directory.iterdir():
                shutil.copy2(produced, destination / produced.name)
            cases.append(case)
        unchanged = before == digest()
        if not args.scenario:
            current = scratch / "screens"
            review = subprocess.run([str(tester), "--make-review", str(current)],
                                    env={**os.environ, "QT_QPA_PLATFORM": plugin}).returncode == 0
            if (current / "review.png").exists():
                shutil.copy2(current / "review.png", output / "review.png")
    manifest = {"os": platform.platform(), "platformPlugin": plugin, "nativeAcceptance": not args.supplementary_offscreen,
                "fixturesUnchanged": unchanged,
                "reviewSheetComplete": review, "scenarios": cases}
    (output / "manifest.json").write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    failures = [case["scenario"] for case in cases if case["returnCode"]]
    print(f"GUI matrix: {len(cases) - len(failures)}/{len(cases)} scenarios passed. "
          f"{sum(len(c['screenshots']) for c in cases)} screenshots. Evidence: {output}", flush=True)
    if failures:
        print("Failed scenarios: " + ", ".join(failures), file=sys.stderr)
    return int(bool(failures) or not unchanged or review is False)


if __name__ == "__main__":
    sys.exit(main())
