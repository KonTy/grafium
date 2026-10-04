import { bindingFromEvent, canonicalBinding, chordHasCommandModifier, chordKey, isFunctionKey } from "./keyBinding";

export const BIONIC_SHORTCUT = "mod+alt+b";
export const BOOKMARK_SHORTCUT = "mod+alt+m";

export type ReaderShortcutId = "toggle-bionic" | "bookmark";
export type ReaderShortcutBindings = Record<ReaderShortcutId, string[]>;

let readerBindings: ReaderShortcutBindings = {
  "toggle-bionic": [BIONIC_SHORTCUT],
  bookmark: [BOOKMARK_SHORTCUT],
};

/**
 * The keys for Bionic reading and bookmarks. The app keeps these in step with
 * Settings, and passes them on to the isolated reader.
 */
export function setReaderShortcutBindings(next: unknown): void {
  if (!next || typeof next !== "object") return;
  const read = (value: unknown) => Array.isArray(value)
    ? value.filter((item): item is string => typeof item === "string" && item.length <= 80).slice(0, 6)
    : null;
  const record = next as Record<string, unknown>;
  const bionic = read(record["toggle-bionic"]);
  const bookmark = read(record.bookmark);
  readerBindings = {
    "toggle-bionic": bionic ?? readerBindings["toggle-bionic"],
    bookmark: bookmark ?? readerBindings.bookmark,
  };
}

export function readerShortcutBindings(): ReaderShortcutBindings {
  return { "toggle-bionic": [...readerBindings["toggle-bionic"]], bookmark: [...readerBindings.bookmark] };
}

/** Shared with the isolated reader: no application state or native dependencies. */
export function readerShortcut(event: KeyboardEvent): ReaderShortcutId | null {
  if (event.defaultPrevented || event.repeat) return null;
  const pressed = bindingFromEvent(event);
  // Reader shortcuts hold Ctrl, Cmd or Alt, or are a function key, so typing
  // never triggers one.
  if (!pressed || (!chordHasCommandModifier(pressed) && !isFunctionKey(chordKey(pressed)))) return null;
  for (const id of ["toggle-bionic", "bookmark"] as const) {
    if (readerBindings[id].some((binding) => canonicalBinding(binding) === pressed)) return id;
  }
  return null;
}
