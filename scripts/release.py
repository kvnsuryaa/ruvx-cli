#!/usr/bin/env python3
"""Version checks, binary packaging, and checksums (Python 3.11+, stdlib only)."""

import argparse
import hashlib
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib


ROOT = Path(__file__).resolve().parent.parent
TARGETS = (
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
)


def check_version(root, tag):
    with (root / "Cargo.toml").open("rb") as source:
        package = tomllib.load(source)["package"]
    version = package["version"]
    if tag != f"v{version}":
        raise ValueError(f"Tag {tag!r} must match Cargo.toml: v{version}")
    with (root / "Cargo.lock").open("rb") as source:
        locked = tomllib.load(source)["package"]
    if not any(p["name"] == package["name"] and p["version"] == version
               and "source" not in p for p in locked):
        raise ValueError("Cargo.lock must contain the current package version")
    return version


def archive_name(tag, target):
    if target not in TARGETS:
        raise ValueError(f"Unsupported release target: {target}")
    return f"ruvx-{tag}-{target}.tar.gz"


def package_binary(root, tag, target, binary, output):
    version = check_version(root, tag)
    filename = archive_name(tag, target)
    actual = subprocess.check_output([str(binary.resolve()), "--version"], text=True).strip()
    if actual != f"ruvx {version}":
        raise ValueError(f"Binary version {actual!r} does not match ruvx {version}")
    output.mkdir(parents=True, exist_ok=True)
    archive = output / filename
    with tempfile.TemporaryDirectory(prefix="ruvx-package-") as temporary:
        directory = Path(temporary) / filename.removesuffix(".tar.gz")
        directory.mkdir()
        shutil.copy2(binary, directory / "ruvx")
        (directory / "ruvx").chmod(0o755)
        for name in ("README.md", "LICENSE"):
            shutil.copy2(root / name, directory / name)
        shutil.copytree(root / "docs", directory / "docs")
        with tarfile.open(archive, "w:gz") as destination:
            destination.add(directory, arcname=directory.name)
    return archive


def write_checksums(root, tag, directory):
    check_version(root, tag)
    expected = {archive_name(tag, target) for target in TARGETS}
    actual = {path.name for path in directory.glob("*.tar.gz")}
    if actual != expected:
        raise ValueError(f"Package set is incomplete or unexpected; "
                         f"missing={sorted(expected - actual)}, extra={sorted(actual - expected)}")
    lines = []
    for name in sorted(expected):
        with (directory / name).open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        lines.append(f"{digest}  {name}\n")
    checksums = directory / "SHA256SUMS"
    checksums.write_text("".join(lines), encoding="utf-8")
    return checksums


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    version = commands.add_parser("check-version", help="Check tag, manifest, and lockfile")
    version.add_argument("tag")
    package = commands.add_parser("package", help="Package a built and tested binary")
    package.add_argument("tag")
    package.add_argument("--target", choices=TARGETS, required=True)
    package.add_argument("--binary", type=Path, required=True)
    package.add_argument("--output", type=Path, default=ROOT / "target/dist")
    checksums = commands.add_parser("checksums", help="Require all four packages and hash them")
    checksums.add_argument("tag")
    checksums.add_argument("--directory", type=Path, default=ROOT / "target/dist")
    args = parser.parse_args()
    try:
        if args.command == "check-version":
            result = check_version(ROOT, args.tag)
        elif args.command == "package":
            result = package_binary(ROOT, args.tag, args.target, args.binary, args.output)
        else:
            result = write_checksums(ROOT, args.tag, args.directory)
    except (ValueError, OSError, subprocess.SubprocessError) as error:
        parser.exit(1, f"release: {error}\n")
    print(result)


if __name__ == "__main__":
    main()
