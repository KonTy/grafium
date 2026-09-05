// WCAG 2.x colour-contrast helpers.
//
// These implement the standard relative-luminance and contrast-ratio formulas
// (https://www.w3.org/TR/WCAG21/#dfn-relative-luminance and #dfn-contrast-ratio)
// so the theme system can *prove* — in tests — that every semantic accent stays
// legible (>= 4.5:1, WCAG AA for normal text) against its own theme background,
// rather than relying on eyeballing.

/** A colour parsed into 8-bit sRGB channels. */
export interface Rgb {
  r: number;
  g: number;
  b: number;
}

/**
 * Parse a `#rgb` or `#rrggbb` hex string into 0–255 channels.
 * Throws on anything that isn't a valid 3- or 6-digit hex colour so a typo in a
 * theme definition fails loudly in tests instead of silently scoring as black.
 */
export function parseHex(hex: string): Rgb {
  const cleaned = hex.trim().replace(/^#/, "");
  const expanded =
    cleaned.length === 3
      ? cleaned
          .split("")
          .map((c) => c + c)
          .join("")
      : cleaned;
  if (!/^[0-9a-fA-F]{6}$/.test(expanded)) {
    throw new Error(`Invalid hex colour: ${JSON.stringify(hex)}`);
  }
  return {
    r: parseInt(expanded.slice(0, 2), 16),
    g: parseInt(expanded.slice(2, 4), 16),
    b: parseInt(expanded.slice(4, 6), 16),
  };
}

/** Linearize a single 0–255 sRGB channel to its 0–1 linear-light value. */
function channelLuminance(value: number): number {
  const c = value / 255;
  return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
}

/**
 * WCAG relative luminance of a colour, in [0, 1].
 * Accepts a hex string or already-parsed {@link Rgb}.
 */
export function relativeLuminance(color: string | Rgb): number {
  const { r, g, b } = typeof color === "string" ? parseHex(color) : color;
  return (
    0.2126 * channelLuminance(r) +
    0.7152 * channelLuminance(g) +
    0.0722 * channelLuminance(b)
  );
}

/**
 * WCAG contrast ratio between two colours, in [1, 21].
 * The order of arguments does not matter.
 */
export function contrastRatio(a: string | Rgb, b: string | Rgb): number {
  const la = relativeLuminance(a);
  const lb = relativeLuminance(b);
  const lighter = Math.max(la, lb);
  const darker = Math.min(la, lb);
  return (lighter + 0.05) / (darker + 0.05);
}

/** WCAG AA contrast threshold for normal-size text. */
export const WCAG_AA_NORMAL = 4.5;

/** True when `fg` on `bg` meets WCAG AA (4.5:1) for normal text. */
export function meetsAA(fg: string | Rgb, bg: string | Rgb): boolean {
  return contrastRatio(fg, bg) >= WCAG_AA_NORMAL;
}

/**
 * Mix two opaque hex colours the way CSS `color-mix(in srgb, a a%, b)` does:
 * a per-channel weighted average in gamma-encoded sRGB space. Used to reproduce
 * the callout title/background compositions (which are built with `color-mix`)
 * so the contrast guard measures the colours that actually render, not the raw
 * accent tokens.
 */
export function mixSrgb(a: string, b: string, aWeightPct: number): string {
  const wa = Math.max(0, Math.min(100, aWeightPct)) / 100;
  const wb = 1 - wa;
  const A = parseHex(a);
  const B = parseHex(b);
  const ch = (x: number, y: number) => Math.round(x * wa + y * wb);
  const hex = (n: number) => n.toString(16).padStart(2, "0");
  return `#${hex(ch(A.r, B.r))}${hex(ch(A.g, B.g))}${hex(ch(A.b, B.b))}`;
}
