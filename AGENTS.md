# Hello Needle project record

**Alfredo Llaquet-Alsina** created Hello Needle. **Programmed by ChatGPT.** Leopold Luley and the Clapgrep contributors wrote the application from which this project derives. Keep both credits specific to their work. [Attribution](NOTICE.md)

## How to maintain this record

This file is the living manual for decisions, development notes, controls, source responsibilities, build procedures, and verification. Update the affected sections **in the same change as the program**. Replace superseded instructions and date substantive decisions. Report tested systems and evidence precisely. The README describes the application for someone installing or using it. The license texts, fixtures, and automated tests are the authoritative artifacts for their respective subjects.

This practice comes from Crumb Writer’s rule to keep its detailed manual synchronized with the program. Hello Needle puts the rule and the manual together here. A new contributor should be able to trace a result from the keyboard to the matching text, find the responsible module, and reproduce the test. [Crumb Writer maintenance rule](https://github.com/AlfredoLlaquet/crumbwriter/blob/ec92c550babbb52d34b7c4872b3fecc4c1b91d48/AGENTS.md) · [Crumb Writer manual](https://github.com/AlfredoLlaquet/crumbwriter/blob/ec92c550babbb52d34b7c4872b3fecc4c1b91d48/CrumbWriter.txt)

## Decisions and ancestry

| Date | Decision |
| --- | --- |
| November 23, 2026 | Renamed the existing FreeBSD fork to `AlfredoLlaquet/helloneedle`, kept its history, and published the initial layout. The product is **Hello Needle**, the executable is `helloneedle`. Its package, window, and settings have their own identities. |
| November 23, 2026 | Authorized the complete desktop application across FreeBSD, Windows, Linux, and macOS. Alfredo selected **1.0.0** as the first Hello Needle version. Use the same three-part version for the desktop, Rust worker and native packages. The build inventory records the exact Git revision separately. The inherited GTK executable remains available under its original name, `clapgrep`. Historical Clapgrep tags identify the original project. |
| November 23, 2026 | Selected Qt 6 Widgets with C++ for the desktop. The search worker is a separate Rust executable. One worker serves one request. This preserves Clapgrep’s approach to matching/traversal, isolates PDF and document decoding, and lets the desktop stop obsolete work without blocking its event loop. |
| November 23, 2026 | Limited the application interface to **English and Spanish**. English supplies original and fallback text. Spanish regional locales select Spanish. The desktop Language menu can override the system selection. Neither language choice filters document content. |
| November 23, 2026 | Adopted Alfredo’s icon design, the face of an astonished cow. The transparent master is `desktop/qt/helloneedle.png`. Use it for the running window, Linux and FreeBSD application menus, Windows Explorer, and the macOS Finder and Dock. Keep its ears, horns and open mouth inside the frame. |
| November 23, 2026 | Required a native GUI appearance audit on all four platforms. A clean first launch receives particular attention because Crumb Writer exhibited platform-specific problems with icons, dark menus, and input fields. Inspect actual images in addition to contrast and geometry measurements. |
| November 23, 2026 | Alfredo requested standalone installers with Crumb Writer’s GitHub build provenance. The Windows product is an Inno Setup executable for the current account. The Mac product is a universal drag-to-Applications disk image. Both include the document runtime and source companions. |

Development, issues, and packages belong in Alfredo’s repository. The fork points back to Clapgrep. Review any adopted changes individually and preserve their authorship. Contacting an upstream maintainer or submitting a patch upstream requires a separate instruction from Alfredo. Discussion-only messages and messages introduced with “Question:” ask for an answer without repository changes.

## Using the desktop

Choose a local folder, enter a phrase, and press Enter. Selecting a matching occurrence in the results tree opens its preview. The folder picker also responds to the platform’s Open shortcut. The Find shortcut returns focus to the search field. Escape cancels active work. The preview’s normal Copy action copies the selected text. Opening an original file in its usual application is explicit. Searches and previews leave original files intact.

The default is a case-insensitive literal search. Options expose regular expressions, case sensitivity, hidden and ignored entries, matching filenames, and a path glob. A filename match carries the `filename` kind and is distinct from a passage. Errors from individual files are counted while readable files continue. The search result limit is 10,000 occurrences. A truncated search is identified in its completion event. New searches and canceled previews have distinct process identities, so a stale response cannot revive an old selection.

The window groups matches by file, gives the preview more space, and adapts the split layout to the window. Qt supplies native control, menu and dialog behavior. Enabled prompts, typed field text, preview guidance and application-painted menu lettering are tested against a 4.5:1 glyph contrast threshold. Disabled actions and operating-system-owned controls are recorded separately. Use descriptive visible control labels, the platform font and palette, and local project artwork for the application icon. Check typed text, placeholder text, focus, selections and disabled controls against the surface they are drawn on. Settings retain the last folder, options, language choice and window layout. The clean-start test uses a fresh profile for each configuration.

The English–Spanish catalog in `desktop/qt/language.cpp` covers application controls, statuses, result locations and worker error categories. Keep both sides of every new string together, preserving placeholders and plural distinctions. The older GTK tree uses English source strings and the single `locale/es.po` translation. Preserve the original Spanish translator credits there. The operating system supplies the native file picker's own controls.

## Search, preview, and supported formats

| Reader | Searchable representation | Location and preview |
| --- | --- | --- |
| Text | UTF-8 including BOM, and BOM-marked UTF-16 LE/BE. A mixed folder may contain arbitrary binary data. | Numbered source lines, exact whitespace and a UTF-16 occurrence range usable by Qt. |
| HTML / Markdown | Document mode parses readable text across inline formatting. Source mode searches literal saved markup, links, tags and code. | Document blocks include a local rich fragment. Headings, emphasis, lists and code can be represented without loading a network resource. Source mode returns the original line. HTML recognizes supported declared character sets. |
| PDF | Poppler `pdftotext -bbox-layout` returns text, page dimensions and word coordinates. Files need a searchable text layer. | Each result identifies a page and line. The selected preview carries word boxes and offsets. Qt uses `pdftoppm` for the page raster, zoom, fit and navigation. On a page with repeated words the selected occurrence determines the highlighted words. |
| DOCX / ODT / FODT | Decode the document body in reading order, joining text divided by Word runs or ODF styled spans. Read supported headings, paragraphs and table cells. | Paragraphs or table positions, nearby text and selected occurrence. Page layout depends on the originating application and fonts. Flat ODF is read directly. |
| XLSX / ODS | Decode stored text and values. XLSX resolves workbook relationships and shared strings. ODS understands repeated row and cell coordinates. | Named sheet and A1-style cell address. Show nearby entries for context. Formula evaluation belongs to the spreadsheet application. |
| PPTX / ODP | Read presentation order and text blocks on slides. | Slide number and matching text. Text is presented in context without executing presentations or embedded objects. |
| RTF | Read paragraphs, character runs, escapes and supported character encodings. | Searchable paragraph text. Formatting commands and skipped control groups are excluded from the text. |
| Legacy DOC | Read the main text of Word 97–2003 compound documents directly in Rust. Follow the active piece table, honor character encodings and supported revision flags, and exclude deleted or hidden body text and non-body stories. The external `antiword` and UTF-8 map handle earlier supported variants. | Text passage with nearby context. Short Word 97 compound streams are covered by a genuine multilingual fixture. Unsupported variants, missing streams, and encryption produce diagnostics. |

A Document search for `The Latest Batch` matches `The **Latest** Batch` and `The <strong>Latest</strong> Batch`. A Source search for `<strong>` finds the markup itself. The same extractor constructs the searched and previewed text. Keep matching within individual document blocks. HTML and Markdown previews admit safe local formatting. Links, images, embedded objects and scripts must not cause network access or automatic opening of files. File names and search text can use Unicode regardless of interface language.

Inputs have a 64 MiB limit and extracted text has a 32 MiB limit. Readers also bound structural work, XML expansion, archive contents, or converter runtime as appropriate. HTML/Markdown Document mode accepts up to 8 MiB of markup. PDF extraction limits pages to 8,192 and has a timeout. Image-only and protected PDFs, corrupt packages, oversized files and missing converters receive distinguishable diagnostics where the reader identifies the cause. OCR, encrypted-document password entry, exhaustive support for Office revisions/comments/embedded objects, and page-faithful Office rendering are outside version 1.0.0.

## Source map

| Path | Responsibility |
| --- | --- |
| `bridge/src/main.rs`, `lib.rs`, `protocol.rs`, `paths.rs` | Rust worker entry points, file revisions, request validation, path identity, limits, results, and preview context. |
| `bridge/src/converters.rs`, `desktop/qt/runtime.cpp` | Locate the installed helpers and their font and character-map data, including when the desktop supplies a system-only environment. |
| `bridge/src/search.rs` | Ignore rules, folder traversal, regular expression/literal matching, filename and path options, excerpts and UTF-16 match indices. |
| `bridge/src/documents/` | Text decoding and lines, HTML and Markdown projection, Poppler PDF extraction and geometry. |
| `bridge/src/office.rs`, `bridge/src/office/` | Shared Office block model, package limits, OOXML, ODF, RTF, and legacy binary Word adapters. |
| `desktop/qt/application.*`, `mainwindow.*`, `language.*` | Application identity, startup, palette, search controls, results, keyboard, localization and saved settings. |
| `desktop/qt/workerclient.*`, `previewpane.*`, `pdfpane.*`, `credits.*` | Async process boundary, occurrence highlighting, PDF display, explicit opening of originals, credits and offline notices. |
| `CMakeLists.txt`, `cmake/`, `packaging/` | CMake/Cargo integration, desktop metadata, platform launch and install layout, native artifacts and checksums. |
| `tests/desktop/` | Original document fixtures, worker protocol verification, native Qt interaction and appearance tests, screenshots and machine-readable reports. |
| `tools/source_bundle.py`, `LICENSES/`, `NOTICE.md` | Exact dependency inventory and corresponding-source generation, third-party texts, dates and authorship. |
| `core/`, `gnome/`, `assets/`, `locale/`, Meson root files, `tests/freebsd/` | Clapgrep reference build and tests. The separate Rust workspace and GUI remain useful for comparison. |

Use descriptive names for source files, types and tests. Keep widget code responsible for interaction and display. Keep matching, file parsing and location generation in the worker. Put platform differences near their platform API, converter discovery or packaging boundary. The document readers share the same result representation across systems.

### Desktop-to-worker protocol

The front end launches the sibling `helloneedle-worker search` or `helloneedle-worker preview` with QProcess. It writes one UTF-8 JSON request and closes stdin. The worker streams one JSON object per stdout line. Separate the machine protocol on stdout from process diagnostics on stderr. The wire boundary avoids sharing toolkit objects, allocation ownership or an in-process ABI across Rust and C++.

A search supplies `folder`, `query`, `mode` (`document` or `source`) and the named option fields `regex`, `caseSensitive`, `hidden`, `ignored`, `filenames`, and `pathPattern`. A `match` event carries file identity, relative display path, kind, revision, location, label, excerpt, and UTF-16 offsets. `start` and `length` identify the displayed excerpt. `blockStart` and `blockLength` identify the occurrence in the entire preview block. On Unix `pathBytes` and `folderBytes` can carry base64-encoded native bytes. On Windows the normal path uses Unicode.

A preview supplies the same mode, file identity, revision, and selected location. `location.index` is zero-based in the decoded file. Human-facing line, page, paragraph and slide numbers are one-based. A `preview` response has a context array `blocks` and a selection `index` within that array. PDF context carries page-space word boxes. A `changed` response means the file revision has changed since the search. A `done` event carries the counts and truncation flag. Individual errors use stable codes translated by the interface. Canceling or replacing a request closes its process connection and prevents further updates to the view.

## Build, install, and package

Hello Needle builds with CMake, a C++17 toolchain for Qt 6.4 or newer and the separate Cargo lockfile under `bridge/`. CMake places the worker beside the GUI in the build and installed trees. System packages supply Qt, its native platform plugin and translations, Poppler utilities and antiword on FreeBSD and Linux. The FreeBSD pkg and Ubuntu Debian package install a native desktop entry. Uninstall them with `pkg delete helloneedle` or `apt remove helloneedle`. [Build instructions](README.md#build-from-source)

Windows and macOS follow the installer model used for Crumb Writer. Windows uses a per-user Inno Setup wizard. The Start menu and optional desktop shortcut open the root launcher, which looks for the private Qt and converter binaries in `bin`. A source build can still use an MSYS2 UCRT64 installation. The installer carries the imported runtime DLLs, required Qt plugins, Spanish translation, Poppler data and antiword character maps. The uninstaller is registered with Windows. [Windows installation and verification](packaging/WINDOWS.txt) · [Installer sources](packaging/windows/)

The macOS download is a Finder disk image containing **Hello Needle.app** and an **Applications** shortcut. The app includes a Rust worker, signed Mach-O executables, Qt's Cocoa plugin, runtime libraries, Spanish translations, font configuration, Poppler data and antiword maps. Search and preview locate helpers inside the application when launched from Finder. The build relocates library install names before signing. The universal image combines Apple Silicon and Intel machine code. The GUI uses native Cocoa on either processor. The app uses an ad hoc signature and the build supplies GitHub provenance. [Mac installation and verification](packaging/MACOS.txt) · [Disk image sources](packaging/macos/)

The test suite installs and launches the packaged application on a clean desktop with system-only search paths. The Windows checks exercise Start, setup repair, uninstall, native styles, PDFs and Word documents. The Mac tests mount the disk image read-only, copy the app through its Applications shortcut and verify its signature, document tools and native window. The Intel builder can construct the Apple Silicon slice using matching, signed MacPorts archives and Apple’s cross compiler. A native run and a cross-build are recorded separately.

All builds install `LICENSE`, `NOTICE.md`, component notices, and the Rust dependency inventory. FreeBSD/Linux use `share/doc/helloneedle`. The Mac uses `Contents/Resources/licenses`. The Windows and Mac installers also carry the bundled native runtime inventory and applicable distribution texts under `LICENSES/native`. Each release supplies the exact project revision and 110 locked Cargo dependencies as an offline source archive, plus the source and build recipes for the native software bundled in each installer. The `corresponding_source` field in `BUILD-ENVIRONMENT.json` names the source archive. [Provenance and license choices](NOTICE.md) · [GNU GPL](LICENSE)

The native workflow signs each verified installer and its checksum manifest through GitHub artifact attestations. The release workflow takes one native build run for the exact revision on `main`, verifies its repository, workflow identity, installers, detached signatures and complete matching sources, then checks the remote draft asset hashes. Publishing the draft is a separate choice. When repository release immutability is enabled, the published tag and asset bytes are locked and the release workflow verifies GitHub’s additional release record. [Native workflow](.github/workflows/helloneedle.yml) · [Release workflow](.github/workflows/installers-release.yml) · [Verify a download](README.md#verify-a-download)

## Verification, including native appearance

On a clean Windows installation, the Start menu test records exit codes for both the installed GUI and its launcher. The directory-picker fixture resolves the temporary profile path first because Windows can give the same folder a short name such as `RUNNER~1` and a full name. The installer tests also repair the installation in Spanish, remove it, and verify the original documents by checksum. [Windows installer tests](tests/installer/verify-windows.ps1)

For older Word documents, the worker requests UTF-8 text from antiword. The macOS package includes the character maps supplied by MacPorts. The `UTF-8.txt` map selects the bundled data directory. Antiword uses its separate `fontnames` table for formatted output. The clean-install test reads a Word for DOS document containing `café` so a missing or incorrect text mapping is visible. [Document conversion](bridge/src/converters.rs) · [Installed-document tests](tests/installer/runtime-check.py)

The native Word piece-table reader uses the MIT-licensed `cfb` crate for compound storage. The worker handles compressed and UTF-16 pieces and separates field results from field instructions. The short multilingual DOC and the corresponding text source are under `tests/desktop/fixtures/`. A second Word fixture contains a recorded deletion and manually hidden text.

The filename test passes the exact bytes returned by a search into the preview request. On a filesystem that accepts non-UTF-8 names, it uses a name containing the byte 0xFF. On APFS, which rejects that fixture, it uses a decomposed accented filename and verifies its exact bytes.

The fast worker check exercises the same process protocol used by the desktop. The fixture generator writes a three-page PDF with repeated words on page two, rich Word and ODF paragraphs split across formatting, an XLSX and ODS value at Quarterly!C7, a slide-two match, RTF, markup, and original plain text with an emoji before two hits. `fixtures/sample.doc` is a real binary Word file generated from the tracked original `sample.txt` with LibreOffice Writer's “MS Word 97” export filter. Hash comparisons confirm that search and preview leave the originals byte-identical.

```sh
cargo test --locked --manifest-path bridge/Cargo.toml
python3 tests/desktop/worker_suite.py --worker build-qt/bin/helloneedle-worker --require-doc
ctest --test-dir build-qt --output-on-failure
```

For the GUI, build the `helloneedle-gui-tests` target and run `tests/desktop/gui_matrix.py` through CTest on a graphical desktop. On FreeBSD or Linux use the provided `packaging/x11-tests.sh` when there is no graphical login. The native Qt plugin is XCB on both Unix systems, Windows on Windows, and Cocoa on macOS. Use fresh settings on first launch. Capture the window before the first query, menus, editable fields, selected results, document previews, file picker, licenses, and a narrow window. Exercise the keyboard, cancellation, empty results and repeated searches. Include Spanish, whose longer controls make clipping easier to spot. Test system appearance and explicit light/dark coverage under normal and increased scaling. A deliberately supplied test palette must be recorded as such. Keep any distinction between a hosted Windows Server and a Windows 11 desktop visible in the report.

For first-launch evidence, compare the captured window’s client area with Hello Needle’s rendered controls before saving a frame. A hosted Windows 11 desktop can put a setup window in front of the application. Capture Hello Needle by its native window handle and record which capture was verified. On macOS the menu bar is separate from the window. Record the translated titles published through AppKit and verify ownership before treating a menu-bar image as evidence.

Attach the native OS, architecture, Qt version, platform plugin, theme source, scaling, checks, numerical measurements, and screenshot files to each run. Review the rendered images as well as the numerical results. A text field can have sufficient measured contrast while being crowded into half a line. A menu can be readable while displaying a generic Qt placeholder icon. A palette on an offscreen plugin cannot establish the appearance of the native platform. Keep failed evidence and any reproducible defect with the correction until it passes on the affected platform. The Actions run also records package hashes and the source archive. [Build and appearance runs](https://github.com/AlfredoLlaquet/helloneedle/actions)

### Native appearance in 1.0.0

Each environment below passed 59 GUI checks across ten configurations. The tests used fresh profiles and the installed 1.0.0 worker. Separate smoke tests launched the packaged application, searched a real folder and opened the selected passage. The galleries preserve original PNGs from the native run for commit `011e2406e5cc`. [Build, packages, manifests and full screenshot sets](https://github.com/AlfredoLlaquet/helloneedle/actions/runs/45262698979)

| Tested system | Qt | Screenshots | Window and controls |
| --- | --- | ---: | --- |
| FreeBSD 15.1-RELEASE-p6, amd64 | 6.11.3 | 77 | [First launch](docs/screenshots/1.0.0/freebsd-first-launch.png) · [Spanish dark mode at 150%](docs/screenshots/1.0.0/freebsd-spanish-dark.png) |
| Ubuntu 24.04.5 LTS, x86-64 | 6.4.2 | 81 | [First launch](docs/screenshots/1.0.0/ubuntu-first-launch.png) · [Spanish dark mode at 150%](docs/screenshots/1.0.0/ubuntu-spanish-dark.png) |
| Windows 11 Enterprise, Arm64 running the x64 build | 6.12.0 | 84 | [First launch](docs/screenshots/1.0.0/win11-first-launch.png) · [Spanish dark mode at 150%](docs/screenshots/1.0.0/win11-spanish-dark.png) |
| Windows Server 2025 Datacenter, x64 | 6.12.0 | 84 | [First launch](docs/screenshots/1.0.0/winserver-first-launch.png) · [Spanish dark mode at 150%](docs/screenshots/1.0.0/winserver-spanish-dark.png) |
| macOS Sequoia 15.7.9, Intel | 6.12.0 | 84 | [First launch](docs/screenshots/1.0.0/macintel-first-launch.png) · [Spanish dark mode at 150%](docs/screenshots/1.0.0/macintel-spanish-dark.png) |

The first-launch column shows the host’s default appearance. The dark-mode images show Spanish controls at increased scaling with the options expanded. FreeBSD and Ubuntu used X11 with Openbox. Their explicit dark runs used a test palette with the native XCB plugin. Windows used its platform settings. On macOS Sequoia, the application used Cocoa. The desktop kept its light system menu bar in both application themes. The tests verified that Hello Needle owned that menu bar and that all four application menus showed their English or Spanish titles. The screenshot manifests identify the capture source for each window.

A separate installer build at revision `3ec29a516602` passed 59 native Cocoa checks across ten configurations on Apple Silicon. [Native Apple Silicon build and tests](https://github.com/AlfredoLlaquet/helloneedle/actions/runs/45282805114/job/138071728271) · [Complete screenshot set](https://github.com/AlfredoLlaquet/helloneedle/actions/runs/45282805114/artifacts/14870284448)

| Tested system | Qt | Screenshots | Window and controls |
| --- | --- | ---: | --- |
| macOS Tahoe 26.7.2, Apple Silicon (arm64) | 6.12.0 | 84 | [First launch](docs/screenshots/1.0.0/macarm-first-launch.png) · [Spanish dark mode at 150%](docs/screenshots/1.0.0/macarm-spanish-dark.png) |

The search suite verified 15 selected occurrences across 13 document formats, checked Word 97 text with the converter path empty and compared the original files before and after the tests. The corresponding-source archive also built and installed the Qt application with an empty Cargo registry and all 110 bundled dependencies. The resulting program passed the installed search and preview tests.

## Historical FreeBSD reference

The original baseline is Clapgrep `v26.09` plus the upstream translation update at `5642bbee94b6684192fa075403222f2d4e2f7559`. The FreeBSD port revision `a88d6100d68c09405abb7f21ff55b3f1afaeb714` added native installation and real installed GTK application tests. The layout revision verified FreeBSD 15.1-RELEASE-p6 amd64 in both English and Spanish. In each locale Meson validated desktop metadata and ran the Rust workspace tests. Four installed GUI searches found a plain-text passage on line 3, a PDF match on page 2, and a DOCX paragraph, then covered empty and repeated searches. The test also exercised the optional Nautilus integration. The inherited Linux Flatpak workflow is disabled. The dedicated Hello Needle jobs build the new application independently. [Verified reference run](https://github.com/AlfredoLlaquet/helloneedle/actions/runs/45243533421) · [Historical build, installation and tests](https://github.com/AlfredoLlaquet/helloneedle/blob/b96ee0e4c4ca216b6a35bced52e6457ac7ff497f/README.md#build-the-current-freebsd-reference)
