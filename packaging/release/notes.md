Hello Needle @VERSION@ finds words and phrases in local documents, including searchable PDF, Microsoft Office and OpenDocument files. Its Qt 6 interface is available in English and Spanish.

| Desktop | Download | Installation |
| --- | --- | --- |
| FreeBSD 15.1 amd64 | [FreeBSD package](https://github.com/@REPOSITORY@/releases/download/v@VERSION@/helloneedle-@VERSION@.pkg) | Run `pkg install ./helloneedle-@VERSION@.pkg` as root. |
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
