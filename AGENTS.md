# Clapgrep for FreeBSD — Project Purpose and Working Rules

> [!IMPORTANT]
> **Never send any request upstream.** Treat Clapgrep and all of its upstream dependencies as read-only. Never push changes upstream, open an issue or pull request there, ask for a feature, review, or support, or contact upstream maintainers by comment, discussion, email, or private message. This applies to everyone acting on behalf of this project, including automated workflows. Reading documentation and fetching upstream source and tags are part of normal maintenance.
>
> Keep fixes and proposals in `AlfredoLlaquet/clapgrep-freebsd`. Check the GitHub destination before opening an issue or pull request. For a pull request, the **base repository** must be this fork.

## Purpose and name

Use the following repository description verbatim.

> An independently maintained FreeBSD port of Clapgrep

Keep the application name **Clapgrep**. The goal is to build and run the existing graphical document search application natively on FreeBSD. Preserve its Rust search engine, GTK 4 interface, and behavior when searching plain text, PDFs, and Office documents. Keep downstream changes focused so future stable releases are easy to merge. [Upstream Clapgrep](https://github.com/luleyleo/clapgrep)

Programmed by **ChatGPT** for Alfredo Llaquet-Alsina.

## Marking FreeBSD code changes

Use one pair of `CG4FBS` markers around each logical block changed for this port. Use the same marker words in every language and the file’s own comment syntax. Add a short reason to the opening marker when it helps explain the change.

For a replacement, start with `CG4FBS BEGIN`. Immediately below it, reproduce the complete upstream block as comments, preserving its line breaks and indentation. Put `CG4FBS REPLACEMENT` on its own comment line, followed by the active replacement and then `CG4FBS END`. For example, a Meson platform check can be extended as follows.

```meson
# CG4FBS BEGIN — Refresh installed desktop caches on FreeBSD.
# if build_machine.system() == 'linux'
# CG4FBS REPLACEMENT
if build_machine.system() in ['linux', 'freebsd']
# CG4FBS END
```

For an entirely new block, put the active code directly between `CG4FBS BEGIN ADDITION` and `CG4FBS END`. The following example uses Rust comment syntax.

```rust
// CG4FBS BEGIN ADDITION
#[cfg(target_os = "freebsd")]
const PLATFORM_NAME: &str = "FreeBSD";
// CG4FBS END
```

Keep the commented original in sync with the upstream revision used by this fork. When a later release changes that block, refresh the commented copy and review its FreeBSD replacement in the same update.

## Following Clapgrep releases

1. Use `origin` for this fork and `upstream` for `luleyleo/clapgrep`. Fetch and merge each new stable upstream tag into this fork. Resolve conflicts here and review which FreeBSD patches are still needed.
2. Before publishing, build and test the merged revision in a **FreeBSD virtual machine**. Run the Meson test suite, including the Cargo workspace tests. Launch the installed application in a graphical session and verify searches of plain text, PDFs, and Office documents. Fix FreeBSD regressions in this fork.
3. Identify the upstream tag in each downstream release. Record the FreeBSD version, architecture, tests actually run, and any gaps. Publish a tagged source archive corresponding to that release.

### Native FreeBSD validation

Keep the `FreeBSD` workflow (`.github/workflows/freebsd.yml`) enabled for this fork. Keep the inherited `CI` workflow (`.github/workflows/ci.yaml`), which builds a Linux Flatpak and checks Rust formatting and linting, disabled in the fork’s GitHub Actions settings. Keep its file synchronized with upstream and recheck the setting if upstream renames or replaces the workflow.

The initial FreeBSD work dates from November 23, 2026. The fork starts at upstream commit `5642bbee94b6684192fa075403222f2d4e2f7559`, two commits after `v26.09`. Its validation target is FreeBSD 15.1-RELEASE on amd64. Configure and compile with Meson and Cargo as a regular user. Install under `/usr/local` with root privileges, then run the tests and the installed application as the regular user.

Keep coverage for these behaviors when integrating upstream changes.

- The installed `clapgrep` executable opens a GTK window and accepts a search folder. Its desktop entry and application resources work from the installation prefix.
- A search finds known matches in plain text, PDF, and Office files. Check the matching file and its line or page where the format supplies one. Include a file without the search term.
- Exercise successive short searches in one window. Append each match on the GTK main context before processing the completion message, so a fast search displays its results. A new search must clear the previous results, including when the new query has no matches.
- On a native desktop, the optional Nautilus integration uses the account’s XDG data directory. The search-folder row must open without GTK critical errors.
- The desktop-entry and AppStream metadata checks run alongside the Cargo workspace tests. Use a virtual X11 display and fresh per-user XDG settings for the graphical checks.

Build and install once per VM run. Exercise the installed interface in English under `UTC` and in Spanish under `Europe/Madrid`, with fresh settings for each run. Record which checks used a virtual display and which desktop sessions received additional manual testing.

### Keeping the README in sync

Maintain the FreeBSD notice at the top of `README.md`. Include the programming credit from “Purpose and name” and the full, clickable upstream project URL. Keep FreeBSD-specific README edits in that notice. Preserve the visible space before the first horizontal rule (`---`) and the blank line after it.

Keep **Install on FreeBSD** in step with commands verified in the VM. Include the dependencies, clone URL for this fork, regular-user build, privileged installation, and desktop launch command. Keep a separate **Update on FreeBSD** section for an existing clone. Identify the starting directory and show how to pull, rebuild as a regular user, and reinstall with root privileges. Put the optional automated tests after the installation and update instructions. Explain what Meson tests, where to run the command, how to display failure output, and where Meson saves the full log.

When integrating an upstream tag or commit, copy the complete `README.md` from that exact revision below the divider. Verify that the bytes after the divider and its blank line match the upstream file.

## Licensing and FreeBSD packages

Clapgrep is licensed under **GNU GPL version 3 or later**. Preserve copyright and license notices, retain the notices for bundled components, and identify and date downstream modifications. When distributing a binary, make its complete corresponding source available, including the FreeBSD patches and build scripts used for the release. [License](LICENSE) · [Upstream license declarations](assets/de.leopoldluley.Clapgrep.metainfo.xml.in.in)

The proposed FreeBSD package name is `clapgrep`. A ports maintainer can keep the build recipe and checksum in the separate FreeBSD Ports Collection and fetch a tagged source archive from this fork. That work can be done independently of Alfredo with read-only access to this repository. The FreeBSD Project decides on inclusion and can mirror the source archive and distribute binary packages.

For a future ports recipe, `Cargo.lock` records the dependency versions and registry checksums. Use it to prepare Cargo distfiles for an offline build of the tagged source.

## Working with Alfredo

ChatGPT handles the engineering work Alfredo requests, including implementation, builds, testing, upstream release integration, documentation, packaging, and publication of authorized releases. Alfredo provides authorization when a step requires it, performs any additional testing he chooses, and uses the application.

When Alfredo introduces a message with **`Question:`**, answer the question only. Wait for a separate instruction before acting on it.

## Keeping this guide current

Update this file whenever a new project decision is made, an existing decision changes, or something relevant needs to be taken into account in future work. Make the update as part of the work it governs. Replace superseded guidance and distinguish settled decisions from open questions.
