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

  it("takes the chat list out of flow below the drawer breakpoint", () => {
    // Measured at a 320px viewport: the inline list left the conversation
    // 156px of a 300px host. As an off-canvas drawer the conversation gets
    // the full 300px, and it stays 300px while the drawer is open because
    // the drawer overlays rather than displaces.
    const css = styleOf("ChatSwitcher.svelte");
    const drawer = css.slice(css.indexOf("@media (max-width: 560px)"));
    expect(drawer, "ChatSwitcher lost its drawer breakpoint").not.toBe("");
    expect(ruleIn(drawer, ".switcher")).toContain("position: absolute");
    expect(ruleIn(drawer, ".switcher")).toContain("transform: translateX(-102%)");
    // Out of flow but still on the page is a tab stop the user cannot see.
    expect(ruleIn(drawer, ".switcher:not(.open)")).toContain("visibility: hidden");
    expect(ruleIn(drawer, ".switcher.open")).toContain("transform: translateX(0)");
  });

  it("only shows the chat drawer toggle where the drawer exists", () => {
    // The toggle and its scrim are dead weight on a desktop width, where the
    // list is always beside the conversation.
    const css = styleOf("ChatView.svelte");
    expect(ruleIn(css, ".chat-bar")).toContain("display: none");
    expect(ruleIn(css, ".scrim")).toContain("display: none");
    const narrow = css.slice(css.indexOf("@media (max-width: 560px)"));
    expect(narrow, "ChatView lost its drawer breakpoint").not.toBe("");
    expect(ruleIn(narrow, ".chat-bar")).toContain("display: flex");
    expect(ruleIn(narrow, ".scrim")).toContain("display: block");
  });

  it("anchors the chat drawer to the conversation host", () => {
    // An absolutely positioned drawer with no positioned ancestor escapes to
    // the viewport and covers the app chrome.
    expect(ruleIn(styleOf("ChatView.svelte"), ".conversation-host")).toContain(
      "position: relative",
    );
  });
});
