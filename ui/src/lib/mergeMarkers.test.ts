import { describe, expect, it } from "vitest";
import { mergeMarkerKind, renderMergeMarkerBlock } from "./mergeMarkers";
import { renderBlock } from "./markdown";

const YOURS = "<<<<<<< YOURS (merge 3fa9c2d1)";
const DIVIDER = "======= (merge 3fa9c2d1)";
const THEIRS = ">>>>>>> THEIRS (merge 3fa9c2d1)";

describe("mergeMarkerKind", () => {
  it("recognises the three lines sync writes around a conflict", () => {
    expect(mergeMarkerKind(YOURS)).toBe("yours");
    expect(mergeMarkerKind(DIVIDER)).toBe("divider");
    expect(mergeMarkerKind(THEIRS)).toBe("theirs");
  });

  it("tolerates stray whitespace the editor may leave around a line", () => {
    expect(mergeMarkerKind(`  ${YOURS}  `)).toBe("yours");
    expect(mergeMarkerKind(`${DIVIDER}\n`)).toBe("divider");
  });

  it("ignores text that merely looks like a marker", () => {
    for (const text of [
      "<<<<<<< YOURS",
      "=======",
      ">>>>>>> THEIRS",
      "<<<<<<< YOURS (merge 3FA9C2D1)",
      "<<<<<<< YOURS (merge 3fa9c2d)",
      "<<<<<<< YOURS (merge 3fa9c2d12)",
      "<<<<<<< YOURS (merge zzzzzzzz)",
      "======= (merge 3fa9c2d1) and more",
      "use >>>>>>> THEIRS (merge 3fa9c2d1) in prose",
      "> a real quotation",
      "plain text",
      "",
    ]) {
      expect(mergeMarkerKind(text), text).toBeNull();
    }
  });

  it("does not claim a block that holds more than the one marker line", () => {
    expect(mergeMarkerKind(`${YOURS}\nmine\n${DIVIDER}`)).toBeNull();
  });
});

describe("renderMergeMarkerBlock", () => {
  it("shows the marker text itself, escaped, with a class per side", () => {
    const yours = renderMergeMarkerBlock(YOURS)!;
    expect(yours).toContain('class="merge-marker merge-marker-yours"');
    expect(yours).toContain("&lt;&lt;&lt;&lt;&lt;&lt;&lt; YOURS (merge 3fa9c2d1)</div>");
    expect(yours).not.toContain("<<<");

    const theirs = renderMergeMarkerBlock(THEIRS)!;
    expect(theirs).toContain('class="merge-marker merge-marker-theirs"');
    expect(theirs).toContain("&gt;&gt;&gt;&gt;&gt;&gt;&gt; THEIRS (merge 3fa9c2d1)</div>");

    expect(renderMergeMarkerBlock(DIVIDER)).toContain('class="merge-marker merge-marker-divider"');
  });

  it("gives each marker a short hint for mouse users", () => {
    expect(renderMergeMarkerBlock(YOURS)).toContain('title="Your version, from this device.');
    expect(renderMergeMarkerBlock(THEIRS)).toContain('title="End of the other device');
  });

  it("returns null for any other block", () => {
    expect(renderMergeMarkerBlock("hello")).toBeNull();
    expect(renderMergeMarkerBlock("> quote")).toBeNull();
  });
});

describe("renderBlock with conflict markers", () => {
  it("renders a marker as a marker, not as nested block quotes", () => {
    const html = renderBlock(THEIRS);
    expect(html).toContain("merge-marker-theirs");
    expect(html).not.toContain("<blockquote");
  });

  it("renders the divider as a marker rather than a setext heading underline", () => {
    const html = renderBlock(DIVIDER);
    expect(html).toContain("merge-marker-divider");
    expect(html).not.toMatch(/<h[1-6]/);
  });

  it("still renders real quotations and ordinary text as before", () => {
    expect(renderBlock("> a real quotation")).toContain("<blockquote");
    expect(renderBlock("> a real quotation")).not.toContain("merge-marker");
    expect(renderBlock("see ======= for details")).not.toContain("merge-marker");
  });
});
