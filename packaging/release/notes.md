Hello Needle @VERSION@ finds words and phrases in local documents, including searchable PDF, Microsoft Office and OpenDocument files. Its Qt 6 interface is available in English and Spanish.

| Desktop | Download | Installation |
| --- | --- | --- |
| Windows x64 | [Hello Needle Setup](https://github.com/@REPOSITORY@/releases/download/v@VERSION@/HelloNeedle-@VERSION@-Windows-x64-Setup.exe) | Run Setup and open Hello Needle from Start. |
| macOS on Intel or Apple Silicon | [Universal disk image](https://github.com/@REPOSITORY@/releases/download/v@VERSION@/HelloNeedle-@VERSION@-macOS-Universal.dmg) | Open the disk image and drag Hello Needle to Applications. |

Both installers include Qt, the document converters and their runtime files. GitHub Actions signed the installer build records. Verify the downloaded file with the corresponding command.

```
gh attestation verify HelloNeedle-@VERSION@-Windows-x64-Setup.exe --repo @REPOSITORY@
gh attestation verify HelloNeedle-@VERSION@-macOS-Universal.dmg --repo @REPOSITORY@
```

Windows may identify the publisher as unknown. The macOS app is ad hoc signed. Gatekeeper may request approval on first launch. The matching project source, native runtime sources, build recipes, checksum lists and detached GitHub attestations accompany the installers. For a split source archive, concatenate the numbered parts in order before extracting.

Created by Alfredo Llaquet-Alsina. Programmed by ChatGPT. [Credits and licensing](https://github.com/@REPOSITORY@/blob/v@VERSION@/NOTICE.md)
