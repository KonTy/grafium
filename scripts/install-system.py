#!/usr/bin/env python3
"""Install a Grafium build system-wide, keeping nothing older.

After scripts/deploy-local.sh has installed and verified a build in ~/.local:

    pkexec python3 scripts/install-system.py BUILD_DIR SHA256 [VERSION]

BUILD_DIR is that build (the directory ~/.local/bin/grafium runs from) and
SHA256 the digest of its grafium-bin, so a build changed after you checked it
is refused. The build is copied into /opt/grafium, every file is verified, and
/usr/bin/grafium is switched to it in one step. Once the switch succeeded,
older /opt/grafium builds and the previous launcher's backup are deleted: we
can always rebuild. A build that a running Grafium still maps is kept until a
later install or prune.

    pkexec python3 scripts/install-system.py --prune

only removes what the current /usr/bin/grafium no longer needs.

GRAFIUM_SYSTEM_PREFIX puts /opt, /var/backups and /usr/bin under another
directory, for tests; it is the only way to run this without root.
"""
from __future__ import annotations

import hashlib
import os
from pathlib import Path
import re
import shutil
import sys
import tempfile

PREFIX = os.environ.get("GRAFIUM_SYSTEM_PREFIX")
BASE = Path(PREFIX) if PREFIX else Path("/")
ROOT = BASE / "opt/grafium"
BACKUPS = BASE / "var/backups/grafium"
LAUNCHER = BASE / "usr/bin/grafium"


def digest(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def sync(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | (os.O_DIRECTORY if path.is_dir() else 0))
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def in_use(build: Path) -> bool:
    """Whether a running process maps a file of `build` (its executable or a library)."""
    needle = f"{build}/"
    for maps in Path("/proc").glob("[0-9]*/maps"):
        try:
            if needle in maps.read_text(errors="replace"):
                return True
        except OSError:
            continue
    return False


def current_build() -> Path | None:
    """The build /usr/bin/grafium runs, if it is one of ours."""
    try:
        text = LAUNCHER.read_text(errors="replace")
    except OSError:
        return None
    match = re.search(r'^exec "?(.+?)/grafium-bin"? "\$@"$', text, re.M)
    return Path(match.group(1)) if match else None


def prune(keep: Path | None) -> None:
    """Delete every build but `keep` (and builds still in use) and every launcher backup."""
    removed = 0
    if ROOT.is_dir():
        for build in sorted(ROOT.glob("build-*")):
            if not build.is_dir() or build.is_symlink() or build == keep:
                continue
            if in_use(build):
                print(f"kept {build}: a running Grafium still uses it")
                continue
            shutil.rmtree(build)
            removed += 1
    backups = 0
    if BACKUPS.is_dir():
        for backup in sorted(BACKUPS.glob("before-*")):
            if backup.is_dir() and not backup.is_symlink():
                shutil.rmtree(backup)
                backups += 1
    print(f"removed {removed} older build(s) and {backups} launcher backup(s)")


def install(source: Path, expected: str, version: str) -> None:
    if not source.is_dir() or source.is_symlink():
        raise SystemExit(f"Not a build directory: {source}")
    entries = sorted(source.iterdir())
    for entry in entries:
        if entry.is_symlink() or not entry.is_file():
            raise SystemExit(f"Unexpected source entry: {entry}")
    if not (source / "grafium-bin").is_file():
        raise SystemExit(f"No grafium-bin in {source}")
    if digest(source / "grafium-bin") != expected:
        raise SystemExit("Source build identity changed; installation not modified.")

    for directory in (ROOT, BACKUPS, LAUNCHER.parent):
        if directory.is_symlink():
            raise SystemExit(f"Refusing symlink directory: {directory}")
        directory.mkdir(mode=0o755, parents=True, exist_ok=True)

    stage = Path(tempfile.mkdtemp(prefix=f"build-{version}-", dir=ROOT))
    for entry in entries:
        destination = stage / entry.name
        shutil.copyfile(entry, destination)
        destination.chmod(0o755)
        if digest(entry) != digest(destination):
            shutil.rmtree(stage)
            raise SystemExit(f"Staged file verification failed: {entry.name}")
        sync(destination)
    if digest(stage / "grafium-bin") != expected:
        shutil.rmtree(stage)
        raise SystemExit("Staged executable identity mismatch; installation not modified.")
    stage.chmod(0o755)
    sync(stage)

    # The previous launcher is kept only until the new one is in place.
    if LAUNCHER.exists():
        backup = Path(tempfile.mkdtemp(prefix=f"before-{version}-", dir=BACKUPS))
        saved = backup / "grafium"
        shutil.copy2(LAUNCHER, saved)
        if digest(LAUNCHER) != digest(saved):
            raise SystemExit("Backup verification failed; existing installation unchanged.")
        sync(saved)

    fd, name = tempfile.mkstemp(prefix=".grafium-update-", dir=LAUNCHER.parent)
    with os.fdopen(fd, "w") as launcher:
        launcher.write(f'#!/bin/sh\nexport LD_LIBRARY_PATH="{stage}"\nexec "{stage}/grafium-bin" "$@"\n')
        launcher.flush()
        os.fsync(launcher.fileno())
    os.chmod(name, 0o755)
    os.replace(name, LAUNCHER)
    sync(LAUNCHER.parent)
    print(f"Updated {LAUNCHER} -> {stage}/grafium-bin")

    prune(stage)
    print("Running applications and user data were not modified.")


def main(argv: list[str]) -> None:
    if not PREFIX and os.geteuid() != 0:
        raise SystemExit("Administrator authorization is required (run with pkexec).")
    if argv == ["--prune"]:
        current = current_build()
        if current is None or not current.is_dir():
            raise SystemExit(f"Cannot tell which build {LAUNCHER} runs; nothing removed.")
        prune(current)
        return
    if len(argv) not in (2, 3) or not re.fullmatch(r"[0-9a-f]{64}", argv[1]):
        raise SystemExit(__doc__)
    version = argv[2] if len(argv) == 3 else "build"
    if not re.fullmatch(r"[0-9A-Za-z._-]+", version):
        raise SystemExit("VERSION may only contain letters, digits, '.', '_' and '-'.")
    install(Path(argv[0]), argv[1], version)


if __name__ == "__main__":
    main(sys.argv[1:])
