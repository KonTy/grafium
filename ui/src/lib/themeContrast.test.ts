import { describe, it, expect } from "vitest";
import {
  contrastRatio,
  relativeLuminance,
  parseHex,
  meetsAA,
  WCAG_AA_NORMAL,
} from "./contrast";
import { themes, type ThemeColors } from "./themes";

describe("contrast helper (WCAG relative luminance / ratio)", () => {
  it("computes luminance endpoints", () => {
    expect(relativeLuminance("#000000")).toBeCloseTo(0, 6);
    expect(relativeLuminance("#ffffff")).toBeCloseTo(1, 6);
  });

  it("computes the canonical black/white and identity ratios", () => {
    expect(contrastRatio("#000000", "#ffffff")).toBeCloseTo(21, 5);
    expect(contrastRatio("#ffffff", "#ffffff")).toBeCloseTo(1, 5);
  });

  it("is order-independent", () => {
    expect(contrastRatio("#1e1e2e", "#cdd6f4")).toBeCloseTo(
      contrastRatio("#cdd6f4", "#1e1e2e"),
      10
    );
  });

  it("supports 3-digit hex and a leading #", () => {
    expect(parseHex("#fff")).toEqual({ r: 255, g: 255, b: 255 });
    expect(parseHex("000")).toEqual({ r: 0, g: 0, b: 0 });
  });

  it("rejects malformed hex loudly", () => {
    expect(() => parseHex("#12g456")).toThrow();
    expect(() => parseHex("#1234")).toThrow();
  });

  it("matches a mid-grey reference contrast", () => {
    // #767676 on white is the classic ~4.54:1 AA-passing grey.
    expect(contrastRatio("#767676", "#ffffff")).toBeGreaterThanOrEqual(4.5);
    expect(meetsAA("#767676", "#ffffff")).toBe(true);
  });
});

// The 8 semantic accent tokens, mapped to their ThemeColors fields.
const ACCENT_FIELDS: Array<[string, keyof ThemeColors]> = [
  ["orange", "accentOrange"],
  ["magenta", "accentMagenta"],
  ["green", "accentGreen"],
  ["yellow", "accentYellow"],
  ["blue", "accentBlue"],
  ["cyan", "accentCyan"],
  ["purple", "accentPurple"],
  ["red", "accentRed"],
];

describe("every theme's accent palette meets WCAG AA against its background", () => {
  it("includes the true-black OLED theme", () => {
    const oled = themes.find((t) => t.id === "oled");
    expect(oled).toBeDefined();
    expect(oled!.colors.bgPrimary.toLowerCase()).toBe("#000000");
  });

  // The guard: iterate every theme × every accent token and assert >= 4.5:1.
  // This is what keeps a future theme edit from silently shipping an
  // unreadable tag/link colour.
  for (const theme of themes) {
    for (const [hue, field] of ACCENT_FIELDS) {
      it(`${theme.id}: ${hue} is legible`, () => {
        const fg = theme.colors[field] as string;
        const bg = theme.colors.bgPrimary;
        const ratio = contrastRatio(fg, bg);
        expect(
          ratio,
          `${theme.id} --accent-${hue} (${fg}) on ${bg} = ${ratio.toFixed(2)}:1`
        ).toBeGreaterThanOrEqual(WCAG_AA_NORMAL);
      });
    }
  }
});

describe("worst-case contrast summary", () => {
  it("reports the weakest accent per theme (all >= 4.5:1)", () => {
    const rows = themes.map((theme) => {
      let worst = Infinity;
      let worstHue = "";
      for (const [hue, field] of ACCENT_FIELDS) {
        const ratio = contrastRatio(
          theme.colors[field] as string,
          theme.colors.bgPrimary
        );
        if (ratio < worst) {
          worst = ratio;
          worstHue = hue;
        }
      }
      return { id: theme.id, worstHue, worst: Number(worst.toFixed(2)) };
    });
    for (const row of rows) {
      expect(row.worst).toBeGreaterThanOrEqual(WCAG_AA_NORMAL);
    }
    // Surface the numbers when run with reporter output.
    // eslint-disable-next-line no-console
    console.table(rows);
  });
});
