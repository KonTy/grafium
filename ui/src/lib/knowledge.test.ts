import { describe, it, expect } from "vitest";
import {
  formatSourceLabel,
  formatWebSourceLabel,
  shouldShowIndexBanner,
  type ChatSource,
  type WebSource,
} from "./knowledge";

function source(overrides: Partial<ChatSource> = {}): ChatSource {
  return {
    index: 1,
    page_id: "p1",
    page_title: "Rust",
    block_id: "b1",
    date: null,
    ...overrides,
  };
}

describe("formatSourceLabel", () => {
  it("includes the citation index and page title", () => {
    expect(formatSourceLabel(source())).toBe("[1] · Rust");
  });

  it("inserts the date between the index and title when present", () => {
    const label = formatSourceLabel(
      source({ index: 3, page_title: "2026-03-14", date: "2026-03-14" })
    );
    expect(label).toBe("[3] · 2026-03-14 · 2026-03-14");
  });

  it("omits the date when it is null or undefined", () => {
    expect(formatSourceLabel(source({ date: undefined }))).toBe("[1] · Rust");
  });
});

describe("formatWebSourceLabel", () => {
  function web(overrides: Partial<WebSource> = {}): WebSource {
    return {
      number: 1,
      title: "How creatine works",
      url: "https://www.example.com/creatine",
      ...overrides,
    };
  }

  it("shows the citation number, the bare hostname, and the title", () => {
    // The leading www. is stripped so the host reads cleanly.
    expect(formatWebSourceLabel(web())).toBe(
      "[1] example.com · How creatine works"
    );
  });

  it("collapses to just the host when the title duplicates it", () => {
    expect(
      formatWebSourceLabel(web({ title: "example.com", url: "https://example.com" }))
    ).toBe("[1] example.com");
  });

  it("collapses to just the host when the title is blank", () => {
    expect(formatWebSourceLabel(web({ number: 4, title: "  " }))).toBe(
      "[4] example.com"
    );
  });

  it("falls back to the raw URL when it cannot be parsed", () => {
    expect(
      formatWebSourceLabel(web({ title: "", url: "not a url" }))
    ).toBe("[1] not a url");
  });
});

describe("shouldShowIndexBanner", () => {
  it("shows the banner only when the index is known to be empty", () => {
    expect(shouldShowIndexBanner(0)).toBe(true);
  });

  it("hides the banner when there are indexed chunks", () => {
    expect(shouldShowIndexBanner(42)).toBe(false);
  });

  it("hides the banner while status is still loading (null)", () => {
    expect(shouldShowIndexBanner(null)).toBe(false);
  });
});
