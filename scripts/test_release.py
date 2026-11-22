"""Exercise release failures before any assets can be published."""

import hashlib
from pathlib import Path
import tarfile
import tempfile
import unittest

import release


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "Cargo.toml").write_text(
            '[package]\nname = "ruvx"\nversion = "0.1.0"\n', encoding="utf-8")
        (self.root / "Cargo.lock").write_text(
            '[[package]]\nname = "ruvx"\nversion = "0.1.0"\n', encoding="utf-8")
        (self.root / "README.md").write_text("Read docs/guide.md\n", encoding="utf-8")
        (self.root / "LICENSE").write_text("Test license\n", encoding="utf-8")
        (self.root / "docs").mkdir()
        (self.root / "docs/guide.md").write_text("Guide\n", encoding="utf-8")
        self.binary = self.root / "ruvx"
        self.binary.write_text('#!/bin/sh\nprintf "ruvx 0.1.0\\n"\n', encoding="utf-8")
        self.binary.chmod(0o755)
        self.output = self.root / "dist"

    def test_version_and_prerelease(self):
        self.assertEqual(release.check_version(self.root, "v0.1.0"), "0.1.0")
        for name in ("Cargo.toml", "Cargo.lock"):
            path = self.root / name
            path.write_text(path.read_text().replace("0.1.0", "0.2.0-rc.1"), encoding="utf-8")
        self.assertEqual(release.check_version(self.root, "v0.2.0-rc.1"), "0.2.0-rc.1")

    def test_tag_mismatch(self):
        for tag in ("0.1.0", "v0.2.0", "v../../escape"):
            with self.subTest(tag=tag), self.assertRaisesRegex(ValueError, "must match"):
                release.check_version(self.root, tag)

    def test_lockfile_mismatch(self):
        (self.root / "Cargo.lock").write_text(
            '[[package]]\nname = "ruvx"\nversion = "0.0.1"\n', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Cargo.lock"):
            release.check_version(self.root, "v0.1.0")

    def test_archive_contains_executable_docs_and_license_only(self):
        (self.root / "secret.env").write_text("never ship", encoding="utf-8")
        archive = release.package_binary(
            self.root, "v0.1.0", release.TARGETS[0], self.binary, self.output)
        prefix = archive.name.removesuffix(".tar.gz")
        with tarfile.open(archive) as source:
            self.assertEqual(set(source.getnames()), {
                prefix, f"{prefix}/ruvx", f"{prefix}/README.md", f"{prefix}/LICENSE",
                f"{prefix}/docs", f"{prefix}/docs/guide.md",
            })
            self.assertEqual(source.getmember(f"{prefix}/ruvx").mode, 0o755)
            self.assertEqual(source.extractfile(f"{prefix}/ruvx").read(), self.binary.read_bytes())
            self.assertEqual(source.extractfile(f"{prefix}/LICENSE").read(), b"Test license\n")

    def test_wrong_binary_version(self):
        self.binary.write_text('#!/bin/sh\nprintf "ruvx 0.0.1\\n"\n', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "Binary version"):
            release.package_binary(
                self.root, "v0.1.0", release.TARGETS[0], self.binary, self.output)
        self.assertFalse(self.output.exists())

    def test_missing_binary(self):
        self.binary.unlink()
        with self.assertRaises(FileNotFoundError):
            release.package_binary(
                self.root, "v0.1.0", release.TARGETS[0], self.binary, self.output)

    def test_unsupported_target(self):
        with self.assertRaisesRegex(ValueError, "Unsupported"):
            release.package_binary(
                self.root, "v0.1.0", "x86_64-pc-windows-msvc", self.binary, self.output)

    def create_packages(self):
        self.output.mkdir()
        for target in release.TARGETS:
            (self.output / release.archive_name("v0.1.0", target)).write_bytes(target.encode())

    def test_checksums_cover_every_package(self):
        self.create_packages()
        path = release.write_checksums(self.root, "v0.1.0", self.output)
        lines = path.read_text().splitlines()
        self.assertEqual(len(lines), 4)
        for line in lines:
            digest, name = line.split("  ")
            self.assertEqual(digest, hashlib.sha256((self.output / name).read_bytes()).hexdigest())

    def test_checksums_reject_missing_package(self):
        self.create_packages()
        (self.output / release.archive_name("v0.1.0", release.TARGETS[0])).unlink()
        with self.assertRaisesRegex(ValueError, "missing="):
            release.write_checksums(self.root, "v0.1.0", self.output)
        self.assertFalse((self.output / "SHA256SUMS").exists())

    def test_checksums_reject_stale_package(self):
        self.create_packages()
        (self.output / "ruvx-v0.0.1-old.tar.gz").write_bytes(b"old")
        with self.assertRaisesRegex(ValueError, "extra="):
            release.write_checksums(self.root, "v0.1.0", self.output)


if __name__ == "__main__":
    unittest.main()
