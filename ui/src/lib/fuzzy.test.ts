import { describe, it, expect } from "vitest";
import { fuzzyScore, fuzzyMatches, fuzzyRank } from "./fuzzy";

describe("fuzzyScore", () => {
  it("matches a subsequence, not just a prefix", () => {
    expect(fuzzyMatches("tech/linux/net-config", "linux networking")).toBe(false);
    expect(fuzzyMatches("tech/linux/networking", "lnnet")).toBe(true);
    expect(fuzzyMatches("tech/linux/networking", "linuxnet")).toBe(true);
  });

  it("is case-insensitive", () => {
    expect(fuzzyMatches("Tech/Linux", "linux")).toBe(true);
    expect(fuzzyMatches("tech/linux", "LINUX")).toBe(true);
  });

  it("rejects a needle that isn't a subsequence", () => {
    expect(fuzzyScore("tech/linux", "windows")).toBeNull();
  });

  /// An empty box should show everything, not nothing.
  it("treats an empty needle as matching", () => {
    expect(fuzzyScore("anything", "")).toEqual({ score: 0, positions: [] });
    expect(fuzzyScore("anything", "   ")).toEqual({ score: 0, positions: [] });
  });

  /// Spaces separate fragments rather than needing a literal space, so
  /// "linux net" finds a path with a slash between them.
  it("ignores spaces in the query", () => {
    expect(fuzzyMatches("tech/linux/networking", "linux net")).toBe(true);
  });

  it("reports positions for highlighting", () => {
    const m = fuzzyScore("abc", "ac");
    expect(m?.positions).toEqual([0, 2]);
  });
});

describe("fuzzyRank", () => {
  /// The point of scoring: a boundary match should beat an incidental one.
  it("ranks path-boundary matches above mid-word matches", () => {
    const pages = ["colonel", "tech/linux/networking", "unrelated"];
    const ranked = fuzzyRank(pages, "ln", (p) => p);
    expect(ranked[0]).toBe("tech/linux/networking");
  });

  it("ranks a consecutive run above a scattered match", () => {
    const ranked = fuzzyRank(["c-a-t-s", "cats"], "cats", (p) => p);
    expect(ranked[0]).toBe("cats");
  });

  it("drops non-matches", () => {
    expect(fuzzyRank(["alpha", "beta"], "zzz", (p) => p)).toEqual([]);
  });

  /// An unstable sort makes a filtered list visibly shuffle as you type.
  it("is stable for equal scores", () => {
    const items = ["bbb", "aaa"];
    const a = fuzzyRank(items, "", (p) => p);
    const b = fuzzyRank(items, "", (p) => p);
    expect(a).toEqual(b);
    expect(a).toEqual(["aaa", "bbb"]);
  });

  it("returns everything for an empty query", () => {
    expect(fuzzyRank(["a", "b"], "", (p) => p)).toHaveLength(2);
  });
});
