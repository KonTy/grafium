import { describe, it, expect } from "vitest";
import {
  TAG_HUES,
  tagHue,
  tagColorVar,
  tagHashKey,
  fnv1a,
  type TagHue,
} from "./tagColor";

describe("tagHashKey", () => {
  it("strips a leading # and lower-cases", () => {
    expect(tagHashKey("#Work")).toBe("work");
    expect(tagHashKey("WORK")).toBe("work");
  });

  it("keeps only the first hierarchy segment (/, \\)", () => {
    expect(tagHashKey("work/urgent")).toBe("work");
    expect(tagHashKey("#work/later/today")).toBe("work");
    expect(tagHashKey("work\\legacy")).toBe("work");
  });

  it("trims whitespace and is empty-safe", () => {
    expect(tagHashKey("   ")).toBe("");
    expect(tagHashKey("#")).toBe("");
    expect(tagHashKey("")).toBe("");
  });

  it("skips leading empty segments so a leading separator still hashes the family", () => {
    expect(tagHashKey("/work")).toBe("work");
    expect(tagHashKey("#/work")).toBe("work");
    expect(tagHashKey("\\work")).toBe("work");
    expect(tagHashKey("#/work/urgent")).toBe("work");
  });

  it("normalizes separator-only tags to the empty key", () => {
    expect(tagHashKey("/")).toBe("");
    expect(tagHashKey("#/")).toBe("");
    expect(tagHashKey("//")).toBe("");
    expect(tagHashKey("#\\/")).toBe("");
  });
});

describe("fnv1a", () => {
  it("is deterministic and returns an unsigned 32-bit integer", () => {
    const a = fnv1a("hello");
    const b = fnv1a("hello");
    expect(a).toBe(b);
    expect(Number.isInteger(a)).toBe(true);
    expect(a).toBeGreaterThanOrEqual(0);
    expect(a).toBeLessThanOrEqual(0xffffffff);
  });

  it("matches the known FNV-1a 32-bit reference vectors", () => {
    // Canonical FNV-1a/32 test vectors.
    expect(fnv1a("")).toBe(0x811c9dc5);
    expect(fnv1a("a")).toBe(0xe40c292c);
    expect(fnv1a("foobar")).toBe(0xbf9cf968);
  });

  it("differs for different inputs", () => {
    expect(fnv1a("work")).not.toBe(fnv1a("play"));
  });
});

describe("tagHue", () => {
  it("always returns a hue from the palette", () => {
    for (const tag of ["work", "idea", "café", "", "#", "日本語", "a-b_c"]) {
      expect(TAG_HUES).toContain(tagHue(tag));
    }
  });

  it("is deterministic across many calls", () => {
    const first = tagHue("project/alpha");
    for (let i = 0; i < 1000; i++) {
      expect(tagHue("project/alpha")).toBe(first);
    }
  });

  it("is case-insensitive", () => {
    expect(tagHue("Recipes")).toBe(tagHue("recipes"));
    expect(tagHue("#TODO")).toBe(tagHue("todo"));
  });

  it("shares one hue across a hierarchical tag family", () => {
    const parent = tagHue("work");
    expect(tagHue("work/urgent")).toBe(parent);
    expect(tagHue("work/later")).toBe(parent);
    expect(tagHue("#work/2024/q1")).toBe(parent);
    expect(tagHue("work\\archived")).toBe(parent);
  });

  it("does not collapse leading-separator tags onto the empty-key hue", () => {
    // A leading `/` used to make every such tag hash the empty parent key and
    // share one colour. Now the first non-empty segment decides the hue.
    expect(tagHue("#/work")).toBe(tagHue("work"));
    expect(tagHue("/home")).toBe(tagHue("home"));
    const leading = ["/work", "/home", "/health", "/finance", "/reading", "/music"];
    expect(new Set(leading.map(tagHue)).size).toBeGreaterThan(1);
  });

  it("distinguishes different families (not all one colour)", () => {
    const families = ["work", "home", "health", "finance", "reading", "music"];
    const hues = new Set(families.map(tagHue));
    // The whole point is that colour carries information: several distinct
    // families must not collapse onto a single hue.
    expect(hues.size).toBeGreaterThan(1);
  });

  it("is unicode-safe and deterministic for non-ASCII input", () => {
    expect(() => tagHue("café")).not.toThrow();
    expect(() => tagHue("日本語")).not.toThrow();
    expect(() => tagHue("emoji-🚀")).not.toThrow();
    expect(tagHue("café")).toBe(tagHue("café"));
    // Different unicode strings should be independently hashable.
    expect(TAG_HUES).toContain(tagHue("naïve/accents"));
  });

  it("distributes well across the whole palette", () => {
    const counts = new Map<TagHue, number>();
    for (const hue of TAG_HUES) counts.set(hue, 0);
    const N = 4000;
    for (let i = 0; i < N; i++) {
      const hue = tagHue(`tag-${i}`);
      counts.set(hue, (counts.get(hue) ?? 0) + 1);
    }
    // Every hue is used...
    for (const hue of TAG_HUES) {
      expect(counts.get(hue)!).toBeGreaterThan(0);
    }
    // ...and reasonably balanced: no bucket wildly over/under the mean.
    const mean = N / TAG_HUES.length;
    for (const hue of TAG_HUES) {
      const c = counts.get(hue)!;
      expect(c).toBeGreaterThan(mean * 0.6);
      expect(c).toBeLessThan(mean * 1.4);
    }
  });
});

describe("tagColorVar", () => {
  it("returns the CSS token matching the tag's hue", () => {
    for (const tag of ["work", "work/urgent", "reading", "café"]) {
      expect(tagColorVar(tag)).toBe(`var(--accent-${tagHue(tag)})`);
    }
  });
});
