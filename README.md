# Hello Needle

Hello Needle is a desktop application in development for finding words and phrases inside local files. Select a folder, enter a phrase, and choose a result to see the passage in its document. The project targets **FreeBSD, Windows, Linux, and macOS**. It builds on the Rust search engine from Leopold Luley’s Clapgrep and will have a new C++ interface built with Qt 6 Widgets.

Created by **Alfredo Llaquet-Alsina**. **Programmed by ChatGPT.** [Clapgrep and other credits](NOTICE.md)

## Project status

The November 23, 2026 project layout is published. The executable available from this source tree is the inherited **Clapgrep** application. Its FreeBSD build, installation, and graphical searches have been verified. The new Hello Needle interface and installers are planned. Each platform will get its own installation and update instructions when a build has been tested there. [Project record and milestones](AGENTS.md) · [Verified FreeBSD baseline](https://github.com/AlfredoLlaquet/helloneedle/actions/runs/45205259209)

## Interface languages

Hello Needle’s interface will be available in **English and Spanish**. In the current source tree, English is the original interface text and Spanish is the sole translation catalog. Document searches accept text in any language that the chosen file reader can decode.

**En español.** Hello Needle permitirá buscar una frase en una carpeta y leer el pasaje en su contexto. La interfaz estará disponible en español e inglés. Los primeros formatos de documentos de texto con formato serán DOCX (Word) y ODT (LibreOffice Writer). Por ahora, las instrucciones de compilación de esta página corresponden a la aplicación de referencia, Clapgrep para FreeBSD.

## Finding a passage

In the planned interface, the folder and query sit above two resizable panes. The left pane groups matches by file and shows an excerpt for each one. Selecting an excerpt opens the right pane at that occurrence. Someone searching a folder of meeting notes can read the paragraph containing a decision, see which file it came from, and open the original in its usual application.

| Documents | Planned search and preview |
| --- | --- |
| Plain text | Search the text, jump to the matching line, and highlight the occurrence among the surrounding lines. |
| HTML and Markdown | **Document** searches the readable text and shows a formatted preview. **Source** searches the saved markup and opens the matching source line. |
| PDF | Show the matching page and highlight the passage. Zoom, fit the page, and move to neighboring pages for context. The initial scope covers PDFs with a text layer. |
| Word and OpenDocument Text | Treat Word **DOCX** and LibreOffice/OpenOffice Writer **ODT** as first-class formats. Show matching text in its paragraph, with surrounding paragraphs and headings. |
| Spreadsheets and presentations | Cover Excel **XLSX**, Calc **ODS**, PowerPoint **PPTX**, and Impress **ODP**. Use a sheet and cell, or a slide, to identify where a passage occurs. |
| Other word-processor files | The compatibility plan includes Rich Text Format **RTF**, older binary Word **DOC**, and flat OpenDocument Text **FODT**. Their readers and format-specific tests are later work. |

For example, a Document search for `The Latest Batch` should find `The **Latest** Batch` in Markdown and `The <strong>Latest</strong> Batch` in HTML. A Source search for `<strong>` should take you to the tag. The document preview will read local content with scripts and network requests disabled. Searching and previewing leave the original files untouched.

## Build the current FreeBSD reference

The following commands build **Clapgrep**, which is the working reference during Hello Needle development. The recorded test system was FreeBSD 15.1-RELEASE-p6 on amd64.

1. In a root shell, install the dependencies.

   ```sh
   pkg install git rust meson ninja pkgconf gettext-tools gtk4 libadwaita \
       gtksourceview5 poppler-glib blueprint-compiler desktop-file-utils AppStream
   ```

2. As the desktop account, clone the repository and compile from its top-level directory. The first build downloads the Rust dependencies recorded in `Cargo.lock`.

   ```sh
   git clone https://github.com/AlfredoLlaquet/helloneedle.git
   cd helloneedle
   meson setup build --prefix=/usr/local
   CARGO_BUILD_JOBS=2 meson compile -C build
   ```

3. From the same directory, install with root privileges. Launch from a graphical session as the desktop account.

   ```sh
   su root -c 'meson install -C build --no-rebuild'
   clapgrep
   ```

To update an existing checkout of this project, start in the directory that contains `meson.build`. A clone made under the former name can keep its local directory. Set its remote once with `git remote set-url origin https://github.com/AlfredoLlaquet/helloneedle.git`.

```sh
git switch main
git pull --ff-only origin main
CARGO_BUILD_JOBS=2 meson compile -C build
su root -c 'meson install -C build --no-rebuild'
```

## Verify the reference build

From the repository root, the desktop account can run the Rust workspace tests and validate the desktop entry and AppStream metadata. Meson prints any failures and records the full output in `build/meson-logs/testlog.txt`.

```sh
CARGO_BUILD_JOBS=2 meson test -C build --print-errorlogs --timeout-multiplier 20
```

For the installed-application tests, install these additional packages as root.

```sh
pkg install xorg-vfbserver dbus python3 py312-atspi py312-pycairo dejavu xdotool
dbus-uuidgen --ensure
```

Run the two commands below as the desktop account. They open an isolated X11 session for each language, exercise four consecutive searches, and check the optional Nautilus extension in a temporary data directory. The fixtures contain a plain-text match on line 3, a PDF match on page 2, and a DOCX paragraph. [FreeBSD workflow](.github/workflows/freebsd.yml)

```sh
TZ=UTC sh tests/freebsd/installed-search.sh en
TZ=Europe/Madrid sh tests/freebsd/installed-search.sh es
```

## License and origins

Hello Needle is licensed under the **GNU General Public License, version 3 or later** (`GPL-3.0-or-later`) and is supplied without warranty as set out in that license. The original Clapgrep work is by Leopold Luley and its contributors. Individual third-party files retain their own notices and license terms. Each future binary release is to include its notices and offer the complete corresponding source for that build. [Full license](LICENSE) · [Attribution and component notices](NOTICE.md) · [Original Clapgrep project](https://github.com/luleyleo/clapgrep)
