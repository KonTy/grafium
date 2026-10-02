export interface ReaderTheme {
  background: string;
  text: string;
  link: string;
  selectionBackground: string;
  selectionText: string;
}

export const DEFAULT_READER_THEME: ReaderTheme = {
  background: "#1e1e2e", text: "#cdd6f4", link: "#f5c2e7",
  selectionBackground: "#cba6f7", selectionText: "#1e1e2e",
};

// Only literal colors cross the document boundary: no URLs, variables, markup,
// declarations, or context-dependent keywords (including currentColor).
const component = "[+-]?(?:\\d+(?:\\.\\d+)?|\\.\\d+)%?";
const functionalColor = new RegExp(`^(?:rgb|hsl)a?\\((?:${component}\\s*,\\s*${component}\\s*,\\s*${component}(?:\\s*,\\s*${component})?|${component}\\s+${component}\\s+${component}(?:\\s*/\\s*${component})?)\\)$`, "i");
export function readerColor(value: unknown): value is string {
  return typeof value === "string" && value.length <= 100
    && (/^#(?:[\da-f]{3}|[\da-f]{4}|[\da-f]{6}|[\da-f]{8})$/i.test(value) || functionalColor.test(value));
}

export function isReaderTheme(value: unknown): value is ReaderTheme {
  return !!value && typeof value === "object"
    && Object.keys(DEFAULT_READER_THEME).every(key => readerColor((value as Record<string, unknown>)[key]));
}

export function readReaderTheme(): ReaderTheme {
  if (typeof document === "undefined") return { ...DEFAULT_READER_THEME };
  const style = getComputedStyle(document.documentElement);
  const read = (name: string, fallback: string) => {
    const value = style.getPropertyValue(name).trim();
    return readerColor(value) ? value : fallback;
  };
  const background = read("--bg-primary", DEFAULT_READER_THEME.background);
  return {
    background,
    text: read("--text-primary", DEFAULT_READER_THEME.text),
    link: read("--text-link", DEFAULT_READER_THEME.link),
    selectionBackground: read("--accent", DEFAULT_READER_THEME.selectionBackground),
    selectionText: background,
  };
}

/** Immediately reports the palette, then coalesces root theme changes. */
export function observeReaderTheme(callback: (theme: ReaderTheme) => void): () => void {
  let previous = "";
  const update = () => {
    const theme = readReaderTheme();
    const key = JSON.stringify(theme);
    if (key !== previous) { previous = key; callback(theme); }
  };
  const observer = new MutationObserver(update);
  observer.observe(document.documentElement, { attributes: true });
  update();
  return () => observer.disconnect();
}
