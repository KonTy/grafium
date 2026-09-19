import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const source = readFileSync(join(process.cwd(), "src", "components/AllPages.svelte"), "utf8");
const css = source.slice(source.lastIndexOf("<style>"), source.lastIndexOf("</style>"));

/** Comments discuss declarations they do not make; strip them before asserting. */
const stripComments = (block: string) => block.replace(/\/\*[\s\S]*?\*\//g, "");

const mediaQuery = (() => {
  const at = css.indexOf("@media (max-width: 640px)");
  expect(at, "missing the narrow-screen media query").toBeGreaterThan(-1);
  return stripComments(css.slice(at));
})();

const desktop = stripComments(css.slice(0, css.indexOf("@media (max-width: 640px)")));

const ruleIn = (block: string, selector: string) => {
  const from = block.indexOf(`${selector} {`);
  expect(from, `missing rule: ${selector}`).toBeGreaterThan(-1);
  return block.slice(from, block.indexOf("}", from));
};

describe("All Pages toolbar on narrow screens", () => {
  it("wraps the control row instead of overflowing it", () => {
    // Four groups need ~595px. The panel can be half that, and the row has
    // nowhere to scroll, so without wrapping it pushed the whole page wider
    // than the window and cut "Recent / A-Z" off the right edge.
    expect(ruleIn(desktop, ".browser-controls")).toContain("flex-wrap: wrap");
  });

  it("never shrinks a group below its labels", () => {
    // This is the guard that does not depend on `min-width: auto`. WebKitGTK,
    // which is what Tauri renders with on Linux, mis-computes the automatic
    // minimum size of a nested flex container, and the labels collapsed to one
    // letter per line. An item that cannot shrink cannot collapse.
    expect(ruleIn(desktop, ".control-group")).toContain("flex-shrink: 0");
    expect(ruleIn(desktop, ".mode-btn")).toContain("white-space: nowrap");
  });

  it("lets narrow-screen groups grow to fill a row but not shrink", () => {
    // `flex: 1` is `flex: 1 1 0%`: basis zero plus shrink, which is exactly the
    // combination that collapses when the automatic minimum is unreliable.
    for (const selector of [".control-group", ".mode-btn"]) {
      const rule = ruleIn(mediaQuery, selector);
      expect(rule).toContain("flex: 1 0 auto");
      expect(rule, `${selector} must not use basis-0 flex`).not.toMatch(/flex:\s*1\s*;/);
    }
  });

  it("only stretches the toolbar's own Import Media button", () => {
    // `.btn-import-media` doubles as the generic secondary button, so the
    // bulk-rename "Preview" carries the same class and an unscoped rule stretched
    // it across its entire row.
    expect(mediaQuery).toContain(".controls > .btn-import-media");
    expect(mediaQuery, "unscoped rule also hits bulk-rename Preview").not.toMatch(
      /^\s*\.btn-import-media\s*\{/m,
    );
  });
});
