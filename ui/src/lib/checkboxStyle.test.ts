import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const read = (p: string) => readFileSync(join(process.cwd(), "src", p), "utf8");

const css = read("styles/global.css");

/** Comments discuss declarations they do not make; strip them before asserting. */
const declarationsOf = (rule: string) => rule.replace(/\/\*[\s\S]*?\*\//g, "");

const ruleAt = (selector: string) => {
  const from = css.indexOf(selector);
  expect(from, `missing rule: ${selector}`).toBeGreaterThan(-1);
  return declarationsOf(css.slice(from, css.indexOf("\n}", from)));
};

describe("checkbox tick", () => {
  it("is drawn, not typed", () => {
    // U+2713 is in none of the UI font stacks, so it resolves to whichever
    // symbol font the OS ships. Its side bearings decided how centred the tick
    // looked, which is why it drifted from machine to machine. Measured in
    // Chromium the glyph sat 2.0% right and 3.5% low of the box centre; the
    // drawn one lands within 0.3% horizontally and exactly on centre.
    const tick = ruleAt('input[type="checkbox"]:checked::after');
    expect(tick).toContain('content: ""');
    expect(tick).not.toContain("✓");
    expect(tick).not.toContain("2713");
    expect(tick).not.toContain("font-weight");
  });

  it("centres by geometry, so it holds at any size or font", () => {
    const tick = ruleAt('input[type="checkbox"]:checked::after');
    // Rotating a rectangle that shows only its right and bottom borders puts
    // the ink dead centre horizontally and 0.05em low for this width:height
    // ratio. Change either dimension and the correction below stops matching.
    expect(tick).toContain("width: 0.3em");
    expect(tick).toContain("height: 0.58em");
    expect(tick).toContain("border-width: 0 0.16em 0.16em 0");
    expect(tick).toContain("transform: translateY(-0.05em) rotate(45deg)");
    // Sized in em throughout, so the tick tracks the box rather than needing a
    // second set of numbers per place a checkbox appears.
    expect(tick).not.toMatch(/\d+px/);
  });

  it("covers the note tick and the native one with the same rule", () => {
    // One appearance everywhere was the point; a second copy is how they drift.
    const from = css.indexOf('input[type="checkbox"]:checked::after');
    const selectors = css.slice(from, css.indexOf("{", from));
    expect(selectors).toContain(".rendered-content .task-checkbox.checked::after");
  });

  it("does not let hover wash the fill out of a ticked box", () => {
    // `input:hover` outranks `input:checked`, so an unguarded hover rule would
    // replace the filled colour with the empty-box tint on mouseover.
    const from = css.indexOf('input[type="checkbox"]:not(:checked):hover');
    expect(from, "hover tint must exclude :checked").toBeGreaterThan(-1);
  });
});

describe("checkbox box", () => {
  it("drops native rendering so the shared look actually applies", () => {
    const base = ruleAt('input[type="checkbox"],\n.rendered-content .task-checkbox');
    expect(base).toContain("appearance: none");
    expect(base).toContain("-webkit-appearance: none");
  });

  it("is sized from the surrounding text rather than a fixed pixel count", () => {
    const base = ruleAt('input[type="checkbox"],\n.rendered-content .task-checkbox');
    expect(base).toContain("font-size: inherit");
    expect(base).toContain("width: 1.08em");
    expect(base).toContain("height: 1.08em");
  });

  it("keeps a focus ring after appearance:none removed the built-in one", () => {
    const ring = ruleAt('input[type="checkbox"]:focus-visible');
    expect(ring).toContain("box-shadow");
  });

  it("marks disabled boxes, which no longer differ on their own", () => {
    const off = ruleAt('input[type="checkbox"]:disabled');
    expect(off).toContain("opacity");
    expect(off).toContain("cursor: default");
  });
});

describe("checkbox styling lives in one place", () => {
  const components = [
    "components/ChatSwitcher.svelte",
    "components/AISettings.svelte",
    "components/ResearchSettings.svelte",
  ];

  it("leaves no component redefining the box or its tick", () => {
    for (const path of components) {
      const source = declarationsOf(read(path));
      expect(source, `${path} re-styles the tick`).not.toContain('content: "✓"');
      // accent-color does nothing once appearance is none; leaving it behind
      // reads as if the native control were still in play.
      expect(source, `${path} sets a dead accent-color`).not.toMatch(
        /input\[type="checkbox"\][^{]*\{[^}]*accent-color/,
      );
    }
  });
});
