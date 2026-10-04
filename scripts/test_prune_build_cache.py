#!/usr/bin/env python3
"""Exercise build-cache pruning against real Cargo builds of a tiny workspace."""
import fcntl
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/prune-build-cache.py"
DAY = 24 * 3600
# The workspace build unifies `extra` into the dependency; testing the engine
# alone does not, so its test harness exists in two variants. Nothing depends
# on a test harness, so only variant tracking can keep both.
COMMANDS = (("build",), ("test", "--no-run"), ("test", "--no-run", "-p", "engine"))
COMPILING = re.compile(r"^\s+Compiling (\S+)", re.MULTILINE)


class PruneBuildCacheTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="grafium-prune-test-")
        self.root = Path(self.temp.name)
        self.workspace = self.root / "workspace"
        self.target = self.workspace / "target"
        self.env = {key: value for key, value in os.environ.items()
                    if key not in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET_DIR",
                                   "CARGO_INCREMENTAL", "RUSTFLAGS")}
        # A package outside the workspace stands in for a crates.io dependency.
        self.write_dependency("0.1.0")
        self.write("dependency/build.rs", 'fn main() { println!("cargo:rerun-if-changed=build.rs"); }\n')
        self.write("dependency/src/lib.rs",
                   'pub fn value() -> i32 { if cfg!(feature = "extra") { 2 } else { 1 } }\n')
        self.write("workspace/Cargo.toml",
                   '[workspace]\nmembers = ["engine", "app"]\nresolver = "2"\n\n'
                   '[workspace.package]\nversion = "0.1.0"\nedition = "2021"\n')
        self.write("workspace/engine/Cargo.toml",
                   '[package]\nname = "engine"\nversion.workspace = true\nedition.workspace = true\n\n'
                   '[dependencies]\ndependency = { path = "../../dependency" }\n')
        self.write("workspace/engine/build.rs", 'fn main() { println!("cargo:rerun-if-changed=build.rs"); }\n')
        self.write("workspace/engine/src/lib.rs",
                   "pub fn value() -> i32 { dependency::value() }\n\n"
                   "#[cfg(test)]\nmod tests {\n    #[test]\n    fn works() { assert!(super::value() > 0); }\n}\n")
        self.write("workspace/app/Cargo.toml",
                   '[package]\nname = "app"\nversion.workspace = true\nedition.workspace = true\n\n'
                   '[dependencies]\nengine = { path = "../engine" }\n'
                   'dependency = { path = "../../dependency", features = ["extra"] }\n')
        self.write("workspace/app/src/main.rs", 'fn main() { println!("{}", engine::value()); }\n')

    def tearDown(self):
        self.temp.cleanup()

    def write(self, name, text):
        path = self.root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)

    def write_dependency(self, version):
        self.write("dependency/Cargo.toml",
                   f'[package]\nname = "dependency"\nversion = "{version}"\nedition = "2021"\n\n'
                   '[features]\nextra = []\n')

    def bump(self, version):
        manifest = self.workspace / "Cargo.toml"
        manifest.write_text(re.sub(r'version = "[^"]+"', f'version = "{version}"',
                                   manifest.read_text()))

    def cargo(self, *args, succeeds=True):
        result = subprocess.run(["cargo", *args, "--offline"], cwd=self.workspace, env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, succeeds, result.stderr)
        return COMPILING.findall(result.stderr)

    def build_everything(self):
        return [crate for command in COMMANDS for crate in self.cargo(*command)]

    def age(self, seconds):
        """Make everything so far, sources included, look `seconds` older."""
        for directory, names, files in os.walk(self.root):
            for name in names + files:
                path = os.path.join(directory, name)
                status = os.lstat(path)
                os.utime(path, (status.st_atime - seconds, status.st_mtime - seconds),
                         follow_symlinks=False)

    def prune(self, *args):
        result = subprocess.run([sys.executable, str(SCRIPT), *args, str(self.workspace)],
                                env=self.env, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        return result.stdout

    def units(self):
        return {path.name for path in self.target.glob("*/.fingerprint/*")}

    def caches(self):
        return {path.name for path in self.target.glob("*/incremental/*")}

    def test_version_bump_retires_the_previous_build_and_current_builds_stay_fresh(self):
        self.build_everything()
        self.age(3 * DAY)
        before_units, before_caches = self.units(), self.caches()
        self.assertTrue(before_caches)
        self.bump("0.1.1")
        self.assertIn("engine", self.build_everything())

        self.assertIn("removed", self.prune())
        after = self.units()
        dependency = {unit for unit in before_units if unit.startswith("dependency-")}
        self.assertTrue(dependency, before_units)
        self.assertLessEqual(dependency, after, "dependency builds still in use were removed")
        self.assertFalse((before_units - dependency) & after, "previous version's builds survived")
        self.assertFalse(before_caches & self.caches(), "previous version's caches survived")
        self.assertTrue(any(cache.startswith("engine-") for cache in self.caches()))
        self.assertEqual(self.build_everything(), [])

    def test_caches_of_a_build_superseded_moments_ago_are_removed(self):
        self.build_everything()
        before = {cache for cache in self.caches() if not cache.startswith("build_script_build-")}
        self.assertTrue(before)
        self.bump("0.1.1")
        self.build_everything()

        self.prune()
        self.assertFalse(before & self.caches(), "previous version's caches survived")
        self.assertEqual(self.build_everything(), [])

    def test_dependencies_of_a_failed_build_survive_until_it_is_fixed(self):
        self.build_everything()
        self.age(3 * DAY)
        self.write_dependency("0.2.0")
        source = self.workspace / "engine/src/lib.rs"
        working = source.read_text()
        source.write_text(working + 'compile_error!("not yet");\n')
        self.assertIn("dependency", self.cargo("build", succeeds=False))

        self.prune()
        source.write_text(working)
        self.assertNotIn("dependency", self.cargo("build"))

    def test_idle_builds_are_removed_and_unrelated_files_are_not(self):
        self.build_everything()
        notes = self.target / "notes.log"
        notes.write_text("kept\n")
        self.age(8 * DAY)

        self.prune()
        self.assertEqual(self.units(), set())
        self.assertEqual(self.caches(), set())
        self.assertEqual(notes.read_text(), "kept\n")

    def test_a_target_with_a_running_build_is_left_alone(self):
        self.build_everything()
        self.age(8 * DAY)
        units = self.units()
        with open(self.target / "debug/.cargo-lock") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            self.assertIn("skipped (a Cargo build is running)", self.prune())
        self.assertEqual(self.units(), units)

    def test_dry_run_reports_without_removing(self):
        self.build_everything()
        self.age(8 * DAY)
        units, caches = self.units(), self.caches()
        self.assertIn("would remove", self.prune("--dry-run"))
        self.assertEqual((self.units(), self.caches()), (units, caches))


if __name__ == "__main__":
    unittest.main()
