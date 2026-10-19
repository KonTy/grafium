import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const read = (file: string) => readFileSync(join(process.cwd(), "src/components", file), "utf8");

// Both editors create their own CodeMirror view, and neither is mounted in tests,
// so what matters here is that each one wires the spell checker the same way:
// the extension registers its own menu section, and the editor draws no menu.
describe.each(["BlockEditor.svelte", "UnifiedPageEditor.svelte"])("%s spell check wiring", (file) => {
  const source = read(file);

  it("installs the spell checker beside the Markdown language", () => {
    expect(source).toContain('import { spellcheckExtension } from "../lib/spellcheckExtension";');
    expect(source).toMatch(/markdown\(\),\s+spellcheckExtension\(\),/);
  });

  it("leaves the spelling menu to the shared context menu", () => {
    expect(source).not.toMatch(/SpellcheckMenu|SpellcheckRequest|spellMenu|onContextMenu/);
  });
});

describe.each(["BlockEditor.svelte", "UnifiedPageEditor.svelte"])("%s menus", (file) => {
  const source = read(file);

  it("draws no menu of its own and opens none from a contextmenu event", () => {
    expect(source).not.toMatch(/image-size-menu|make-link-menu|app-context-menu/);
    expect(source).not.toMatch(/oncontextmenu|contextmenu:|addEventListener\("contextmenu"/);
    expect(source).not.toContain("contextMenuPositionFromEvent");
  });
});
