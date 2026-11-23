# Hello Needle 1.0.0

<img src="desktop/qt/helloneedle.png" alt="Hello Needle’s astonished cow" width="96" height="96">

Hello Needle finds a word or phrase inside a folder of documents and opens each match in context. The desktop application uses Qt 6 Widgets on FreeBSD, Windows, Linux, and macOS. Its Rust search worker builds on the file traversal and matching code in Leopold Luley’s Clapgrep.

Created by **Alfredo Llaquet-Alsina**. **Programmed by ChatGPT.** [Origins and credits](NOTICE.md)

## Find a passage

1. Choose a folder, type a phrase, and press **Enter**. Matches appear grouped by file. Search options include regular expressions, case sensitivity, a path pattern, filenames, hidden files, and ignored files.
2. Select a passage. The preview shows the occurrence in its document. Use **Open original** to open the file in the desktop’s usual application. Search and preview read the files in place.
3. For Markdown or HTML, choose **Document** to search formatted text or **Source** to search markup. A Document search for `The Latest Batch` finds `The **Latest** Batch` across the bold text. A Source search for `<strong>` finds the HTML tag itself.

**English and Spanish** are the two interface languages. On first launch, Spanish regional locales select Spanish. Other locales use English. The Language menu lets you choose. The interface language places no restriction on the text or filenames being searched.

**En español.** Hello Needle busca palabras y frases en los documentos de una carpeta. Escribe una expresión, pulsa **Intro** y selecciona una coincidencia para leer el pasaje. En HTML y Markdown, el modo **Documento** busca el texto visible y el modo **Fuente** busca las etiquetas o el marcado. El menú **Idioma** permite elegir español o inglés.

[First-launch and dark-mode screenshots on each tested desktop](AGENTS.md#native-appearance-in-100)

## Documents

| Format | Where a match opens |
| --- | --- |
| Plain text and text-based source files | The original line with the selected occurrence highlighted. Reads UTF-8 and byte-order-marked UTF-16. |
| HTML and Markdown | A formatted block or a source line. Document mode joins text across inline formatting and ignores scripts. Previews use local content. |
| PDF with a text layer | The matching page, with a highlight over the matching words, zoom, fit, and page navigation. |
| Word DOCX, OpenDocument Text ODT and flat FODT | The matching paragraph, heading, or table passage with nearby text. Character formatting can divide a word across multiple runs in the source file. The search joins those runs. |
| Excel XLSX and OpenDocument Spreadsheet ODS | The sheet and cell address. Text and stored values are searchable. |
| PowerPoint PPTX and OpenDocument Presentation ODP | The slide containing the matching passage. |
| RTF and legacy binary Word DOC | Readable text by paragraph. The worker reads RTF and Word 97–2003 binary documents directly, including short documents. Earlier Word variants use the antiword converter. |

Office previews show a passage in reading order. Pagination and layout can differ from the originating word processor. Scanned PDFs need a text layer for searching. Protected, unsupported, damaged, and oversized documents can produce a per-file diagnostic while the folder search continues.

## Install and run

The desktop executable is `helloneedle`. It starts the adjacent `helloneedle-worker` automatically. The FreeBSD/Linux package-manager builds, Windows UCRT64 archive, and macOS app use separately installed Qt and document tools. Binary artifacts are accompanied by the exact project source, locked Rust dependencies, checksums, and the build inventory. [Download 1.0.0 packages and matching source](https://github.com/AlfredoLlaquet/helloneedle/actions/runs/45262698979#artifacts) · [Build and packaging record](AGENTS.md#build-install-and-package)

| Platform | Runtime and package |
| --- | --- |
| FreeBSD | Install the `.pkg` with `pkg add ./<package>.pkg` on a compatible FreeBSD release. Qt 6, Poppler utilities, and antiword come from FreeBSD packages. Launch **Hello Needle** from the desktop or run `helloneedle`. |
| Linux | On the matching Ubuntu release, install the downloaded `.deb` with `sudo apt install ./<package>.deb`. A graphical desktop entry and the `helloneedle` command are installed. Other distributions can build from source. |
| Windows | Use the MSYS2 **UCRT64** build. Install the MSYS2 runtime packages, extract the archive, and double-click **Hello Needle.exe** at its top level. [Windows runtime instructions](packaging/WINDOWS.txt) |
| macOS | The verified Intel archive targets macOS 15 Sequoia (`x86_64`). The Apple Silicon build targets macOS 26 Tahoe (`arm64`). Install the MacPorts runtime packages in `/opt/local`, extract the ad hoc signed **Hello Needle.app**, and open it in Finder. [macOS runtime instructions](packaging/MACOS.txt) |

### Build from source

The desktop build requires a C++17 compiler, Qt 6.4 or newer with Qt Widgets, CMake 3.21 or newer, Rust/Cargo 1.85 or newer, Python 3.9 or newer, and a build tool such as Ninja. The native GUI audit also uses Qt Test. PDF extraction and display require `pdftotext` and `pdftoppm`. Pre-97 Word variants need antiword and its character maps.

| Build host | Example dependencies |
| --- | --- |
| FreeBSD | `pkg install git cmake ninja rust python3 qt6-base qt6-translations poppler-utils antiword` |
| Ubuntu | `sudo apt install git build-essential cmake ninja-build python3 qt6-base-dev qt6-qpa-plugins qt6-translations-l10n poppler-utils antiword`, plus current stable Rust/Cargo |
| Windows, MSYS2 UCRT64 shell | `pacman -S --needed git mingw-w64-ucrt-x86_64-{gcc,cmake,ninja,rust,python,qt6-base,qt6-translations,poppler,antiword}` |
| macOS, Apple Silicon or Intel | Install the Xcode Command Line Tools and MacPorts for the installed macOS release, then run `sudo /opt/local/bin/port install cmake ninja rust cargo python313 qt6-qtbase qt6-qttranslations poppler antiword`. [MacPorts installation](https://www.macports.org/install.php) |

From a source checkout or the unpacked corresponding source archive, run the following commands. Qt and the converters must be discoverable in the selected platform environment. On macOS, set `export CMAKE_PREFIX_PATH=/opt/local/libexec/qt6:/opt/local` before configuring so CMake finds the MacPorts Qt installation.

```sh
cmake -S . -B build-qt -G Ninja -DCMAKE_BUILD_TYPE=Release
cmake --build build-qt --parallel
cmake --install build-qt --prefix "$PWD/stage"
```

On FreeBSD and Linux the resulting command is `stage/bin/helloneedle`. Windows produces `stage/bin/helloneedle.exe`. On macOS the installed application is `stage/Hello Needle.app`. A folder can also be supplied as a positional argument. The developer record covers installed tests, package removal, appearance checks, component sources, and the retained Clapgrep reference. [Project record](AGENTS.md)

## License

Hello Needle is distributed under **GPL-3.0-or-later**. The original Clapgrep code is by Leopold Luley and its contributors. Embedded components, dependencies, and individual assets retain their own licenses and credits. The application’s About dialog and installed notices provide offline access to these texts. [GNU GPL](LICENSE) · [Attribution and license inventory](NOTICE.md) · [Original Clapgrep](https://github.com/luleyleo/clapgrep)
