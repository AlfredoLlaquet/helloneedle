# Clapgrep for FreeBSD

This repository contains the source for an independently maintained FreeBSD port of Clapgrep, a graphical application that searches inside plain text, PDFs, and Office documents. The upstream project is at <https://github.com/luleyleo/clapgrep>.

Programmed by **ChatGPT** for Alfredo Llaquet-Alsina.

## Install on FreeBSD

Tested on FreeBSD 15.1-RELEASE-p6 (amd64). These commands build Clapgrep with Rust and Meson and install it under `/usr/local`.

1. In a root shell, install the dependencies.

   ```sh
   pkg install git rust meson ninja pkgconf gettext-tools gtk4 libadwaita \
       gtksourceview5 poppler-glib blueprint-compiler desktop-file-utils AppStream
   ```

2. As your regular user, clone this fork and build Clapgrep. Cargo downloads the dependencies recorded in `Cargo.lock` on the first build.

   ```sh
   git clone https://github.com/AlfredoLlaquet/clapgrep-freebsd.git
   cd clapgrep-freebsd
   meson setup build --prefix=/usr/local
   CARGO_BUILD_JOBS=2 meson compile -C build
   ```

3. From the same `clapgrep-freebsd` directory, install with root privileges.

   ```sh
   su root -c 'meson install -C build --no-rebuild'
   ```

4. Launch Clapgrep from your desktop’s application menu or run it as your regular user. To start with a particular folder selected, pass its path as an argument.

   ```sh
   clapgrep
   ```

## Update on FreeBSD

As your regular user, start in the top level of your existing `clapgrep-freebsd` clone, the directory containing `meson.build`. Pull the latest changes on `main` and rebuild.

```sh
git switch main
git pull --ff-only origin main
CARGO_BUILD_JOBS=2 meson compile -C build
```

From that same directory, reinstall with root privileges. Close and reopen Clapgrep to run the updated version.

```sh
su root -c 'meson install -C build --no-rebuild'
```

## Run the automated tests (optional)

From the top level of your clone, run the upstream test suite as your regular user after building. Meson validates the desktop entry and AppStream metadata and runs the Rust workspace tests. It prints details for any failure and writes the full log to `build/meson-logs/testlog.txt`.

```sh
CARGO_BUILD_JOBS=2 meson test -C build --print-errorlogs --timeout-multiplier 20
```

The fork also tests the installed application in a temporary graphical session. In a root shell, install the test dependencies and initialize the D-Bus machine ID if needed.

```sh
pkg install xorg-vfbserver dbus python3 py312-atspi py312-pycairo dejavu xdotool
dbus-uuidgen --ensure
```

After installing Clapgrep, run the following from the top level of the clone as your regular user. The checks open fresh virtual X11 sessions in English and Spanish. They verify the matching text line, PDF page, and Word paragraph, repeat searches in the same window, and use the Nautilus preference to install and remove the extension in a temporary data directory.

```sh
TZ=UTC sh tests/freebsd/installed-search.sh en
TZ=Europe/Madrid sh tests/freebsd/installed-search.sh es
```

Everything below the horizontal rule is the upstream Clapgrep README, reproduced verbatim from the revision used by this fork. Its Flathub link is for the upstream Linux release.

<br><br>

---

# Clapgrep

One app to search through all your files

[![Get it on Flathub](https://flathub.org/api/badge?svg&locale=en)](https://flathub.org/apps/de.leopoldluley.Clapgrep)

## Description

Ever had a folder full of PDF files, where you knew, somewhere in there, is what you're looking for. But you did not know in which file. So you had to search each of them at a time...

With Clapgrep this is no longer necessary! Simply open the folder with Clapgrep, enter the search term, and Clapgrep will do all the hard work of finding out on which page / line in which file the information is that you are looking for!

Clapgrep can currently search all sorts of text files, PDFs and Office documents, with more to come.

![screenshot of the app](assets/screenshot-1.png)

## Translations

[![Translation status](https://hosted.weblate.org/widget/clapgrep/clapgrep/multi-auto.svg)](https://hosted.weblate.org/engage/clapgrep/)

You can help translate Clapgrep on [Weblate](https://hosted.weblate.org/engage/clapgrep/) .

## Building from Source

See [CONTRIBUTING.md](/CONTRIBUTING.md) for code contribution and build instructions.
