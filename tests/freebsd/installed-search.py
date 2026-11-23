# CG4FBS BEGIN ADDITION — Check search results through the real GTK window.
import re
import subprocess
import sys
import time
from pathlib import Path
from zipfile import ZIP_DEFLATED, ZipFile

import cairo
import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi, GLib


def create_documents(folder):
    (folder / "notes.txt").write_text("First line\nAnother line\nText searchmarker\n", encoding="utf-8")
    (folder / "decoy.txt").write_text("Nothing to find here.\n", encoding="utf-8")

    with cairo.PDFSurface(str(folder / "paper.pdf"), 300, 200) as surface:
        context = cairo.Context(surface)
        for line in ["Cover page", "PDF searchmarker"]:
            context.move_to(20, 40)
            context.show_text(line)
            context.show_page()

    with ZipFile(folder / "letter.docx", "w", ZIP_DEFLATED) as document:
        document.writestr("[Content_Types].xml", """<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
            <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
            <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
            </Types>""")
        document.writestr("_rels/.rels", """<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
            <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
            </Relationships>""")
        document.writestr("word/document.xml", """<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
            <w:body><w:p><w:r><w:t>Office searchmarker</w:t></w:r></w:p></w:body></w:document>""")


def walk(node):
    yield node
    for index in range(node.get_child_count()):
        child = node.get_child_at_index(index)
        if child is not None:
            yield from walk(child)


def wait_for(description, process, check):
    deadline = time.monotonic() + 30
    last_error = None
    while time.monotonic() < deadline:
        if process.poll() is not None:
            raise RuntimeError(f"Clapgrep exited with status {process.returncode} while waiting for {description}")
        while GLib.MainContext.default().iteration(False):
            pass
        try:
            result = check()
            if result:
                return result
        except GLib.Error as error:
            last_error = error
        time.sleep(0.2)
    raise RuntimeError(f"Timed out waiting for {description}") from last_error


def check_window(process):
    desktop = Atspi.get_desktop(0)

    def find_app():
        for index in range(desktop.get_child_count()):
            app = desktop.get_child_at_index(index)
            if app is not None and app.get_process_id() == process.pid:
                return app
        return None

    app = wait_for("the installed GTK application", process, find_app)
    try:
        # Search Pattern is the first editable field, before the optional File Pattern.
        entry = wait_for("the search field", process, lambda: next(
            (node for node in walk(app) if "EditableText" in node.get_interfaces()), None
        ))
        if not Atspi.EditableText.set_text_contents(entry, "searchmarker"):
            raise RuntimeError("Could not enter the search pattern")
        subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(process.pid),
                        "windowfocus", "key", "ctrl+Return"], check=True, timeout=10)

        def results_shown():
            labels = [node.get_name() for node in walk(app) if node.get_role() == Atspi.Role.LABEL]
            visible = " ".join(label for label in labels if label)
            return (all(name in labels for name in ("notes.txt", "paper.pdf", "letter.docx", "Office searchmarker"))
                    and "decoy.txt" not in labels
                    and re.search(r"\b3\s*:\s*Text searchmarker", visible)
                    and re.search(r"\b2\s*:\s*PDF searchmarker", visible))

        wait_for("the text line, PDF page, and Office match", process, results_shown)
        print("Installed Clapgrep found notes.txt line 3, paper.pdf page 2, and the matching DOCX paragraph.")
    except Exception:
        print("Accessible window content:", file=sys.stderr)
        for node in walk(app):
            print(f"  {node.get_role_name()}: {node.get_name()!r}", file=sys.stderr)
        raise


def main():
    folder = Path(sys.argv[1])
    create_documents(folder)
    process = subprocess.Popen(["/usr/local/bin/clapgrep", str(folder)])
    try:
        check_window(process)
        subprocess.run(["gapplication", "action", "de.leopoldluley.Clapgrep", "quit"], check=True, timeout=10)
        if process.wait(timeout=10) != 0:
            raise RuntimeError("Clapgrep did not exit successfully")
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()


if __name__ == "__main__":
    main()
# CG4FBS END
