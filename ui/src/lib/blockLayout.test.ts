import { describe, expect, it } from "vitest";
import { getBulletMinHeight, getHeadingLevel } from "./blockLayout";

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
