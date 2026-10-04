#!/usr/bin/env python3
"""Remove superseded Cargo build artifacts, keeping what later builds reuse.

Cargo never deletes anything from `target/`. Every version bump, dependency
update, toolchain upgrade, or profile change compiles new copies of the
affected crates under new hashed names, and the old copies stay: a few days of
Grafium work left 20-85 GB in each checkout.

For each kind of build that has been run (every package target, test harness,
and feature set, per profile and platform) this keeps the newest artifacts and
everything they depend on, so the next build of any of them is as fast as
before. It removes:

* superseded copies of workspace crates, such as the previous version's build
  after every version bump, together with their incremental caches;
* dependency builds that nothing kept still uses, once they have gone unused
  for --grace-hours (which protects a build that failed part-way);
* other variants of a target (for example one compiled with different
  dependency features) not used within --variant-hours of its newest build;
* everything not built or used for --idle-days.

A target directory with a Cargo build in progress is skipped. Cargo's own
build locks are held while pruning, so a build started meanwhile waits.

    scripts/prune-build-cache.py                  # this checkout
    scripts/prune-build-cache.py --all-worktrees  # every worktree of the repo
    scripts/prune-build-cache.py --dry-run -v     # only report
"""
from __future__ import annotations

import argparse
from collections import defaultdict
from dataclasses import dataclass, field
import fcntl
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import time

UNIT_DIR = re.compile(r"(?P<package>.+)-(?P<hash>[0-9a-f]{16})")
ARTIFACT = re.compile(r"-(?P<hash>[0-9a-f]{16})(?:\.|$)")
FINGERPRINT = re.compile(r"[0-9a-f]{16}")
TARGET_KINDS = ("lib-", "bin-", "integration-test-", "example-", "bench-", "build-script-")
LOCK_NAMES = (".cargo-lock", ".cargo-build-lock", ".cargo-artifact-lock")
# rustc saves an incremental session before linking, and Cargo records the
# unit only after linking, which can take a while for large test binaries.
SESSION_BEFORE_UNIT = 180.0
SESSION_AFTER_UNIT = 5.0
# A session this close to a unit's record is that unit's own.
OWN_SESSION = 2.0
HOUR = 3600.0


@dataclass(eq=False)
class Unit:
    """One compiled (or executed build-script) unit in `.fingerprint/`."""

    profile: Path
    name: str
    hash: str
    package: str
    stem: str
    fingerprint: str
    data: dict
    built: float
    used: float
    local: bool
    keep: bool = False

    @property
    def build_script(self) -> bool:
        return self.stem.startswith(("build-script-", "run-build-script-"))

    @property
    def crate(self) -> str | None:
        """Crate name that names this unit's incremental cache, if it has one."""
        if self.stem.startswith(("run-", "doc-")):
            return None
        stem = self.stem.removeprefix("test-")
        for kind in TARGET_KINDS:
            if stem.startswith(kind):
                return stem[len(kind):].replace("-", "_")
        return None

    def dependencies(self):
        for _package, name, _public, value in self.data["deps"]:
            yield name, value.to_bytes(8, "little").hex()


@dataclass
class Report:
    target: Path
    removed_units: int = 0
    removed_caches: int = 0
    kept_units: int = 0
    freed: int = 0
    skipped: str | None = None
    removed: list[str] = field(default_factory=list)


def read(path: Path) -> str:
    """Read without updating the access time that records a build as used."""
    try:
        descriptor = os.open(path, os.O_RDONLY | getattr(os, "O_NOATIME", 0))
    except PermissionError:
        descriptor = os.open(path, os.O_RDONLY)
    with open(descriptor, encoding="utf-8") as file:
        return file.read()


def valid_dependency(dependency) -> bool:
    return (isinstance(dependency, list) and len(dependency) == 4
            and isinstance(dependency[1], str) and type(dependency[3]) is int
            and 0 <= dependency[3] < 1 << 64)


def read_unit(profile: Path, name: str, package: str, unit_hash: str,
              local: set[str]) -> Unit | None:
    directory = profile / ".fingerprint" / name
    stems = [path.name[:-5] for path in directory.iterdir() if path.name.endswith(".json")]
    if len(stems) != 1:
        return None
    stem = stems[0]
    record = directory / stem
    status = record.stat()
    fingerprint = read(record).strip()
    data = json.loads(read(directory / f"{stem}.json"))
    if not FINGERPRINT.fullmatch(fingerprint) or not isinstance(data, dict):
        return None
    if not isinstance(data.get("deps"), list) or not all(map(valid_dependency, data["deps"])):
        return None
    return Unit(profile, name, unit_hash, package, stem, fingerprint, data,
                built=status.st_mtime, used=max(status.st_mtime, status.st_atime),
                local=package in local)


def load_units(profiles: list[Path], local: set[str]) -> tuple[list[Unit], int]:
    units, unreadable = [], 0
    for profile in profiles:
        for entry in os.scandir(profile / ".fingerprint"):
            match = UNIT_DIR.fullmatch(entry.name)
            if not match or not entry.is_dir(follow_symlinks=False):
                continue
            try:
                unit = read_unit(profile, entry.name, match["package"], match["hash"], local)
            except (OSError, UnicodeDecodeError, ValueError):
                unit = None
            if unit is None:
                unreadable += 1
            else:
                units.append(unit)
    return units, unreadable


def signature(unit: Unit, index: dict[str, list[Unit]]) -> tuple:
    """What distinguishes a genuine variant from a superseded copy.

    Workspace dependencies are left out because a version bump changes all of
    them; any other difference means the unit was built for another command.
    """
    external = []
    for name, fingerprint in unit.dependencies():
        found = index.get(fingerprint)
        if found and all(dependency.local for dependency in found):
            continue
        external.append((name, fingerprint))
    data = unit.data
    return (data.get("rustc"), data.get("profile"), tuple(data.get("rustflags") or ()),
            tuple(sorted(external)))


def select_roots(units: list[Unit], index: dict[str, list[Unit]], now: float,
                 options: argparse.Namespace) -> list[Unit]:
    groups = defaultdict(list)
    for unit in units:
        if unit.local and not unit.build_script:
            key = (unit.profile, unit.package, unit.stem,
                   unit.data.get("features"), unit.data.get("compile_kind"))
            groups[key].append(unit)
    roots = []
    for group in groups.values():
        group.sort(key=lambda unit: (unit.used, unit.built), reverse=True)
        newest = group[0]
        if now - newest.used > options.idle_days * 24 * HOUR:
            continue
        roots.append(newest)
        seen = {signature(newest, index)}
        for unit in group[1:]:
            if newest.used - unit.used > options.variant_hours * HOUR:
                break
            variant = signature(unit, index)
            if variant not in seen:
                seen.add(variant)
                roots.append(unit)
    return roots


def mark(roots: list[Unit], index: dict[str, list[Unit]]) -> None:
    pending = list(roots)
    while pending:
        unit = pending.pop()
        if unit.keep:
            continue
        unit.keep = True
        for _name, fingerprint in unit.dependencies():
            pending.extend(found for found in index.get(fingerprint, ()) if not found.keep)


def stale_caches(profile: Path, units: list[Unit]) -> list[Path]:
    """Incremental caches that no kept workspace unit was compiled with.

    A cache's name cannot be derived from its unit, but each compilation saves
    a session in its cache just before Cargo records the unit. Kept units are
    therefore paired with the caches whose sessions are closest in time, one
    cache per unit unless the unit's own session is in a cache another variant
    also uses. A unit with no session nearby keeps the newest unpaired cache of
    its crate instead.
    """
    root = profile / "incremental"
    if not root.is_dir():
        return []
    builds = defaultdict(list)
    for unit in units:
        if unit.keep and unit.local and unit.profile == profile and unit.crate:
            builds[unit.crate].append(unit.built)
    caches = defaultdict(dict)
    for entry in os.scandir(root):
        crate, dash, _ = entry.name.rpartition("-")
        if not dash or not entry.is_dir(follow_symlinks=False):
            continue
        sessions = [session.stat(follow_symlinks=False).st_mtime
                    for session in os.scandir(entry.path)
                    if session.name.startswith("s-") and session.is_dir(follow_symlinks=False)]
        caches[crate][Path(entry.path)] = (sessions, entry.stat(follow_symlinks=False).st_mtime)
    stale = []
    for crate, entries in caches.items():
        built = builds.get(crate, [])
        pairs = []
        for index, recorded in enumerate(built):
            for path, (sessions, _) in entries.items():
                distances = [abs(session - recorded) for session in sessions
                             if recorded - SESSION_BEFORE_UNIT <= session <= recorded + SESSION_AFTER_UNIT]
                if distances:
                    pairs.append((min(distances), index, path))
        paired, kept = set(), set()
        for distance, index, path in sorted(pairs):
            if index in paired or (path in kept and distance > OWN_SESSION):
                continue
            paired.add(index)
            kept.add(path)
        spare = sorted((path for path in entries if path not in kept),
                       key=lambda path: entries[path][1], reverse=True)
        kept.update(spare[:len(built) - len(paired)])
        stale += [path for path in entries if path not in kept]
    return stale


def artifacts(profile: Path) -> dict[str, list[Path]]:
    found = defaultdict(list)
    for name in ("deps", "examples"):
        directory = profile / name
        if directory.is_dir():
            for entry in os.scandir(directory):
                match = ARTIFACT.search(entry.name)
                if match:
                    found[match["hash"]].append(Path(entry.path))
    return found


def disk_usage(path: Path) -> int:
    """Bytes that deleting `path` frees; extra hard links keep a file's data."""
    try:
        status = path.lstat()
    except FileNotFoundError:
        return 0
    if not stat.S_ISDIR(status.st_mode):
        return status.st_blocks * 512 if status.st_nlink == 1 else 0
    total = status.st_blocks * 512
    for directory, names, files in os.walk(path):
        for name in names + files:
            status = os.lstat(os.path.join(directory, name))
            if stat.S_ISDIR(status.st_mode) or status.st_nlink == 1:
                total += status.st_blocks * 512
    return total


def remove(path: Path) -> None:
    try:
        if path.is_dir() and not path.is_symlink():
            shutil.rmtree(path)
        else:
            path.unlink()
    except FileNotFoundError:
        pass


def lock(profiles: list[Path]) -> list[int] | None:
    """Hold Cargo's build locks, or return None if a build holds one."""
    held = []
    for profile in profiles:
        for name in LOCK_NAMES:
            path = profile / name
            if not path.is_file():
                continue
            try:
                descriptor = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
            except OSError:
                descriptor = None
            else:
                try:
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    held.append(descriptor)
                    continue
                except OSError:
                    os.close(descriptor)
            for other in held:
                os.close(other)
            return None
    return held


def prune(target: Path, local: set[str], options: argparse.Namespace, now: float) -> Report:
    report = Report(target)
    if not (target / "CACHEDIR.TAG").is_file():
        report.skipped = "not a Cargo target directory"
        return report
    profiles = sorted({path.parent for pattern in ("*/.fingerprint", "*/*/.fingerprint")
                       for path in target.glob(pattern)
                       if path.is_dir() and not path.is_symlink() and not path.parent.is_symlink()})
    held = lock(profiles)
    if held is None:
        report.skipped = "a Cargo build is running"
        return report
    try:
        units, unreadable = load_units(profiles, local)
        if unreadable > max(20, len(units) // 10):
            report.skipped = (f"{unreadable} unrecognized build records; "
                              "Cargo's format may have changed")
            return report
        index = defaultdict(list)
        for unit in units:
            index[unit.fingerprint].append(unit)
        mark(select_roots(units, index, now, options), index)
        # Only superseded workspace targets go at once; build scripts and
        # dependencies get a grace period, along with what they depend on.
        recent = [unit for unit in units if not unit.keep
                  and (unit.build_script or not unit.local)
                  and now - unit.used <= options.grace_hours * HOUR]
        mark(recent, index)

        garbage = [unit for unit in units if not unit.keep]
        caches = [cache for profile in profiles for cache in stale_caches(profile, units)]
        outputs = {profile: artifacts(profile) for profile in profiles}
        for unit in garbage:
            # The record goes last, so an interrupted prune is finished next time.
            paths = [*outputs[unit.profile].get(unit.hash, ()),
                     unit.profile / "build" / unit.name,
                     unit.profile / ".fingerprint" / unit.name]
            report.freed += sum(map(disk_usage, paths))
            report.removed.append(f"{unit.profile.relative_to(target)}/{unit.name} ({unit.stem})")
            if not options.dry_run:
                for path in paths:
                    remove(path)
        for cache in caches:
            report.freed += disk_usage(cache)
            report.removed.append(f"{cache.relative_to(target)} (incremental cache)")
            if not options.dry_run:
                remove(cache)
        report.removed_units, report.removed_caches = len(garbage), len(caches)
        report.kept_units = len(units) - len(garbage) + unreadable
    finally:
        for descriptor in held:
            os.close(descriptor)
    return report


def worktrees(checkout: Path) -> list[Path]:
    listing = subprocess.run(["git", "-C", str(checkout), "worktree", "list", "--porcelain"],
                             check=True, capture_output=True, text=True).stdout
    return [Path(line.removeprefix("worktree ")) for line in listing.splitlines()
            if line.startswith("worktree ")]


def workspace(checkout: Path) -> tuple[Path, set[str]] | None:
    """The checkout's target directory and workspace package names."""
    result = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--offline", "--format-version", "1",
         "--manifest-path", str(checkout / "Cargo.toml")],
        capture_output=True, text=True)
    if result.returncode != 0:
        return None
    metadata = json.loads(result.stdout)
    return (Path(metadata["target_directory"]),
            {package["name"] for package in metadata["packages"]})


def size(count: int) -> str:
    return f"{count / 2**30:.1f} GiB" if count >= 2**30 else f"{count / 2**20:.0f} MiB"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__.split("\n\n")[0],
        epilog="See the module documentation at the top of this script for details.")
    parser.add_argument("checkouts", nargs="*", type=Path,
                        help="checkouts to prune (default: the one containing this script)")
    parser.add_argument("--all-worktrees", action="store_true",
                        help="prune every worktree of the checkouts' repositories")
    parser.add_argument("-n", "--dry-run", action="store_true", help="only report what would go")
    parser.add_argument("-v", "--verbose", action="store_true", help="list every removal")
    parser.add_argument("--variant-hours", type=float, default=48,
                        help="keep a target's other variants used this close to its newest build")
    parser.add_argument("--grace-hours", type=float, default=24,
                        help="keep unused dependency builds this long after their last use")
    parser.add_argument("--idle-days", type=float, default=7,
                        help="remove builds of targets not built or used for this long")
    options = parser.parse_args(argv)

    checkouts = options.checkouts or [Path(__file__).resolve().parents[1]]
    if options.all_worktrees:
        try:
            checkouts = [tree for checkout in checkouts for tree in worktrees(checkout)]
        except subprocess.CalledProcessError as error:
            print(f"cannot list worktrees: {error.stderr.strip()}", file=sys.stderr)
            return 1
    status = 0
    targets: dict[Path, set[str]] = {}
    for checkout in dict.fromkeys(path.resolve() for path in checkouts):
        found = workspace(checkout)
        if found is None:
            print(f"build cache {checkout}: skipped (cargo metadata failed)", file=sys.stderr)
            status = 1
            continue
        target, packages = found
        targets.setdefault(target, set()).update(packages)

    verb = "would remove" if options.dry_run else "removed"
    now = time.time()
    for target, packages in targets.items():
        if not target.is_dir():
            continue
        report = prune(target, packages, options, now)
        if report.skipped:
            print(f"build cache {target}: skipped ({report.skipped})")
            continue
        if options.verbose:
            for line in report.removed:
                print(f"  {verb} {line}")
        print(f"build cache {target}: {verb} {report.removed_units} stale builds and "
              f"{report.removed_caches} incremental caches ({size(report.freed)}); "
              f"kept {report.kept_units} builds")
    return status


if __name__ == "__main__":
    sys.exit(main())
