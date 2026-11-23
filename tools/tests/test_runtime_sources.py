"""Regression checks for native installer source ownership and checksum verification."""
# SPDX-License-Identifier: GPL-3.0-or-later

import hashlib
import json
import tempfile
import unittest
from pathlib import Path

from tools.runtime_provenance import common, macports, msys2


class RuntimeSourcesTests(unittest.TestCase):
    def test_exact_package_owner_instead_of_shared_dll_basename(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            entries = {}
            for package, subdirectory in [("one", "bin"), ("two", "plugins/platforms")]:
                origin = f"ucrt64/{subdirectory}/helper.dll"
                info = root / "var/lib/pacman/local" / package
                info.mkdir(parents=True)
                (info / "desc").write_text(f"%NAME%\nmingw-w64-ucrt-x86_64-{package}\n\n%VERSION%\n1:2.3-4\n")
                (info / "files").write_text(f"%FILES%\n{origin}\n")
                entries[origin] = {"source":str(root / origin)}
            result = msys2.owners(root, entries)
            self.assertEqual(result["mingw-w64-one-2.3-4"]["origins"], ["ucrt64/bin/helper.dll"])
            self.assertEqual(result["mingw-w64-two-2.3-4"]["origins"], ["ucrt64/plugins/platforms/helper.dll"])
            entries["bad"] = {"source": str(root / "not-owned.dll")}
            with self.assertRaisesRegex(RuntimeError, "Unowned MSYS2"):
                msys2.owners(root, entries)

    def test_signed_source_metadata_requires_the_exact_recipe_and_its_checkout(self):
        metadata = {"unit/.SRCINFO": (
            "pkgbase = unit\npkgver = 2\npkgrel = 1\nsource = stable::git+https://example.invalid/lib.git#commit=abcdef\n"
            "source = https://example.invalid/unit-2.tar.xz\nsource_i686 = other-arch.patch\n")}
        members = {"unit/.SRCINFO", "unit/PKGBUILD", "unit/stable/objects/pack/pack.dat", "unit/unit-2.tar.xz"}
        self.assertEqual(len(msys2.signed_source_inputs("unit-2-1", metadata, members)), 2)
        with self.assertRaisesRegex(RuntimeError, "missing its declared input"):
            msys2.signed_source_inputs("unit-2-1", metadata, members - {"unit/unit-2.tar.xz"})
        with self.assertRaisesRegex(RuntimeError, "does not match installed"):
            msys2.signed_source_inputs("unit-2-2", metadata, members)

    def test_macports_distfiles_verify_the_actual_cached_bytes(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "test-1.tar.gz"
            path.write_bytes(b"exact bytes")
            expected = hashlib.sha256(path.read_bytes()).hexdigest()
            listing = f"--->  Distfiles for test\n[{path.name}] {path}\n sha256: {expected}\n size: 11\n  https://example.invalid/source\n"
            item, = macports.distfiles(listing)
            dest = Path(tmp) / "copied"
            self.assertEqual(macports.retrieve(item, dest), "MacPorts distfile cache")
            self.assertEqual(dest.read_bytes(), b"exact bytes")
            path.write_bytes(b"changed now")
            self.assertFalse(macports.matches(path, item["checksums"]))

    def test_untracked_runtime_is_rejected_and_derived_origins_are_explicit(self):
        with tempfile.TemporaryDirectory() as tmp:
            base = Path(tmp)
            stage = base / "app"
            stage.mkdir()
            source = base / "original.dll"
            source.write_bytes(b"original")
            (stage / "one.dll").write_bytes(b"relocated")
            (stage / "two.dll").write_bytes(b"derived")
            manifest = base / "files.json"
            manifest.write_text(json.dumps([{"installed":"one.dll", "source":str(source)}]))
            with self.assertRaisesRegex(ValueError, "two.dll"):
                common.manifest_files(manifest, stage)
            files = common.manifest_files(manifest, stage, {"two.dll"})
            self.assertEqual(files["one.dll"]["preInstallerSha256"], hashlib.sha256(b"relocated").hexdigest())


if __name__ == "__main__":
    unittest.main()
