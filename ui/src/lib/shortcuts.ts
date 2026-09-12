import { keymap_manager, type Shortcut } from "./keymap";

export interface ShortcutRow {
  id: string;
  description: string;
  category: string;
  /** Vim/Logseq chords such as "g j". */
  chords: string[];
  /** Modifier combos such as "mod+shift+j". */
  modifiers: string[];
}

function isModifierBinding(binding: string): boolean {
  return binding.includes("+");
}

/** Groups alias bindings into one help row per action. */
export function groupShortcutRows(shortcuts: Shortcut[]): ShortcutRow[] {
  const rows = new Map<string, ShortcutRow>();
  const order: string[] = [];
  for (const shortcut of shortcuts) {
    const id = shortcut.id || shortcut.description || shortcut.binding;
    let row = rows.get(id);
    if (!row) {
      row = {
        id,
        description: shortcut.description || shortcut.binding,
        category: shortcut.category || "other",
        chords: [],
        modifiers: [],
      };
      rows.set(id, row);
      order.push(id);
    }
    const list = isModifierBinding(shortcut.binding) ? row.modifiers : row.chords;
    if (!list.includes(shortcut.binding)) list.push(shortcut.binding);
  }
  return order.map((id) => rows.get(id)!);
}

/** Groups registered shortcuts by category, collapsing aliases onto one row. */
export function getShortcutRowsByCategory(): Map<string, ShortcutRow[]> {
  const map = new Map<string, ShortcutRow[]>();
  for (const row of groupShortcutRows(keymap_manager.getShortcuts())) {
    const existing = map.get(row.category);
    if (existing) existing.push(row);
    else map.set(row.category, [row]);
  }
  return map;
}

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

/** Renders an internal binding string for display, e.g. "mod+k" -> "Ctrl-K". */
export function formatBinding(binding: string): string {
  if (!binding.includes("+")) return binding;
  const parts = binding.split("+").map((part) => part.trim().toLowerCase());
  if (parts.includes("shift") && parts.includes(".")) {
    const mods = parts.filter((part) => part !== "shift" && part !== ".");
    return [...mods.map((part) => formatBindingPart(part)), ">"].join("-");
  }
  if (parts.includes("shift") && parts.includes(",")) {
    const mods = parts.filter((part) => part !== "shift" && part !== ",");
    return [...mods.map((part) => formatBindingPart(part)), "<"].join("-");
  }
  return binding
    .split("+")
    .map((part) => formatBindingPart(part.trim()))
    .join("-");
}

function formatBindingPart(part: string): string {
  const lower = part.toLowerCase();
  if (lower === "mod" || lower === "ctrl" || lower === "control" || lower === "meta") return "Ctrl";
  if (lower === "shift") return "Shift";
  if (lower === "alt") return "Alt";
  if (part.length === 1) return part.toUpperCase();
  return part;
}

export function formatBindingList(bindings: string[]): string {
  return bindings.map(formatBinding).join(" | ");
}
