import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const read = (file: string) => readFileSync(join(process.cwd(), "src/components", file), "utf8");

// Both editors create their own CodeMirror view, and neither is mounted in tests,
// so what matters here is that each one wires the spell checker the same way.
describe.each([
  ["BlockEditor.svelte", 2],
  ["UnifiedPageEditor.svelte", 1],
])("%s spell check wiring", (file, resets) => {
  const source = read(file);

  it("installs the spell checker beside the Markdown language", () => {
    expect(source).toContain(
      'import { spellcheckExtension, type SpellcheckRequest } from "../lib/spellcheckExtension";',
    );
    expect(source).toContain('import SpellcheckMenu from "./SpellcheckMenu.svelte";');
    expect(source).toMatch(/markdown\(\),\s+spellcheckExtension\(\{/);
  });

  it("closes the other editor menus when the spelling menu opens", () => {
    expect(source).toMatch(
      /onContextMenu: \(request\) => \{[^}]*imageSizeMenu = null;[^}]*spellMenu = request;/,
    );
  });

  it("drops the open request with the editor view it points into", () => {
    expect(source).toContain("let spellMenu = $state.raw<SpellcheckRequest | null>(null);");
    expect(source.match(/^\s*spellMenu = null;$/gm)).toHaveLength(resets);
  });

  it("shows the menu for the open request and closes it again", () => {
    expect(source).toMatch(
      /\{#if spellMenu\}\s+<SpellcheckMenu request=\{spellMenu\} onClose=\{\(\) => \(spellMenu = null\)\} \/>\s+\{\/if\}/,
    );
  });
});
