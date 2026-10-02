export const BIONIC_WORD_RE = /[\p{L}\p{M}\p{N}][\p{L}\p{M}\p{N}'’_-]*/gu;

export function bionicPrefixLength(word: string): number {
  const chars = Array.from(word);
  if (chars.length <= 1) return chars.length;
  if (chars.length <= 3) return 1;
  return Math.max(1, Math.ceil(chars.length * 0.42));
}
