import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const read = (name: string) =>
  readFileSync(join(process.cwd(), "src", "components", name), "utf8");

/** A rule's selector slice starts after the previous `}`, so a comment sitting
 *  in between becomes part of it. Strip comments before matching selectors. */
const styleOf = (name: string) => {
  const source = read(name);
  return source
    .slice(source.lastIndexOf("<style>"), source.lastIndexOf("</style>"))
    .replace(/\/\*[\s\S]*?\*\//g, "");
};

const ruleIn = (block: string, selector: string) => {
  const from = block.indexOf(`${selector} {`);
  expect(from, `missing rule: ${selector}`).toBeGreaterThan(-1);
  return block.slice(from, block.indexOf("}", from));
};

const widthQuery = /@media \(max-width:/;

describe("narrow-screen guards", () => {
  it("keeps the block selection toolbar reachable", () => {
    // Six children including a button whose label is live progress text. With
    // no wrapping, Delete and Clear were pushed off the right edge, and
    // `body { overflow: hidden }` means nothing can scroll them back.
    const css = styleOf("PageContent.svelte");
    expect(ruleIn(css, ".selection-toolbar")).toContain("flex-wrap: wrap");
    expect(ruleIn(css, ".selection-toolbar-btn")).toContain("flex-shrink: 0");
  });

  it("gives Settings a narrow layout at all", () => {
    // Measured before this existed: at a 320px viewport the keymap description
    // column resolved to 0px wide and 87px tall, because 48px of page padding
    // plus the section's 16px left less room than the two key columns reserve.
    const css = styleOf("Settings.svelte");
    expect(css, "Settings had no width breakpoint").toMatch(widthQuery);
    const narrow = css.slice(css.search(widthQuery));
    expect(ruleIn(narrow, ".settings-page")).toContain("padding: 20px 14px");
    expect(ruleIn(narrow, ".keymap-row")).toContain("grid-template-columns: 1fr");
    // The slider and number field were 180px of unyielding width in a row
    // whose content box is far smaller than that on a phone.
    expect(ruleIn(css, ".setting-row")).toContain("flex-wrap: wrap");
    const range = ruleIn(css, '.setting-range input[type="range"]');
    expect(range).toContain("flex: 1 1 90px");
    expect(range, "a fixed width cannot yield").not.toMatch(/(^|[\s;])width:\s*120px/);
  });

  it("keeps toasts on screen", () => {
    // Pinned to `right: 16px` with a flat 380px cap, the left edge went
    // negative below ~396px and swallowed the icon and first words.
    expect(ruleIn(styleOf("Toaster.svelte"), ".toaster")).toContain(
      "max-width: min(380px, calc(100vw - 32px))",
    );
  });

  it("keeps all four flashcard grades reachable", () => {
    const css = styleOf("FlashcardReview.svelte");
    expect(css, "FlashcardReview had no width breakpoint").toMatch(widthQuery);
    expect(ruleIn(css.slice(css.search(widthQuery)), ".grades")).toContain(
      "grid-template-columns: repeat(2, 1fr)",
    );
  });

  it("avoids basis-zero flex on wrapped action buttons", () => {
    // `flex: 1` is `flex: 1 1 0%`, which needs `min-width: auto` to stay
    // readable -- the thing WebKitGTK gets wrong for nested flex containers.
    expect(styleOf("AIWritingPanel.svelte")).toContain(".actions button { flex: 1 0 auto; }");
  });
});
