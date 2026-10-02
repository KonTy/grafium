import hashlib
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest

spec = importlib.util.spec_from_file_location("voice_helper", Path(__file__).with_name("prepare-reader-voice.py"))
helper = importlib.util.module_from_spec(spec)
spec.loader.exec_module(helper)


class VoicePackageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="voice-helper-", dir=Path(__file__).parent)
        self.root = Path(self.temp.name)
        (self.root / "sample.onnx").write_bytes(b"synthetic-model-not-for-inference")
        (self.root / "MODEL_CARD").write_text("Synthetic test license")
        (self.root / "sample.onnx.json").write_text(json.dumps({
            "audio": {"sample_rate": 22050}, "language": {"code": "en_US"}}))
        self.args = SimpleNamespace(folder=str(self.root), runtime="piper-onnx-v1", id="example",
                                    name="Example", language="en-US", license="Synthetic test license",
                                    license_url="https://example.org/LICENSE", license_file="MODEL_CARD", sample_rate=None)

    def tearDown(self):
        self.temp.cleanup()

    def test_piper_manifest_matches_adjacent_files_without_modifying_them(self):
        before = {p.name: p.read_bytes() for p in self.root.iterdir()}
        result = json.loads(helper.prepare(self.args).read_text())
        self.assertEqual(result["sample_rate"], 22050)
        self.assertEqual(result["runtime"], "piper-onnx-v1")
        self.assertEqual({a["role"] for a in result["artifacts"]}, {"model", "config", "license"})
        for artifact in result["artifacts"]:
            data = before[artifact["path"]]
            self.assertEqual(artifact["bytes"], len(data))
            self.assertEqual(artifact["sha256"], hashlib.sha256(data).hexdigest())
            self.assertIsNone(artifact["url"])
            self.assertEqual((self.root / artifact["path"]).read_bytes(), data)

    def test_sherpa_preserves_nested_phonemizer_paths(self):
        self.args.runtime = "sherpa-vits-v1"
        self.args.sample_rate = 22050
        (self.root / "tokens.txt").write_text("a 1\n")
        (self.root / "espeak-ng-data/lang/gmw").mkdir(parents=True)
        (self.root / "espeak-ng-data/lang/gmw/en").write_text("synthetic English phonemizer")
        (self.root / "espeak-ng-data/phontab").write_bytes(b"synthetic table")
        result = json.loads(helper.prepare(self.args).read_text())
        self.assertEqual({a["role"] for a in result["artifacts"]}, {"model", "tokens", "license", "espeak"})
        self.assertIn("espeak-ng-data/lang/gmw/en", [a["path"] for a in result["artifacts"]])

    def test_existing_manifest_is_never_overwritten(self):
        path = self.root / "manifest.json"
        path.write_text("user-owned manifest")
        with self.assertRaises(FileExistsError):
            helper.prepare(self.args)
        self.assertEqual(path.read_text(), "user-owned manifest")

    def test_rejects_missing_support_file(self):
        (self.root / "sample.onnx.json").unlink()
        with self.assertRaisesRegex(ValueError, "matching"):
            helper.prepare(self.args)
        self.assertFalse((self.root / "manifest.json").exists())

    def test_rejects_wrong_language_and_sample_rate(self):
        self.args.language = "fr-FR"
        with self.assertRaisesRegex(ValueError, "Language"):
            helper.prepare(self.args)
        self.args.language = "en-US"
        self.args.sample_rate = 48000
        with self.assertRaisesRegex(ValueError, "Sample rate"):
            helper.prepare(self.args)

    def test_rejects_links_and_escaping_license_path(self):
        (self.root / "linked-card").symlink_to(self.root / "MODEL_CARD")
        self.args.license_file = "linked-card"
        with self.assertRaisesRegex(ValueError, "symlinks"):
            helper.prepare(self.args)
        self.args.license_file = "../elsewhere"
        with self.assertRaisesRegex(ValueError, "relative"):
            helper.prepare(self.args)

    def test_rejects_invalid_metadata_before_writing(self):
        for key, value in [("id", "unsafe/id"), ("language", "en_US"), ("license", ""),
                           ("license_url", "https://example.org/license?key=secret")]:
            with self.subTest(key=key):
                args = SimpleNamespace(**vars(self.args))
                setattr(args, key, value)
                with self.assertRaises(ValueError):
                    helper.prepare(args)
                self.assertFalse((self.root / "manifest.json").exists())

    def test_sherpa_requires_tokens_and_explicit_rate(self):
        self.args.runtime = "sherpa-vits-v1"
        with self.assertRaisesRegex(ValueError, "actual sample rate"):
            helper.prepare(self.args)
        self.args.sample_rate = 22050
        with self.assertRaisesRegex(ValueError, "tokens.txt"):
            helper.prepare(self.args)


if __name__ == "__main__":
    unittest.main()
