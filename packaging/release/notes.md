Hello Needle @VERSION@ finds words and phrases in local documents, including searchable PDF, Microsoft Office and OpenDocument files. Its Qt 6 interface is available in English and Spanish.

**New in @VERSION@**

- Navigate the live results by document or occurrence with the four buttons below **Results** or their keyboard shortcuts. The preview centers the selected match.
- Search multiple files in parallel. **File → Settings…** remembers your preferred number of search workers and simultaneous PDF conversions.
- Find text in Windows-1252 HTML and Markdown, Word-exported text containing nonbreaking-hyphen markers, and OpenDocument XML with a supported declared legacy encoding.
- Search large text files and JSON exports as they are read. A match can appear millions of lines into a file or inside a long single line. Selecting it opens the passage around that occurrence.
- Search older Western European text with its original accents, saved HTML containing control characters, heavily formatted DOCX files, and older Word documents that use styles to control text visibility.
- Search book-length PDFs a page at a time. If the PDF text extractor crashes while locating words on the printed page, Hello Needle retries in text mode. Successful matches keep their page numbers and the text preview highlights the selected occurrence.
- Watch **Files scanned** advance as Hello Needle examines each file. Videos, archives and image-only PDFs count toward progress and pass quietly. **Details** shows the reason when a searchable document fails.

| Desktop | Download | Installation |
| --- | --- | --- |
@FREEBSD_DOWNLOADS@
| Ubuntu 24.04 amd64 | [Debian package](https://github.com/@REPOSITORY@/releases/download/v@VERSION@/helloneedle_@VERSION@_amd64.deb) | Run `sudo apt install ./helloneedle_@VERSION@_amd64.deb`. |
| Windows x64 | [Hello Needle Setup](https://github.com/@REPOSITORY@/releases/download/v@VERSION@/HelloNeedle-@VERSION@-Windows-x64-Setup.exe) | Run Setup and open Hello Needle from Start. |
| macOS on Intel or Apple Silicon | [Universal disk image](https://github.com/@REPOSITORY@/releases/download/v@VERSION@/HelloNeedle-@VERSION@-macOS-Universal.dmg) | Open the disk image and drag Hello Needle to Applications. |

FreeBSD and Ubuntu install Qt and the document converters through their package managers. The Windows and macOS installers include these dependencies. Windows may identify the publisher as unknown. The macOS app is ad hoc signed. Gatekeeper may request approval on first launch. [Current installation and usage guide](https://github.com/@REPOSITORY@#readme)

GitHub Actions signed the [release checksum list](https://github.com/@REPOSITORY@/releases/download/v@VERSION@/SHA256SUMS). Download the list and its [detached receipt](https://github.com/@REPOSITORY@/releases/download/v@VERSION@/HelloNeedle-@VERSION@-Release.sigstore.json), then verify the list with GitHub CLI.

```sh
gh attestation verify SHA256SUMS --repo @REPOSITORY@ --bundle HelloNeedle-@VERSION@-Release.sigstore.json --signer-workflow @REPOSITORY@/.github/workflows/installers-release.yml
```

Compare the SHA-256 of the downloaded package with its entry in the verified list. FreeBSD provides `sha256` and Ubuntu provides `sha256sum`. The Windows and macOS installers also have individual build attestations.

```sh
gh attestation verify HelloNeedle-@VERSION@-Windows-x64-Setup.exe --repo @REPOSITORY@
gh attestation verify HelloNeedle-@VERSION@-macOS-Universal.dmg --repo @REPOSITORY@
```

The matching project source, bundled runtime sources, build recipes and checksums accompany the downloads. For a split source archive, concatenate the numbered parts in order before extracting.

Created by Alfredo Llaquet-Alsina. Programmed by ChatGPT. [Credits and licensing](https://github.com/@REPOSITORY@/blob/v@VERSION@/NOTICE.md)
