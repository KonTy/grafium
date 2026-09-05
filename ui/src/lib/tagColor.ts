// Deterministic, information-carrying colours for `#tags`.
//
// Every tag is mapped to one of the theme's semantic accent hues by *hashing
// its name*, so a given tag is always the same colour everywhere in the app.
// Colour therefore carries identity (scannable at a glance) instead of being
// mere decoration.
//
// Two deliberate properties:
//   * Hierarchical tags share their parent's hue — `#work/urgent` and
//     `#work/later` both hash from `work`, so a tag family reads as one colour
//     group. (We hash the *first* segment only.)
//   * The hash is a stable, well-distributed FNV-1a over the UTF-8 bytes of the
//     normalized name, so the mapping is deterministic across runs and
//     platforms and spreads evenly across the palette.
//
// This module is intentionally free of DOM/theme imports so it stays a pure,
// unit-testable function. The returned hue name maps to a `--accent-<hue>` CSS
// custom property defined per-theme in `themes.ts` / `global.css`.

/**
 * The ordered set of semantic accent hues a tag can be assigned. The order is
 * part of the hashing contract: changing it re-colours existing tags, so keep
 * it stable. Length (8) is the modulus the hash is reduced into.
 */
export const TAG_HUES = [
  "orange",
  "magenta",
  "green",
  "yellow",
  "blue",
  "cyan",
  "purple",
  "red",
] as const;

export type TagHue = (typeof TAG_HUES)[number];

const encoder =
  typeof TextEncoder !== "undefined" ? new TextEncoder() : undefined;

/**
 * Normalize a raw tag token to the key we hash: strip a leading `#`, lower-case
 * for case-insensitive identity, trim whitespace, and keep only the first
 * hierarchy segment (split on `/` or `\`, matching the backend tag parser). So
 * `#Work/Urgent`, `work/later` and `WORK` all normalize to `work`.
 */
export function tagHashKey(tag: string): string {
  const trimmed = tag.trim().replace(/^#+/, "");
  const firstSegment = trimmed.split(/[\\/]/)[0];
  return firstSegment.toLowerCase();
}

/**
 * 32-bit FNV-1a hash of a string's UTF-8 bytes. Unicode-safe (hashes code
 * points via their UTF-8 encoding, not UTF-16 units) and returns an unsigned
 * 32-bit integer.
 */
export function fnv1a(input: string): number {
  const bytes = encoder
    ? encoder.encode(input)
    : utf8BytesFallback(input);
  let hash = 0x811c9dc5; // FNV offset basis
  for (let i = 0; i < bytes.length; i++) {
    hash ^= bytes[i];
    // hash *= 16777619 (FNV prime), kept in 32-bit range via Math.imul.
    hash = Math.imul(hash, 0x01000193);
  }
  return hash >>> 0;
}

/**
 * Map a tag name to its stable accent hue. Empty / whitespace-only input is
 * safe: it normalizes to `""`, which hashes deterministically to a fixed hue.
 */
export function tagHue(tag: string): TagHue {
  const key = tagHashKey(tag);
  const index = fnv1a(key) % TAG_HUES.length;
  return TAG_HUES[index];
}

/**
 * The CSS token reference for a tag's colour, e.g. `var(--accent-blue)`. Used by
 * the markdown renderer so tag colouring flows entirely through the theme
 * token system (no raw hex in the rendered HTML).
 */
export function tagColorVar(tag: string): string {
  return `var(--accent-${tagHue(tag)})`;
}

/**
 * Minimal UTF-8 encoder used only if the runtime lacks TextEncoder (it never
 * should in a modern webview or Node). Keeps {@link fnv1a} unicode-correct and
 * dependency-free.
 */
function utf8BytesFallback(input: string): number[] {
  const out: number[] = [];
  for (const ch of input) {
    let code = ch.codePointAt(0)!;
    if (code < 0x80) {
      out.push(code);
    } else if (code < 0x800) {
      out.push(0xc0 | (code >> 6), 0x80 | (code & 0x3f));
    } else if (code < 0x10000) {
      out.push(
        0xe0 | (code >> 12),
        0x80 | ((code >> 6) & 0x3f),
        0x80 | (code & 0x3f)
      );
    } else {
      out.push(
        0xf0 | (code >> 18),
        0x80 | ((code >> 12) & 0x3f),
        0x80 | ((code >> 6) & 0x3f),
        0x80 | (code & 0x3f)
      );
    }
  }
  return out;
}
