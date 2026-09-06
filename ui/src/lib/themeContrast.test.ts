import { describe, it, expect } from "vitest";
import {
  contrastRatio,
  relativeLuminance,
  parseHex,
  meetsAA,
  mixSrgb,
  WCAG_AA_NORMAL,
} from "./contrast";
import { themes, type ThemeColors } from "./themes";

// ── WCAG helper unit tests ────────────────────────────────────────────────
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

  it("mixSrgb averages channels like color-mix(in srgb, …)", () => {
    expect(mixSrgb("#000000", "#ffffff", 50).toLowerCase()).toBe("#808080");
    expect(mixSrgb("#ff0000", "#0000ff", 100).toLowerCase()).toBe("#ff0000");
    expect(mixSrgb("#ff0000", "#0000ff", 0).toLowerCase()).toBe("#0000ff");
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

// Callout kind -> the accent it is tinted with (see global.css .callout-*).
const CALLOUT_ACCENTS: Array<[string, keyof ThemeColors]> = [
  ["tip", "accentGreen"],
  ["note", "accentBlue"],
  ["important", "accentRed"],
  ["caution", "accentOrange"],
  ["pinned", "accentPurple"],
  ["warning", "accentYellow"],
];

// The surfaces an accent's *text* is actually rendered on. Body text and chat
// answers sit on bg-primary; backlink/task-item hovers and flashcards were
// deliberately pinned to bg-secondary (see PageContent/Statistics/FlashcardReview)
// precisely so the lightest surface hosting accent text is bg-secondary and the
// palette can be tuned to stay AA on it. surface-raised / bg-hover host only
// chrome (buttons, non-tag hover fills), never accent text.
const TEXT_SURFACES: Array<[string, keyof ThemeColors]> = [
  ["bg-primary", "bgPrimary"],
  ["bg-secondary", "bgSecondary"],
];

describe("theme metadata", () => {
  it("includes the true-black OLED theme", () => {
    const oled = themes.find((t) => t.id === "oled");
    expect(oled).toBeDefined();
    expect(oled!.colors.bgPrimary.toLowerCase()).toBe("#000000");
  });
});

// ── Guard 1: every accent is AA (>=4.5:1) on every surface that hosts its text
describe("accents meet WCAG AA on every surface they render text on", () => {
  for (const theme of themes) {
    it(`${theme.id}: all accents AA on bg-primary and bg-secondary`, () => {
      for (const [hue, field] of ACCENT_FIELDS) {
        const fg = theme.colors[field] as string;
        for (const [sName, sField] of TEXT_SURFACES) {
          const bg = theme.colors[sField] as string;
          const ratio = contrastRatio(fg, bg);
          expect(
            ratio,
            `${theme.id} --accent-${hue} (${fg}) on ${sName} (${bg}) = ${ratio.toFixed(2)}:1`
          ).toBeGreaterThanOrEqual(WCAG_AA_NORMAL);
        }
      }
    });
  }
});

// ── Guard 2: callout title/background compositions are AA on every theme
// The callout title is color-mix(accent 70%, text-primary) and the callout
// background is color-mix(accent 10%, bg-primary) (see global.css). Measuring
// the *composited* colours is what caught the Gruvbox important-callout
// regression (title had dropped to 3.80:1); measuring raw accents hid it.
describe("callout title/background compositions meet WCAG AA", () => {
  for (const theme of themes) {
    it(`${theme.id}: all callout titles AA on their callout background`, () => {
      const c = theme.colors;
      for (const [kind, field] of CALLOUT_ACCENTS) {
        const accent = c[field] as string;
        const title = mixSrgb(accent, c.textPrimary, 70);
        const bg = mixSrgb(accent, c.bgPrimary, 10);
        const ratio = contrastRatio(title, bg);
        expect(
          ratio,
          `${theme.id} ${kind} callout title (${title}) on bg (${bg}) = ${ratio.toFixed(2)}:1`
        ).toBeGreaterThanOrEqual(WCAG_AA_NORMAL);
      }
    });
  }

  it("gruvbox important callout is back above 4.5:1 (regression guard)", () => {
    const c = themes.find((t) => t.id === "gruvbox")!.colors;
    const title = mixSrgb(c.accentRed, c.textPrimary, 70);
    const bg = mixSrgb(c.accentRed, c.bgPrimary, 10);
    expect(contrastRatio(title, bg)).toBeGreaterThanOrEqual(WCAG_AA_NORMAL);
  });
});

// ── Guard 3: OLED control borders clear the 3:1 non-text contrast minimum
// True-black surfaces are the point of the OLED theme, but a near-black border
// (~1.1:1) makes inputs/cards dissolve into the void. A dedicated mid-grey
// border must stay visible (WCAG 1.4.11, 3:1) against every near-black surface.
describe("OLED control borders are visible (WCAG 1.4.11 non-text 3:1)", () => {
  const NON_TEXT_MIN = 3;
  it("--border clears 3:1 against #000 and the raised near-black surfaces", () => {
    const c = themes.find((t) => t.id === "oled")!.colors;
    const surfaces: Array<[string, string]> = [
      ["bg-primary", c.bgPrimary],
      ["bg-secondary", c.bgSecondary],
      ["surface-raised", c.surfaceRaised],
    ];
    for (const [name, bg] of surfaces) {
      const ratio = contrastRatio(c.border, bg);
      expect(
        ratio,
        `OLED --border (${c.border}) on ${name} (${bg}) = ${ratio.toFixed(2)}:1`
      ).toBeGreaterThanOrEqual(NON_TEXT_MIN);
    }
  });
});

// ── Guard 4: colour-vision-deficiency separation between the 8 accent hues
// Colour carries tag identity, so the 8 hues must not *converge* under red-green
// colour blindness. Eight AA-compliant, on-theme hues can't be made strongly
// distinct under dichromacy (that is why tags also carry the `#` prefix + text
// and external links carry a persistent ↗ marker), but no two may collapse onto
// each other. We simulate deuteranopia and protanopia (Machado 2009, severity
// 1.0, applied in linear RGB), convert to CIELAB and require a minimum pairwise
// ΔE (CIE76 — which folds luminance *and* chroma separation into one distance).
// This guards against a future palette edit re-introducing a convergent pair.
const CVD_MIN_DELTA_E = 1.5;
const CVD_MATRICES: Record<"deuteranopia" | "protanopia", number[]> = {
  deuteranopia: [
    0.367322, 0.860646, -0.227968, 0.280085, 0.672501, 0.047413, -0.01182,
    0.04294, 0.968881,
  ],
  protanopia: [
    0.152286, 1.052583, -0.204868, 0.114503, 0.786281, 0.099216, -0.003882,
    -0.048116, 1.051998,
  ],
};

function srgbToLinear(v: number): number {
  const c = v / 255;
  return c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}
function linearToSrgb(c: number): number {
  const v = c <= 0.0031308 ? 12.92 * c : 1.055 * Math.pow(c, 1 / 2.4) - 0.055;
  return Math.max(0, Math.min(255, Math.round(v * 255)));
}
function simulateCvd(hex: string, m: number[]): { r: number; g: number; b: number } {
  const { r, g, b } = parseHex(hex);
  const R = srgbToLinear(r), G = srgbToLinear(g), B = srgbToLinear(b);
  const clamp = (x: number) => Math.max(0, Math.min(1, x));
  return {
    r: linearToSrgb(clamp(m[0] * R + m[1] * G + m[2] * B)),
    g: linearToSrgb(clamp(m[3] * R + m[4] * G + m[5] * B)),
    b: linearToSrgb(clamp(m[6] * R + m[7] * G + m[8] * B)),
  };
}
function toLab({ r, g, b }: { r: number; g: number; b: number }): [number, number, number] {
  const R = srgbToLinear(r), G = srgbToLinear(g), B = srgbToLinear(b);
  const X = (0.4124 * R + 0.3576 * G + 0.1805 * B) / 0.95047;
  const Y = 0.2126 * R + 0.7152 * G + 0.0722 * B;
  const Z = (0.0193 * R + 0.1192 * G + 0.9505 * B) / 1.08883;
  const f = (t: number) => (t > 0.008856 ? Math.cbrt(t) : 7.787 * t + 16 / 116);
  const fx = f(X), fy = f(Y), fz = f(Z);
  return [116 * fy - 16, 500 * (fx - fy), 200 * (fy - fz)];
}
function deltaE(a: [number, number, number], b: [number, number, number]): number {
  return Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]);
}
function minCvdSeparation(theme: ThemeColors, m: number[]): { min: number; pair: string } {
  const labs = ACCENT_FIELDS.map(
    ([hue, field]) => [hue, toLab(simulateCvd(theme[field] as string, m))] as const
  );
  let min = Infinity, pair = "";
  for (let i = 0; i < labs.length; i++) {
    for (let j = i + 1; j < labs.length; j++) {
      const d = deltaE(labs[i][1], labs[j][1]);
      if (d < min) {
        min = d;
        pair = `${labs[i][0]}/${labs[j][0]}`;
      }
    }
  }
  return { min, pair };
}

describe("accent hues stay separable under colour-vision deficiency", () => {
  for (const theme of themes) {
    it(`${theme.id}: no accent pair collapses under deuteranopia/protanopia`, () => {
      for (const [type, m] of Object.entries(CVD_MATRICES)) {
        const { min, pair } = minCvdSeparation(theme.colors, m);
        expect(
          min,
          `${theme.id} ${type}: closest pair ${pair} ΔE=${min.toFixed(2)} (< ${CVD_MIN_DELTA_E})`
        ).toBeGreaterThanOrEqual(CVD_MIN_DELTA_E);
      }
    });
  }
});

// ── Reporting: worst-case ratio per theme, per surface, plus callouts + CVD.
describe("worst-case contrast summary", () => {
  it("reports the weakest composition per theme (all pass their thresholds)", () => {
    const rows = themes.map((theme) => {
      const c = theme.colors;
      const worstOn = (field: keyof ThemeColors) => {
        let w = Infinity, hue = "";
        for (const [h, f] of ACCENT_FIELDS) {
          const r = contrastRatio(c[f] as string, c[field] as string);
          if (r < w) { w = r; hue = h; }
        }
        return { w: Number(w.toFixed(2)), hue };
      };
      const prim = worstOn("bgPrimary");
      const sec = worstOn("bgSecondary");
      let worstCallout = Infinity, worstKind = "";
      for (const [kind, field] of CALLOUT_ACCENTS) {
        const accent = c[field] as string;
        const r = contrastRatio(
          mixSrgb(accent, c.textPrimary, 70),
          mixSrgb(accent, c.bgPrimary, 10)
        );
        if (r < worstCallout) { worstCallout = r; worstKind = kind; }
      }
      const cvd = Math.min(
        minCvdSeparation(c, CVD_MATRICES.deuteranopia).min,
        minCvdSeparation(c, CVD_MATRICES.protanopia).min
      );
      return {
        id: theme.id,
        "bg-primary": `${prim.w} (${prim.hue})`,
        "bg-secondary": `${sec.w} (${sec.hue})`,
        callout: `${worstCallout.toFixed(2)} (${worstKind})`,
        "cvd ΔE": Number(cvd.toFixed(2)),
      };
    });
    for (const row of rows) {
      expect(Number(row["bg-primary"].split(" ")[0])).toBeGreaterThanOrEqual(WCAG_AA_NORMAL);
      expect(Number(row["bg-secondary"].split(" ")[0])).toBeGreaterThanOrEqual(WCAG_AA_NORMAL);
      expect(Number(row.callout.split(" ")[0])).toBeGreaterThanOrEqual(WCAG_AA_NORMAL);
      expect(row["cvd ΔE"]).toBeGreaterThanOrEqual(CVD_MIN_DELTA_E);
    }
    // eslint-disable-next-line no-console
    console.table(rows);
  });
});

/**
 * Actionable text — tree rows, menu items — must be readable on the surfaces
 * it actually renders on.
 *
 * The guard previously only checked accent hues, so it stayed green while an
 * audit measured tree labels at 2.89:1 on Flexoki Light and menu items at
 * 3.59:1: the components were using `--text-secondary`, a de-emphasis token,
 * for text you are meant to read and click. Pinning the composition here is
 * what stops the next component reaching for the same token.
 */
describe("actionable text meets WCAG AA on the surfaces it renders on", () => {
  for (const theme of themes) {
    it(`${theme.id}: primary text is legible on every surface`, () => {
      const c = theme.colors;
      for (const [name, surface] of [
        ["bg-primary", c.bgPrimary],
        ["bg-secondary", c.bgSecondary],
      ] as const) {
        const ratio = contrastRatio(c.textPrimary, surface);
        expect(
          ratio,
          `${theme.id}: --text-primary on ${name} is ${ratio.toFixed(2)}:1`,
        ).toBeGreaterThanOrEqual(4.5);
      }
    });
  }
});
