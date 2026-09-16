import { describe, it, expect } from "vitest";
import {
  loadWikiLinkPages,
  rankWikiLinkTitles,
  wikiLinkCloseExtra,
  wikiLinkReplacement,
  wikiLinkToken,
} from "./wikiLinkCompletion";

describe("wikiLinkToken", () => {
  it("opens on [[ and captures the query", () => {
    expect(wikiLinkToken("see [[suppl")).toEqual({ from: 4, query: "suppl" });
    expect(wikiLinkToken("[[")).toEqual({ from: 0, query: "" });
  });

  it("uses the last unclosed opener on the line", () => {
    expect(wikiLinkToken("[[done]] then [[hea")).toEqual({ from: 14, query: "hea" });
  });

  it("ignores closed links and a single bracket", () => {
    expect(wikiLinkToken("[[page]] more")).toBeNull();
    expect(wikiLinkToken("foo [bar")).toBeNull();
  });

  it("searches only the target, never an alias or escaped wiki literal", () => {
    expect(wikiLinkToken("[[Insulin res")).toEqual({ from: 0, query: "Insulin res" });
    expect(wikiLinkToken("[[Insulin resistance|#insulin")).toBeNull();
    expect(wikiLinkToken("[[Insulin resistance|")).toBeNull();
    expect(wikiLinkToken("\\[[literal")).toBeNull();
    expect(wikiLinkToken("\\\\[[target")).toEqual({ from: 2, query: "target" });
  });
});

describe("wikiLinkReplacement", () => {
  it("wraps the chosen title", () => {
    expect(wikiLinkReplacement("Self/Health/Supplements")).toBe(
      "[[Self/Health/Supplements]]",
    );
  });

  it.each([
    ["|#insulin_resistance]] rest", "[[Insulin resistance|#insulin_resistance]] rest"],
    ["stance|#insulin_resistance]] rest", "[[Insulin resistance|#insulin_resistance]] rest"],
    ["|健康 & café | display]] rest", "[[Insulin resistance|健康 & café | display]] rest"],
    ["|]] rest", "[[Insulin resistance|]] rest"],
    ["|#insulin_resistance", "[[Insulin resistance|#insulin_resistance]]"],
    ["stance]] rest", "[[Insulin resistance]] rest"],
  ])("preserves an existing alias or target suffix: %s", (afterCursor, expected) => {
    const replacement = wikiLinkReplacement("Insulin resistance", afterCursor);
    const remaining = afterCursor.slice(wikiLinkCloseExtra(afterCursor));
    expect(replacement + remaining).toBe(expected);
  });
});

describe("wikiLinkCloseExtra", () => {
  it("consumes a half-typed closer", () => {
    expect(wikiLinkCloseExtra("]] leftover")).toBe(2);
    expect(wikiLinkCloseExtra("] leftover")).toBe(1);
    expect(wikiLinkCloseExtra(" leftover")).toBe(0);
    expect(wikiLinkCloseExtra(" | next table cell |")).toBe(0);
  });
});

describe("rankWikiLinkTitles", () => {
  const pages = [
    { title: "Projects/Apollo" },
    { title: "Self/Health/Supplements" },
    { title: "Recipes/Soup" },
  ];

  it("ranks a path segment like Logseq (suppl → Supplements)", () => {
    expect(rankWikiLinkTitles(pages, "suppl").map((p) => p.title)).toEqual([
      "Self/Health/Supplements",
    ]);
  });

  it("keeps recency order when the query is empty", () => {
    expect(rankWikiLinkTitles(pages, "").map((p) => p.title)).toEqual(
      pages.map((p) => p.title),
    );
  });
});

describe("loadWikiLinkPages", () => {
  it("searches existing titles when a query is typed", async () => {
    const hits = await loadWikiLinkPages("suppl", {
      search: async (query) => {
        expect(query).toBe("suppl");
        return [{ title: "Self/Health/Supplements" }, { title: "Recipes/Soup" }];
      },
      listRecent: async () => {
        throw new Error("empty-query path should not list recent pages");
      },
    });
    expect(hits.map((p) => p.title)).toEqual(["Self/Health/Supplements"]);
  });

  it("lists recent pages for a bare [[", async () => {
    const hits = await loadWikiLinkPages("", {
      search: async () => {
        throw new Error("bare [[ should not search");
      },
      listRecent: async () => [{ title: "Today" }, { title: "Inbox" }],
    });
    expect(hits.map((p) => p.title)).toEqual(["Today", "Inbox"]);
  });
});
