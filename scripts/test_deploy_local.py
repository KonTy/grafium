#!/usr/bin/env python3
"""Exercise deployment with tiny ELF fixtures and a disposable HOME."""
import os
from pathlib import Path
import subprocess
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPT = ROOT / "scripts/deploy-local.sh"


class DeploymentTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="grafium-deploy-test-")
        self.home = Path(self.temp.name)
        self.env = {
            **os.environ, "HOME": str(self.home),
            "XDG_DATA_HOME": str(self.home / ".local/share"),
            "XDG_CONFIG_HOME": str(self.home / ".config"),
            "XDG_CACHE_HOME": str(self.home / ".cache"),
        }
        self.sha = subprocess.check_output(
            ["git", "rev-parse", "--short=7", "HEAD"], cwd=ROOT, text=True
        ).strip()

    def tearDown(self):
        self.temp.cleanup()

    def build(self, name):
        build = self.home / name
        build.mkdir()
        (build / "library.c").write_text("int deployment_fixture(void) { return 0; }\n")
        subprocess.run([
            "cc", "-shared", "-fPIC", "-Wl,-soname,libggml.so.0",
            str(build / "library.c"), "-o", str(build / "libggml.so.0"),
        ], check=True, capture_output=True)
        (build / "libggml.so").symlink_to("libggml.so.0")
        (build / "main.c").write_text(
            '#include <stdio.h>\n#include <string.h>\n#include <unistd.h>\n'
            'extern int deployment_fixture(void);\n'
            'int main(int argc, char **argv) {\n'
            '  if (argc > 1 && strcmp(argv[1], "--hold") == 0) { sleep(60); return 0; }\n'
            f'  puts("Grafium {name} (commit {self.sha})"); return deployment_fixture();\n}}\n'
        )
        subprocess.run([
            "cc", str(build / "main.c"), "-L", str(build), "-lggml",
            "-o", str(build / "grafium"),
        ], check=True, capture_output=True)
        return build

    def deploy(self, build, succeeds=True):
        result = subprocess.run([str(SCRIPT), str(build)], cwd=ROOT, env=self.env,
                                capture_output=True, text=True)
        self.assertEqual(result.returncode == 0, succeeds, result.stdout + result.stderr)
        return result

    def installed(self):
        return subprocess.check_output([str(self.home / ".local/bin/grafium"), "--version"],
                                       env=self.env, text=True).strip()

    def builds(self):
        return sorted((self.home / ".local/lib/grafium").glob("build.*"))

    def build_name(self, build):
        return subprocess.check_output([str(build / "grafium-bin"), "--version"],
                                       env={**self.env, "LD_LIBRARY_PATH": str(build)},
                                       text=True).split()[1]

    def test_only_the_newest_builds_and_backups_that_restore_them_are_kept(self):
        self.env["GRAFIUM_KEEP_BUILDS"] = "2"
        for name in ("first", "second", "third", "fourth"):
            result = self.deploy(self.build(name))
        self.assertIn("fourth", self.installed())
        self.assertIn("removed 1 older build(s); keeping the newest 2", result.stdout)
        self.assertEqual({self.build_name(build) for build in self.builds()}, {"third", "fourth"})
        launchers = [path.read_text() for path in
                     (self.home / ".local/lib/grafium").glob("backup.*/grafium")]
        self.assertEqual(len(launchers), 1)
        self.assertIn(str(next(b for b in self.builds() if self.build_name(b) == "third")),
                      launchers[0])

    def test_a_build_that_is_still_running_is_never_removed(self):
        self.env["GRAFIUM_KEEP_BUILDS"] = "1"
        self.deploy(self.build("running"))
        [running_build] = self.builds()
        process = subprocess.Popen([str(self.home / ".local/bin/grafium"), "--hold"], env=self.env)
        try:
            for _ in range(200):
                if os.readlink(f"/proc/{process.pid}/exe").startswith(str(running_build)):
                    break
                time.sleep(0.05)
            else:
                self.fail("the launcher never started the installed build")
            self.deploy(self.build("next"))
            self.deploy(self.build("latest"))
            self.assertEqual({self.build_name(build) for build in self.builds()},
                             {"running", "latest"})
        finally:
            process.kill()
            process.wait()
        self.deploy(self.build("after-exit"))
        self.assertEqual([self.build_name(build) for build in self.builds()], ["after-exit"])

    def test_an_invalid_keep_count_keeps_three_builds(self):
        self.env["GRAFIUM_KEEP_BUILDS"] = "none"
        for name in ("one", "two", "three", "four"):
            result = self.deploy(self.build(name))
        self.assertIn("GRAFIUM_KEEP_BUILDS must be a positive integer", result.stderr)
        self.assertEqual({self.build_name(build) for build in self.builds()},
                         {"two", "three", "four"})

    def test_generations_remain_immutable_and_backups_are_verified(self):
        legacy = self.home / ".local/lib/libggml.so.0"
        legacy.parent.mkdir(parents=True)
        legacy.write_bytes(b"legacy library must remain untouched")
        self.deploy(self.build("first"))
        old_launcher = (self.home / ".local/bin/grafium").read_bytes()
        old_files = {path: path.read_bytes() for path in
                     (self.home / ".local/lib/grafium").glob("build.*/*")}
        self.deploy(self.build("second"))
        self.assertIn("second", self.installed())
        self.assertEqual(legacy.read_bytes(), b"legacy library must remain untouched")
        self.assertIn("second", subprocess.check_output(
            [str(self.home / ".local/bin/grafium-bin"), "--version"], env=self.env, text=True))
        for path, content in old_files.items():
            self.assertEqual(path.read_bytes(), content)
            self.assertFalse(path.is_symlink())
        backups = list((self.home / ".local/lib/grafium").glob("backup.*/grafium"))
        self.assertTrue(any(path.read_bytes() == old_launcher for path in backups))
        for manifest in (self.home / ".local/lib/grafium").glob("backup.*/SHA256SUMS"):
            subprocess.run(["sha256sum", "--check", "--quiet", manifest.name],
                           cwd=manifest.parent, check=True)

    def test_missing_libraries_leave_current_installation_unchanged(self):
        self.deploy(self.build("first"))
        broken = self.build("broken")
        (broken / "libggml.so").unlink()
        (broken / "libggml.so.0").unlink()
        self.deploy(broken, succeeds=False)
        self.assertIn("first", self.installed())

    def test_desktop_identity_and_all_icon_sizes_are_installed_consistently(self):
        icons = self.home / ".local/share/icons/hicolor"
        stale = icons / "48x48/apps/grafium-local.png"
        stale.parent.mkdir(parents=True)
        stale.write_bytes(b"stale icon")
        apps = self.home / ".local/share/applications"
        apps.mkdir(parents=True)
        old_desktop = b"[Desktop Entry]\nName=Grafium\nIcon=old-icon\nType=Application\n"
        (apps / "grafium.desktop").write_bytes(old_desktop)
        self.deploy(self.build("icons"))
        canonical = (apps / "grafium.desktop").read_text()
        self.assertIn("Icon=grafium-local-icon\n", canonical)
        self.assertIn("StartupWMClass=grafium\n", canonical)
        self.assertIn(f'Exec="{self.home}/.local/bin/grafium"\n', canonical)
        compatibility = (apps / "grafium-bin.desktop").read_text()
        self.assertIn("StartupWMClass=grafium-bin\n", compatibility)
        self.assertIn("NoDisplay=true\n", compatibility)
        for name in ("grafium", "grafium-local", "grafium-bin", "grafium-local-icon"):
            for size in (32, 48, 64, 128, 256):
                source = "128x128@2x.png" if size == 256 else f"{size}x{size}.png"
                self.assertEqual(
                    (icons / f"{size}x{size}/apps/{name}.png").read_bytes(),
                    (ROOT / "ui/src-tauri/icons" / source).read_bytes(),
                )
            if name != "grafium-local-icon":
                self.assertEqual(
                    (icons / f"scalable/apps/{name}.svg").read_bytes(),
                    (ROOT / "ui/src-tauri/icons/grafium-logo.svg").read_bytes(),
                )
            else:
                self.assertFalse((icons / f"scalable/apps/{name}.svg").exists())
        backups = list((self.home / ".local/lib/grafium").glob("backup.*/desktop"))
        self.assertTrue(any((b / "applications/grafium.desktop").read_bytes() == old_desktop
                            for b in backups))
        self.assertTrue(any((b / "icons/hicolor/48x48/apps/grafium-local.png").read_bytes() == b"stale icon"
                            for b in backups))

    def test_optional_cache_failure_is_reported_without_discarding_installed_icons(self):
        tools = self.home / "tools"
        tools.mkdir()
        cache = tools / "gtk-update-icon-cache"
        cache.write_text("#!/bin/sh\nexit 1\n")
        cache.chmod(0o755)
        self.env["PATH"] = str(tools) + os.pathsep + self.env["PATH"]
        result = self.deploy(self.build("cache-warning"))
        self.assertIn("icon cache could not be rebuilt", result.stderr)
        self.assertTrue((self.home / ".local/share/icons/hicolor/48x48/apps/grafium.png").is_file())
        self.assertIn("cache-warning", self.installed())

    def test_smplos_index_and_legacy_pinned_launcher_use_the_local_raster(self):
        tools = self.home / "tools"
        tools.mkdir()
        builder = tools / "rebuild-app-cache"
        builder.write_text(
            '#!/bin/sh\nmkdir -p "$HOME/.cache/smplos"\n'
            'icon=$(sed -n "s/^Icon=//p" "$XDG_DATA_HOME/applications/grafium.desktop")\n'
            'printf "Grafium;grafium;office;%s\\n" "$icon" > "$HOME/.cache/smplos/app_index"\n'
        )
        builder.chmod(0o755)
        self.env["PATH"] = str(tools) + os.pathsep + self.env["PATH"]
        apps = self.home / ".local/share/applications"
        apps.mkdir(parents=True)
        legacy = apps / "Grafium.desktop"
        legacy.write_text(
            f"[Desktop Entry]\nName=Grafium\nExec={self.home}/.local/bin/grafium\n"
            "Icon=grafium-local\nX-Keep-Custom-Field=yes\n"
        )
        cache = self.home / ".cache/smplos/app_index"
        cache.parent.mkdir(parents=True)
        cache.write_text("Grafium;grafium;office;grafium\n")
        pins = self.home / ".config/smplos/pinned-apps.txt"
        pins.parent.mkdir(parents=True)
        original_pins = f"keep-another-app\n{self.home}/.local/bin/grafium\n\"{self.home}/.local/bin/grafium\"\n"
        pins.write_text(original_pins)
        self.deploy(self.build("menu-icon"))
        self.assertIn("Icon=grafium-local-icon\n", legacy.read_text())
        self.assertIn("X-Keep-Custom-Field=yes\n", legacy.read_text())
        self.assertEqual(cache.read_text(), "Grafium;grafium;office;grafium-local-icon\n")
        self.assertEqual(pins.read_text(), f"keep-another-app\n\"{self.home}/.local/bin/grafium\"\n\"{self.home}/.local/bin/grafium\"\n")
        backups = list((self.home / ".local/lib/grafium").glob("backup.*/desktop/smplos-app_index"))
        self.assertTrue(any(path.read_text() == "Grafium;grafium;office;grafium\n" for path in backups))
        pin_backups = list((self.home / ".local/lib/grafium").glob("backup.*/desktop/smplos-pinned-apps.txt"))
        self.assertTrue(any(path.read_text() == original_pins for path in pin_backups))

    def test_smplos_cache_refresh_retries_a_concurrent_writer_failure(self):
        tools = self.home / "tools"
        tools.mkdir()
        builder = tools / "rebuild-app-cache"
        builder.write_text(
            '#!/bin/sh\n'
            'if [ ! -f "$HOME/cache-first-attempt" ]; then\n'
            '  : > "$HOME/cache-first-attempt"\n  exit 1\nfi\n'
            'mkdir -p "$HOME/.cache/smplos"\n'
            'printf "refreshed\\n" > "$HOME/.cache/smplos/app_index"\n'
        )
        builder.chmod(0o755)
        self.env["PATH"] = str(tools) + os.pathsep + self.env["PATH"]
        result = self.deploy(self.build("cache-retry"))
        self.assertNotIn("smplOS app index could not be refreshed", result.stderr)
        self.assertEqual((self.home / ".cache/smplos/app_index").read_text(), "refreshed\n")


if __name__ == "__main__":
    unittest.main()
