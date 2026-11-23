# CG4FBS BEGIN ADDITION — Check search results through the real GTK window.
import os
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


def check_window(process, language):
    entry_name, button_name, no_results = {
        "en": ("Search Pattern", "Start Search", "No Results"),
        "es": ("Patrón de búsqueda", "Iniciar búsqueda", "Sin resultados"),
    }[language]
    desktop = Atspi.get_desktop(0)

    def find_app():
        for index in range(desktop.get_child_count()):
            app = desktop.get_child_at_index(index)
            if app is not None and app.get_process_id() == process.pid:
                return app
        return None

    app = wait_for("the installed GTK application", process, find_app)

    def named_control(name, interface):
        return wait_for(name, process, lambda: next((
            node for node in walk(app)
            if node.get_name() == name and node.get_role() != Atspi.Role.LABEL
            and interface in node.get_interfaces()
        ), None))

    def focus_window():
        # PID-only searches must end before the next xdotool command.
        window = subprocess.check_output(
            ["xdotool", "search", "--sync", "--onlyvisible", "--limit", "1", "--pid", str(process.pid)],
            text=True, timeout=10,
        ).strip()
        subprocess.run(["xdotool", "windowfocus", "--sync", window], check=True, timeout=10)
        return window

    def press(key):
        # A separate command delivers native XTEST events to the focused window.
        subprocess.run(["xdotool", "key", "--clearmodifiers", key], check=True, timeout=10)

    def search(term):
        entry = named_control(entry_name, "EditableText")
        if not Atspi.EditableText.set_text_contents(entry, term):
            raise RuntimeError(f"Could not enter {term!r} in {entry_name!r}")
        wait_for(f"entered query {term!r}", process, lambda: Atspi.Text.get_text(entry, 0, -1) == term)
        focus_window()
        press("ctrl+Return")

    def toggle_nautilus(position):
        row = named_control("Nautilus", "Component")
        window = focus_window()
        previous = None

        def settled_bounds():
            nonlocal previous
            bounds = Atspi.Component.get_extents(row, Atspi.CoordType.WINDOW)
            current = (bounds.x, bounds.y, bounds.width, bounds.height)
            stable = current == previous and bounds.width > 0 and bounds.height > 0
            previous = current
            return bounds if stable else None

        bounds = wait_for("the Nautilus switch to finish opening", process, settled_bounds)
        # Click at different points in the row so GTK recognizes two separate clicks.
        x = bounds.x + position * bounds.width // 3
        y = bounds.y + bounds.height // 2
        subprocess.run(["xdotool", "mousemove", "--window", window, str(x), str(y)], check=True, timeout=10)
        subprocess.run(["xdotool", "click", "1"], check=True, timeout=10)

    def labels():
        return [node.get_name() for node in walk(app) if node.get_role() == Atspi.Role.LABEL]

    def all_formats_shown():
        current = labels()
        visible = " ".join(label for label in current if label)
        return (all(name in current for name in ("notes.txt", "paper.pdf", "letter.docx", "Office searchmarker"))
                and "decoy.txt" not in current
                and re.search(r"\b3\s*:\s*Text searchmarker", visible)
                and re.search(r"\b2\s*:\s*PDF searchmarker", visible))

    def only_office_shown():
        current = labels()
        return ("letter.docx" in current and "Office searchmarker" in current
                and not {"notes.txt", "paper.pdf", "decoy.txt"}.intersection(current))

    def empty_state_shown():
        current = labels()
        return (no_results in current
                and not {"letter.docx", "notes.txt", "paper.pdf", "decoy.txt"}.intersection(current))

    try:
        named_control(button_name, "Component")
        search("searchmarker")
        wait_for("the text line, PDF page, and Office paragraph", process, all_formats_shown)
        search("Office searchmarker")
        wait_for("only the Office match", process, only_office_shown)
        search("absentmarker")
        wait_for("the empty results page", process, empty_state_shown)
        search("searchmarker")
        wait_for("all three matches after an empty search", process, all_formats_shown)
        print(f"Four searches passed ({language}): text line 3, PDF page 2, DOCX paragraph, and empty results.")

        extension = Path(os.environ["XDG_DATA_HOME"]) / "nautilus-python/extensions/clapgrep.py"
        original = Path(__file__).resolve().parents[2] / "assets/integrations/nautilus/clapgrep.py"
        if extension.exists():
            raise RuntimeError(f"Unexpected preexisting extension: {extension}")
        subprocess.run(["gapplication", "action", "de.leopoldluley.Clapgrep", "preferences"], check=True, timeout=10)
        toggle_nautilus(1)
        wait_for("the installed Nautilus extension", process, extension.is_file)
        if extension.read_bytes() != original.read_bytes():
            raise RuntimeError("The installed Nautilus extension differs from the packaged source")
        toggle_nautilus(2)
        wait_for("removal of the Nautilus extension", process, lambda: not extension.exists())
        print("Preferences installed and removed the Nautilus extension under XDG_DATA_HOME.")
        press("Escape")
        wait_for("the Preferences dialog to close", process,
                 lambda: not any(node.get_name() == "Nautilus" for node in walk(app)))
        named_control(button_name, "Component")
    except Exception:
        if process.poll() is None:
            print("Accessible window content:", file=sys.stderr)
            for node in walk(app):
                value = Atspi.Text.get_text(node, 0, -1) if "EditableText" in node.get_interfaces() else ""
                print(f"  {node.get_role_name()}: {node.get_name()!r} {value!r}", file=sys.stderr)
        raise


def main():
    folder = Path(sys.argv[1])
    create_documents(folder)
    process = subprocess.Popen(["/usr/local/bin/clapgrep", str(folder)], cwd=folder.parent,
                               env={**os.environ, "G_DEBUG": "fatal-criticals", "RUST_LOG": "clapgrep_gnome=debug"})
    try:
        check_window(process, sys.argv[2])
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
