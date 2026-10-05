#!/usr/bin/env python3
"""Clearing a Cargo build cache after a deploy, with disposable directories."""
import fcntl
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/clear-build-cache.sh"


class ClearBuildCacheTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="grafium-clear-cache-test-")
        self.root = Path(self.temp.name)

    def tearDown(self):
        self.temp.cleanup()

    def cache(self, path):
        (path / "release/deps").mkdir(parents=True)
        (path / "CACHEDIR.TAG").write_text("Signature: 8a477f597d28d172789f06886806bc55\n")
        (path / "release/.cargo-lock").touch()
        (path / "release/grafium").write_bytes(b"binary")
        (path / "release/deps/libbig.rlib").write_bytes(b"x" * 1024)
        return path

    def clear(self, target):
        return subprocess.run(["bash", str(SCRIPT), str(target)], capture_output=True, text=True, check=True)

    def test_empties_the_cache_but_keeps_a_symlinked_directory_on_its_drive(self):
        real = self.cache(self.root / "other-drive/cache")
        link = self.root / "checkout/target"
        link.parent.mkdir()
        link.symlink_to(real)
        result = self.clear(link)
        self.assertIn("cleared build cache", result.stdout)
        self.assertTrue(link.is_symlink())
        self.assertTrue(real.is_dir())
        self.assertEqual(list(real.iterdir()), [])

    def test_leaves_a_directory_that_is_not_a_cargo_cache(self):
        plain = self.root / "notes"
        plain.mkdir()
        (plain / "keep.txt").write_text("not a build output")
        result = self.clear(plain)
        self.assertIn("not a Cargo build cache", result.stderr)
        self.assertEqual((plain / "keep.txt").read_text(), "not a build output")

    def test_keeps_a_cache_while_a_build_holds_its_lock(self):
        cache = self.cache(self.root / "target")
        with open(cache / "release/.cargo-lock", "w") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            result = self.clear(cache)
        self.assertIn("its cache was kept", result.stderr)
        self.assertEqual((cache / "release/grafium").read_bytes(), b"binary")


if __name__ == "__main__":
    unittest.main()
