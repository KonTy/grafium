import { describe, it, expect } from "vitest";
import { toggleWrapText } from "./editorFormat";

describe("toggleWrapText", () => {
  it("wraps a selection in italic markers, keeping inner text selected", () => {
    // "hello world", select "world" (6..11)
    const r = toggleWrapText("hello world", 6, 11, "*");
    expect(r.doc).toBe("hello *world*");
    expect(r.doc.slice(r.selStart, r.selEnd)).toBe("world");
  });

  it("wraps a selection in bold markers", () => {
    const r = toggleWrapText("hello world", 6, 11, "**");
    expect(r.doc).toBe("hello **world**");
    expect(r.doc.slice(r.selStart, r.selEnd)).toBe("world");
  });

  it("wraps a selection in strikethrough markers", () => {
    const r = toggleWrapText("hello world", 6, 11, "~~");
    expect(r.doc).toBe("hello ~~world~~");
    expect(r.doc.slice(r.selStart, r.selEnd)).toBe("world");
  });

  it("unwraps when the markers are inside the selection", () => {
    // select "**world**" (6..15)
    const r = toggleWrapText("hello **world**", 6, 15, "**");
    expect(r.doc).toBe("hello world");
    expect(r.doc.slice(r.selStart, r.selEnd)).toBe("world");
  });

  it("unwraps when the markers sit just outside the selection", () => {
    // select just "world" (8..13) inside hello **world**
    const r = toggleWrapText("hello **world**", 8, 13, "**");
    expect(r.doc).toBe("hello world");
    expect(r.doc.slice(r.selStart, r.selEnd)).toBe("world");
  });

  it("inserts an empty pair and places the cursor between when no selection", () => {
    // cursor at end of "hello " (6)
    const r = toggleWrapText("hello ", 6, 6, "**");
    expect(r.doc).toBe("hello ****");
    expect(r.selStart).toBe(r.selEnd);
    expect(r.selStart).toBe(8); // between the two ** markers
    expect(r.doc.slice(0, r.selStart)).toBe("hello **");
  });

  it("no-selection insert works for single-char italic markers too", () => {
    const r = toggleWrapText("", 0, 0, "*");
    expect(r.doc).toBe("**");
    expect(r.selStart).toBe(1);
    expect(r.selEnd).toBe(1);
  });

  it("round-trips wrap then unwrap", () => {
    const wrapped = toggleWrapText("abc", 0, 3, "~~");
    expect(wrapped.doc).toBe("~~abc~~");
    const unwrapped = toggleWrapText(
      wrapped.doc,
      wrapped.selStart,
      wrapped.selEnd,
      "~~"
    );
    expect(unwrapped.doc).toBe("abc");
    expect(unwrapped.doc.slice(unwrapped.selStart, unwrapped.selEnd)).toBe("abc");
  });
});
