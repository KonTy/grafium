import { describe, expect, it } from "vitest";
import { findQuoteSegment, normalizeLibraryQuote, type QuoteSegment } from "./privateBookQuoteMatch";

const loc = (cfi: string) => ({ kind: "epub" as const, cfi, rendererVersion: "test" });
const seg = (text: string, cfi: string): QuoteSegment => ({ text, locator: loc(cfi) });

describe("private book quote matching", () => {
  it("does not match tiny title-page fragments like by", () => {
    const found = findQuoteSegment("by", [[seg("by", "title"), seg("Someone", "title2")]]);
    expect(found).toBeNull();
  });

  it("normalizes Markdown, HTML, and entities before matching across segments", () => {
    const quote = "**Replace** the [fuel filter](#a) &amp; check <sup>1</sup> pressure `carefully` before closing the cover.";
    expect(normalizeLibraryQuote(quote)).toContain("replace the fuel filter & check pressure carefully");
    const found = findQuoteSegment(quote, [[
      seg("Preface", "preface"),
      seg("Replace the fuel filter & check", "match-a"),
      seg("pressure carefully before closing the cover and restarting the engine.", "match-b"),
    ]]);
    expect(found?.locator.cfi).toBe("match-a");
  });


  it("matches quotes split by inline formatting without requiring inserted spaces", () => {
    const quote = "He said hello, and then walked away from the repaired engine with the manual in hand.";
    const found = findQuoteSegment(quote, [[
      seg("He said ", "a"),
      seg("hello", "b"),
      seg(", and then walked away from the repaired engine with the manual in hand.", "c"),
    ]]);
    expect(found?.locator.cfi).toBe("a");
  });

  it("matches through a drop-cap span without adding a word gap", () => {
    const quote = "Hello, and then walked away from the repaired engine with the manual in hand.";
    const found = findQuoteSegment(quote, [[
      seg('<span class="dropcap">H</span>', "drop"),
      seg("ello, and then walked away from the repaired engine with the manual in hand.", "rest"),
    ]]);
    expect(found?.locator.cfi).toBe("drop");
  });

  it("returns null when no long normalized quote is found", () => {
    const found = findQuoteSegment("A long quote that does not appear anywhere in this synthetic fixed layout excerpt.", [[seg("Read aloud requires a reflowable EPUB.", "fixed")]]);
    expect(found).toBeNull();
  });
});
