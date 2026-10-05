#!/usr/bin/env python3
"""System installation into a disposable prefix (no root needed)."""
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/install-system.py"


def sha256(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


class InstallSystemTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="grafium-install-system-test-")
        self.home = Path(self.temp.name)
        self.prefix = self.home / "system"
        self.env = {**os.environ, "GRAFIUM_SYSTEM_PREFIX": str(self.prefix)}

    def tearDown(self):
        self.temp.cleanup()

    def build(self, name, executable=None):
        build = self.home / name
        build.mkdir()
        if executable:
            shutil.copyfile(executable, build / "grafium-bin")
        else:
            (build / "grafium-bin").write_text(f"#!/bin/sh\necho Grafium {name}\n")
        (build / "grafium-bin").chmod(0o755)
        (build / "libggml.so.0").write_bytes(name.encode() * 64)
        return build

    def run_script(self, *args, succeeds=True):
        result = subprocess.run([sys.executable, str(SCRIPT), *map(str, args)], env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, succeeds, result.stdout + result.stderr)
        return result

    def install(self, build, version="1.0.0", **kwargs):
        return self.run_script(build, sha256(build / "grafium-bin"), version, **kwargs)

    def builds(self):
        return sorted((self.prefix / "opt/grafium").glob("build-*"))

    def launcher(self):
        return (self.prefix / "usr/bin/grafium").read_text()

    def test_installs_a_verified_copy_and_keeps_nothing_older(self):
        self.install(self.build("first"), "1.0.0")
        result = self.install(self.build("second"), "1.0.1")
        [installed] = self.builds()
        self.assertTrue(installed.name.startswith("build-1.0.1-"))
        self.assertIn(f'exec "{installed}/grafium-bin" "$@"', self.launcher())
        self.assertEqual((installed / "libggml.so.0").read_bytes(), b"second" * 64)
        self.assertIn("removed 1 older build(s) and 1 launcher backup(s)", result.stdout)
        self.assertEqual(list((self.prefix / "var/backups/grafium").iterdir()), [])

    def test_a_changed_or_odd_source_is_refused_and_nothing_changes(self):
        self.install(self.build("first"))
        before = self.launcher()
        changed = self.build("changed")
        expected = sha256(changed / "grafium-bin")
        (changed / "grafium-bin").write_text("#!/bin/sh\necho tampered\n")
        result = self.run_script(changed, expected, succeeds=False)
        self.assertIn("identity changed", result.stderr)
        odd = self.build("odd")
        (odd / "link.so").symlink_to("libggml.so.0")
        self.install(odd, succeeds=False)
        self.assertEqual(self.launcher(), before)
        self.assertEqual(len(self.builds()), 1)

    def test_a_build_still_running_is_kept_until_a_later_prune(self):
        sleeper = shutil.which("sleep")
        self.install(self.build("running", executable=sleeper))
        [running] = self.builds()
        process = subprocess.Popen([str(running / "grafium-bin"), "60"])
        try:
            for _ in range(200):
                if os.readlink(f"/proc/{process.pid}/exe").startswith(str(running)):
                    break
                time.sleep(0.05)
            result = self.install(self.build("next"), "1.0.1")
            self.assertIn("a running Grafium still uses it", result.stdout)
            self.assertEqual(len(self.builds()), 2)
        finally:
            process.kill()
            process.wait()
        self.run_script("--prune")
        [left] = self.builds()
        self.assertTrue(left.name.startswith("build-1.0.1-"))
        self.assertIn(str(left), self.launcher())

    def test_prune_refuses_when_it_cannot_tell_the_current_build(self):
        self.install(self.build("first"))
        (self.prefix / "usr/bin/grafium").write_text("#!/bin/sh\nexec /somewhere/else \"$@\"\n")
        result = self.run_script("--prune", succeeds=False)
        self.assertIn("nothing removed", result.stderr)
        self.assertEqual(len(self.builds()), 1)


if __name__ == "__main__":
    unittest.main()
