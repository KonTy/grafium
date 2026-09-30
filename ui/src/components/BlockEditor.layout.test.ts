import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const blockEditor = readFileSync(join(process.cwd(), "src/components/BlockEditor.svelte"), "utf8");
const pageContent = readFileSync(join(process.cwd(), "src/components/PageContent.svelte"), "utf8");

describe("BlockEditor outline layout", () => {
  it("renders only the connected focused thread, not unrelated ancestor fragments", () => {
    expect(blockEditor).not.toContain("guides?: boolean[]");
    expect(blockEditor).not.toContain("{#each guides");
    expect(pageContent).not.toContain("guides={getBlockGuides(block.id)}");
  });
});
