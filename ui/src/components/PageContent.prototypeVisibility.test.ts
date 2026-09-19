import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const source = readFileSync(join(process.cwd(), "src/components/PageContent.svelte"), "utf8");

describe("continuous editor visibility", () => {
  it("keeps the implementation behind a disabled user-facing gate", () => {
    expect(source).toContain("const SHOW_UNIFIED_EDITOR_PROTOTYPE = false;");
    expect(source).toContain("{#if SHOW_UNIFIED_EDITOR_PROTOTYPE}");
    expect(source).toContain("&& localStorage.getItem(unifiedEditorPrototypeKey(pageId)) === \"1\"");
    expect(source).toContain("<UnifiedPageEditor");
  });
});
