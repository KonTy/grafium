import { describe, expect, it } from "vitest";
import { orderedListMarker } from "./orderedList";

describe("ordered list markers", () => {
  it.each([
    ["1. First", "1. ", "2. "],
    ["3) Third", "3) ", "4) "],
    ["  7.\tIndented", "  7.\t", "  8.\t"],
    ["0. Zero", "0. ", "1. "],
    ["2.", "2.", "3. "],
    ["2) ", "2) ", "3) "],
  ])("recognizes %s without reading other lines", (line, prefix, nextPrefix) => {
    expect(orderedListMarker(line)).toEqual({ prefix, nextPrefix });
  });

  it.each(["1.text", "1.5 metres", "3)word", "    1. Code", "\t1. Code",
    "`1. Code`", "\\1. Literal", "- 1. Nested source", "Text 1. Number", "1000000000. Too long"])(
    "does not treat %s as a list", (line) => {
      expect(orderedListMarker(line)).toBeNull();
    },
  );

  it("does not generate a marker outside Markdown's nine-digit limit", () => {
    expect(orderedListMarker("999999999. Last")?.nextPrefix).toBeNull();
  });
});
