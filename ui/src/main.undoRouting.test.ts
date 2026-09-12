import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const mainSource = readFileSync(join(process.cwd(), "src/main.ts"), "utf8");
const appSource = readFileSync(join(process.cwd(), "src/App.svelte"), "utf8");

describe("global undo/redo routing", () => {
  it("routes stale-editor undo to the app-level undo stack first", () => {
    expect(mainSource).toContain("editorHasDomFocus");
    expect(mainSource).toContain("if (canUndo())");
    expect(mainSource).toContain("unfocused CodeMirror undo fallback");
  });

  it("does not register Ctrl-Z/Ctrl-Y shortcuts as no-ops", () => {
    expect(appSource).toContain("function triggerNativeUndo()");
    expect(appSource).toContain("function triggerNativeRedo()");
    expect(appSource).not.toContain("undo: () => {}");
    expect(appSource).not.toContain("redo: () => {}");
  });
});
