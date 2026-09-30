import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const app = readFileSync(join(process.cwd(), "src/App.svelte"), "utf8");

describe("local media import", () => {
  it("offers a native audio/video file picker while retaining typed paths", () => {
    expect(app).toContain("async function browseImportMedia()");
    expect(app).toContain('title: "Choose Video or Audio File"');
    expect(app).toContain('"webm", "mp4", "mkv"');
    expect(app).toContain("importMediaUrl = selected");
    expect(app).toContain('class="dialog-file-picker"');
    expect(app).toContain('aria-label="Choose local media file"');
    expect(app).toContain("URL or local file");
  });

  it("uses a wider responsive dialog and a compact themed icon button", () => {
    expect(app).toContain("dialog import-media-dialog");
    expect(app).toContain("width: min(560px, calc(100vw - 32px));");
    expect(app).toContain("width: 42px;");
    expect(app).toContain("background: var(--btn-bg);");
    expect(app).toContain("color: var(--text-primary);");
    expect(app).toContain('viewBox="0 0 16 16"');
  });
});
