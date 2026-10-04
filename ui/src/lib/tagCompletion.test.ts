import { beforeEach, describe, expect, it, vi } from "vitest";
import {
  invalidateTagCandidates,
  loadTagSuggestions,
  rankTagSuggestions,
  tagInsertText,
  tagSuffixLength,
  tagToken,
} from "./tagCompletion";

describe("tagToken", () => {
  it("opens right after # and follows the typed tag text", () => {
    expect(tagToken("#")).toEqual({ from: 0, query: "" });
    expect(tagToken("notes #pro")).toEqual({ from: 6, query: "pro" });
    expect(tagToken("(#tech/lin")).toEqual({ from: 1, query: "tech/lin" });
    expect(tagToken("idea #Écoute")).toEqual({ from: 5, query: "Écoute" });
  });

  it("stays closed for headings, ##, URLs, mid-word # and inline code", () => {
    expect(tagToken("# Title")).toBeNull();
    expect(tagToken("##")).toBeNull();
    expect(tagToken("see page#part")).toBeNull();
    expect(tagToken("https://example.com/#top")).toBeNull();
    expect(tagToken("color `#fff")).toBeNull();
    expect(tagToken("#-dash")).toBeNull();
  });
});

describe("tag insertion", () => {
  it("writes a plain tag, or the bracketed form for other titles", () => {
    expect(tagInsertText("project")).toBe("#project");
    expect(tagInsertText("tech/linux")).toBe("#tech/linux");
    expect(tagInsertText("Two words")).toBe("#[[Two words]]");
    expect(tagInsertText("c++")).toBe("#[[c++]]");
  });

  it("replaces the rest of a tag when completing in the middle of it", () => {
    expect(tagSuffixLength("ject rest")).toBe(4);
    expect(tagSuffixLength(" next")).toBe(0);
  });
});

describe("rankTagSuggestions", () => {
  const tags = [
    { title: "project", updated_at: 1 },
    { title: "projects/home", updated_at: 5 },
    { title: "reading", updated_at: 9 },
    { title: "deep-work", updated_at: 3 },
  ];
  const pages = [
    { title: "Project plan", updated_at: 20 },
    { title: "project", updated_at: 30 },
    { title: "2026-10-03", updated_at: 40, is_journal: true },
  ];

  it("lists existing tags first, newest first, before other pages", () => {
    expect(rankTagSuggestions(tags, pages, "")).toEqual([
      { title: "reading", kind: "tag" },
      { title: "projects/home", kind: "tag" },
      { title: "deep-work", kind: "tag" },
      { title: "project", kind: "tag" },
      { title: "Project plan", kind: "page" },
    ]);
  });

  it("filters by substring, exact and prefix matches first, and never repeats a tag", () => {
    expect(rankTagSuggestions(tags, pages, "proj")).toEqual([
      { title: "project", kind: "tag" },
      { title: "projects/home", kind: "tag" },
      { title: "Project plan", kind: "page" },
      { title: "proj", kind: "new" },
    ]);
    expect(rankTagSuggestions(tags, pages, "work").map((s) => s.title)).toEqual(["deep-work", "work"]);
    expect(rankTagSuggestions(tags, pages, "home").map((s) => s.title)).toEqual(["projects/home", "home"]);
  });

  it("keeps what was typed ahead of loose matches, and suggests nothing when nothing matches", () => {
    // Enter accepts the first choice, so a new tag must not turn into a
    // different one that merely contains the same letters.
    expect(rankTagSuggestions(tags, pages, "rdng")).toEqual([
      { title: "rdng", kind: "new" },
      { title: "reading", kind: "tag" },
    ]);
    expect(rankTagSuggestions(tags, pages, "Reading")).toEqual([{ title: "reading", kind: "tag" }]);
    expect(rankTagSuggestions(tags, pages, "zzz")).toEqual([]);
    expect(rankTagSuggestions(tags, pages, "proj", 2)).toEqual([
      { title: "project", kind: "tag" },
      { title: "proj", kind: "new" },
    ]);
  });

  it("puts an exact page match before tags, and loose tags after what was typed", () => {
    expect(rankTagSuggestions([{ title: "wonderful-week" }, { title: "homework" }], [{ title: "Work" }], "work")).toEqual([
      { title: "Work", kind: "page" },
      { title: "homework", kind: "tag" },
      { title: "wonderful-week", kind: "tag" },
    ]);
    expect(rankTagSuggestions([{ title: "pxroxj" }], [{ title: "Project" }], "proj")).toEqual([
      { title: "Project", kind: "page" },
      { title: "proj", kind: "new" },
      { title: "pxroxj", kind: "tag" },
    ]);
  });
});

describe("loadTagSuggestions", () => {
  beforeEach(() => invalidateTagCandidates());

  it("fetches tags once and filters them locally as the query changes", async () => {
    const listTags = vi.fn(async () => [{ title: "alpha" }, { title: "beta" }]);
    const searchPages = vi.fn(async () => []);
    const listRecent = vi.fn(async () => []);
    const fetchers = { listTags, searchPages, listRecent };
    expect((await loadTagSuggestions("", fetchers, 1_000)).map((s) => s.title)).toEqual(["alpha", "beta"]);
    expect((await loadTagSuggestions("be", fetchers, 2_000)).map((s) => s.title)).toEqual(["beta", "be"]);
    expect(listTags).toHaveBeenCalledTimes(1);
    expect(searchPages).toHaveBeenCalledWith("be", 40);
    invalidateTagCandidates();
    await loadTagSuggestions("a", fetchers, 3_000);
    expect(listTags).toHaveBeenCalledTimes(2);
  });
});
