import { describe, expect, it } from "vitest";
import { getBulletMinHeight, getHeadingLevel, isQuoteBlockContent } from "./blockLayout";

describe("block quote layout", () => {
  it("treats a block that starts with > as a quotation", () => {
    expect(isQuoteBlockContent("> wise words")).toBe(true);
    expect(isQuoteBlockContent("  > indented")).toBe(true);
    expect(isQuoteBlockContent("plain text")).toBe(false);
  });

  it("does not mistake the closing line of a sync conflict for a quotation", () => {
    expect(isQuoteBlockContent(">>>>>>> THEIRS (merge 3fa9c2d1)")).toBe(false);
    // Without the signature it is just text starting with `>`.
    expect(isQuoteBlockContent(">>>>>>> THEIRS")).toBe(true);
  });
});

describe("block heading layout", () => {
  it("recognizes Markdown headings without treating hashtags as headings", () => {
    expect(getHeadingLevel("  # Heading")).toBe(1);
    expect(getHeadingLevel("###### Small heading")).toBe(6);
    expect(getHeadingLevel("#tag")).toBe(0);
  });

  it("uses absolute gutter heights that cannot compound with heading font size", () => {
    expect([
      getBulletMinHeight("# Heading"),
      getBulletMinHeight("## Heading"),
      getBulletMinHeight("### Heading"),
      getBulletMinHeight("#### Heading"),
      getBulletMinHeight("Plain block"),
    ]).toEqual(["32px", "28px", "25px", "24px", "24px"]);
  });
});
