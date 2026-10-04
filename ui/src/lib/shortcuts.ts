import { keymap_manager, type Shortcut } from "./keymap";
import { shortcutBindings, shortcutDefinition } from "./shortcutRegistry";

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

const KEY_NAMES: Record<string, string> = {
  escape: "Esc", enter: "Enter", space: "Space", tab: "Tab", backspace: "Backspace",
  delete: "Delete", arrowup: "↑", arrowdown: "↓", arrowleft: "←", arrowright: "→",
  pageup: "PageUp", pagedown: "PageDown", home: "Home", end: "End", plus: "+",
};

function formatSingleKey(key: string): string {
  const lower = key.toLowerCase();
  if (KEY_NAMES[lower]) return KEY_NAMES[lower];
  if (/^f\d{1,2}$/.test(lower)) return lower.toUpperCase();
  return key;
}

/** Renders an internal binding string for display, e.g. "mod+k" -> "Ctrl-K". */
export function formatBinding(binding: string): string {
  const presses = binding.trim().split(/\s+/);
  // Sequences read as typed: "g j".
  if (presses.length > 1) return presses.map(formatSingleKey).join(" ");
  if (!binding.includes("+") || binding.trim() === "+") return formatSingleKey(binding.trim());
  const parts = binding.split("+").map((part) => part.trim().toLowerCase());
  if (parts.includes("shift") && parts.includes(".")) {
    const mods = parts.filter((part) => part !== "shift" && part !== ".");
    return [...mods.map((part) => formatBindingPart(part)), ">"].join("-");
  }
  if (parts.includes("shift") && parts.includes(",")) {
    const mods = parts.filter((part) => part !== "shift" && part !== ",");
    return [...mods.map((part) => formatBindingPart(part)), "<"].join("-");
  }
  if (parts.includes("shift") && parts.includes("=")) {
    const mods = parts.filter((part) => part !== "shift" && part !== "=");
    return [...mods.map((part) => formatBindingPart(part)), "+"].join("-");
  }
  return binding
    .split("+")
    .map((part) => formatBindingPart(part.trim()))
    .join("-");
}

function formatBindingPart(part: string): string {
  const lower = part.toLowerCase();
  if (lower === "mod") return isMac() ? "Cmd" : "Ctrl";
  if (lower === "ctrl" || lower === "control") return "Ctrl";
  if (lower === "meta") return "Cmd";
  if (lower === "shift") return "Shift";
  if (lower === "alt") return "Alt";
  if (KEY_NAMES[lower]) return KEY_NAMES[lower];
  if (/^f\d{1,2}$/.test(lower)) return lower.toUpperCase();
  if (part.length === 1) return part.toUpperCase();
  return part;
}

export function formatBindingList(bindings: string[]): string {
  return bindings.map(formatBinding).join(" | ");
}

function isMac(): boolean {
  return typeof navigator !== "undefined" && navigator.platform.includes("Mac");
}

export function bindingTitle(label: string, ...bindings: string[]): string {
  return bindings.length ? `${label} (${formatBindingList(bindings)})` : label;
}

export function shortcutTitle(label: string, actionId: string): string {
  return bindingTitle(label, ...actionBindings(actionId));
}

function actionBindings(actionId: string): string[] {
  const registered = [...new Set(keymap_manager.getShortcuts()
    .filter(({ id }) => id === actionId).map(({ binding }) => binding))];
  if (registered.length) return registered;
  // Editor and flashcard shortcuts are not app-wide, so ask Settings directly.
  return shortcutDefinition(actionId) ? shortcutBindings(actionId) : [];
}

/** ARIA represents simultaneous keys, not navigation chord sequences. */
export function bindingAria(binding: string): string {
  return binding.split("+").map((part) => {
    const lower = part.toLowerCase();
    if (lower === "mod") return isMac() ? "Meta" : "Control";
    if (lower === "ctrl" || lower === "control") return "Control";
    if (lower === "meta") return "Meta";
    if (lower === "plus") return "Plus";
    return lower === "alt" ? "Alt" : lower === "shift" ? "Shift" : part;
  }).join("+");
}

export function shortcutAria(actionId: string): string | undefined {
  return actionBindings(actionId).filter((binding) => !binding.includes(" "))
    .map(bindingAria).join(" ") || undefined;
}
