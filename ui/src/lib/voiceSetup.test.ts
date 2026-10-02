// @vitest-environment node
import { describe, expect, it } from "vitest";
import { execFileSync } from "node:child_process";
import { mkdtempSync, readFileSync, writeFileSync, rmSync, mkdirSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { voicePackageCommand } from "./voiceSetup";
import type { PrivateVoiceManifest } from "./privateReaderVoice";

describe("the exact copied offline preparation command", () => {
  it.each([false, true])("creates a complete bounded manifest without altering files (Android=%s)", android => {
    const folder = mkdtempSync(join(tmpdir(), "grafium-voice-help-"));
    try {
      writeFileSync(join(folder, "en_US-ljspeech-high.onnx"), "Synthetic fixture; not an inference model");
      writeFileSync(join(folder, "MODEL_CARD"), "Synthetic test notice");
      if (android) {
        writeFileSync(join(folder, "tokens.txt"), "a 1\n");
        mkdirSync(join(folder, "espeak-ng-data"));
        writeFileSync(join(folder, "espeak-ng-data", "phontab"), "Synthetic phonemes");
      } else {
        writeFileSync(join(folder, "en_US-ljspeech-high.onnx.json"),
          JSON.stringify({ audio: { sample_rate: 22050 }, language: { code: "en_US" } }));
      }
      const output = execFileSync("bash", ["-c", voicePackageCommand(android)], { cwd: folder, encoding: "utf8" });
      expect(output).toContain("Originals were not changed");
      const manifest: PrivateVoiceManifest = JSON.parse(readFileSync(join(folder, "manifest.json"), "utf8"));
      expect(manifest.runtime).toBe(android ? "sherpa-vits-v1" : "piper-onnx-v1");
      expect(manifest.sample_rate).toBe(22050);
      expect(manifest.language).toBe("en-US");
      expect(manifest.artifacts.map(item => item.role)).toEqual(android
        ? ["model", "license", "tokens", "espeak"] : ["model", "license", "config"]);
      expect(manifest.artifacts.every(item => item.bytes > 0 && /^[a-f0-9]{64}$/.test(item.sha256) && item.url === null)).toBe(true);
      expect(readFileSync(join(folder, "MODEL_CARD"), "utf8")).toBe("Synthetic test notice");
    } finally { rmSync(folder, { recursive: true, force: true }); }
  });
});
