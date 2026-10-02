export const BIONIC_SHORTCUT = "mod+alt+b";
export const BOOKMARK_SHORTCUT = "mod+alt+m";

/** Shared with the isolated reader: no application state or native dependencies. */
export function readerShortcut(event: KeyboardEvent): "toggle-bionic" | "bookmark" | null {
  if (event.defaultPrevented || event.repeat || event.isComposing || event.keyCode === 229
    || event.shiftKey || !event.altKey || event.getModifierState?.("AltGraph")) return null;
  const mac = typeof navigator !== "undefined" && navigator.platform.includes("Mac");
  if (mac ? !event.metaKey || event.ctrlKey : !event.ctrlKey || event.metaKey) return null;
  const key = event.code || event.key.toLowerCase();
  if (key === "KeyB" || key === "b") return "toggle-bionic";
  if (key === "KeyM" || key === "m") return "bookmark";
  return null;
}
