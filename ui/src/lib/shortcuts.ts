import { keymap_manager, type Shortcut } from "./keymap";

/** Groups registered shortcuts by category, defaulting to "other". */
export function getShortcutsByCategory(): Map<string, Shortcut[]> {
  const map = new Map<string, Shortcut[]>();
  for (const shortcut of keymap_manager.getShortcuts()) {
    const category = shortcut.category || "other";
    const existing = map.get(category);
    if (existing) {
      existing.push(shortcut);
    } else {
      map.set(category, [shortcut]);
    }
  }
  return map;
}

/** Renders an internal binding string for display, e.g. "mod+k" -> "Ctrl + k". */
export function formatBinding(binding: string): string {
  return binding
    .replace(/mod/g, "Ctrl")
    .replace(/\+/g, " + ")
    .replace(/ {2}/g, "  then  ");
}
