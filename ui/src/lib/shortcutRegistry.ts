/**
 * Every keyboard shortcut in Grafium, in one place.
 *
 * Each action has a stable id, the Settings section it belongs to, and its
 * default bindings. People can change the bindings in Settings > Keyboard
 * Shortcuts; changes are saved on this device for all graphs, and everything
 * that handles keys (the app-wide keymap, the block editor, flashcards, the
 * book reader) reads its bindings from here and updates when they change.
 *
 * Keys that make typing and widgets work (Enter, Backspace, arrows, Tab,
 * Escape, copy and paste, undo and redo) are listed as fixed rows instead,
 * and cannot be given to another action.
 */

import {
  bindingChords,
  bindingFromEvent,
  canonicalBinding,
  chordHasCommandModifier,
  chordKey,
  isFunctionKey,
} from "./keyBinding";

export type ShortcutSection =
  | "general"
  | "navigation"
  | "layout"
  | "journal"
  | "editor"
  | "chat"
  | "reading"
  | "flashcards"
  | "dialogs";

export const SHORTCUT_SECTIONS: ReadonlyArray<{ id: ShortcutSection; title: string }> = [
  { id: "general", title: "General" },
  { id: "navigation", title: "Navigation" },
  { id: "layout", title: "Layout" },
  { id: "journal", title: "Journal" },
  { id: "editor", title: "Editor" },
  { id: "chat", title: "Chat" },
  { id: "reading", title: "Library & reading" },
  { id: "flashcards", title: "Flashcards" },
  { id: "dialogs", title: "Menus & dialogs" },
];

/**
 * Where a shortcut works:
 * - `app`: anywhere (sequences like `g j` only while not editing text);
 * - `editor`: while editing a block;
 * - `flashcards`: while reviewing flashcards.
 */
export type ShortcutScope = "app" | "editor" | "flashcards";

export interface ShortcutDefinition {
  id: string;
  section: ShortcutSection;
  label: string;
  scope: ShortcutScope;
  defaults: readonly string[];
  /**
   * Only one key combination works: the book reader runs these too, and it
   * never waits for the second key of a sequence.
   */
  singlePress?: boolean;
}

export interface FixedShortcut {
  section: ShortcutSection;
  label: string;
  keys: readonly string[];
}

const app = (id: string, section: ShortcutSection, label: string, defaults: string[]): ShortcutDefinition =>
  ({ id, section, label, scope: "app", defaults });
const editor = (id: string, label: string, defaults: string[]): ShortcutDefinition =>
  ({ id, section: "editor", label, scope: "editor", defaults });
const flashcards = (id: string, label: string, defaults: string[]): ShortcutDefinition =>
  ({ id, section: "flashcards", label, scope: "flashcards", defaults });

export const SHORTCUT_DEFINITIONS: readonly ShortcutDefinition[] = [
  app("command-palette", "general", "Command palette", ["mod+shift+p"]),
  app("help", "general", "Contextual help", ["f1"]),
  app("search-global", "general", "Global search", ["mod+k"]),
  app("search-local", "general", "Focus current view's filter", ["mod+f"]),
  app("toggle-settings", "general", "Toggle settings", ["t s", "alt+s"]),
  app("import-media", "general", "Import media", ["alt+m"]),
  app("import-books", "general", "Import books", ["alt+b"]),
  app("zoom-in", "general", "Zoom in", ["mod+=", "mod+plus"]),
  app("zoom-out", "general", "Zoom out", ["mod+-"]),
  app("zoom-reset", "general", "Reset zoom", ["mod+0"]),

  app("go-journal", "navigation", "Go to today's journal", ["g j", "mod+shift+j"]),
  app("go-home", "navigation", "Go to home", ["g h", "mod+shift+h"]),
  app("go-all-pages", "navigation", "Go to all pages", ["g a", "mod+shift+a"]),
  app("go-graph", "navigation", "Go to graph view", ["g g", "mod+shift+g"]),
  app("go-flashcards", "navigation", "Go to flashcards", ["g f", "mod+shift+f"]),
  app("go-tasks", "navigation", "Go to tasks", ["mod+shift+t"]),
  app("go-studies", "navigation", "Go to Studies", ["g s"]),
  app("go-library", "navigation", "Go to Library", ["g l"]),
  app("go-chat", "navigation", "Go to Chat tab", ["alt+c"]),
  app("go-link", "navigation", "Go to link", ["mod+l"]),
  app("go-backward", "navigation", "Go backward", ["mod+["]),
  app("go-forward", "navigation", "Go forward", ["mod+]"]),

  app("toggle-left-sidebar", "layout", "Toggle left sidebar", ["t l", "mod+b"]),
  app("toggle-right-sidebar", "layout", "Toggle right sidebar", ["t r", "mod+shift+b", "mod+."]),
  app("toggle-wide", "layout", "Toggle wide mode", ["t w", "alt+w"]),
  app("toggle-zen", "layout", "Toggle zen mode", ["t z", "alt+z"]),
  app("toggle-theme", "layout", "Open theme settings", ["t t"]),

  app("go-journal-date", "journal", "Go to date calendar", ["mod+g"]),
  app("go-tomorrow", "journal", "Go to tomorrow's journal page", ["g t"]),
  app("go-next-journal", "journal", "Go to next journal", ["g n", "mod+shift+."]),
  app("go-prev-journal", "journal", "Go to previous journal", ["g p", "mod+shift+,"]),
  app("insert-time", "journal", "Insert current time", ["alt+t"]),
  app("insert-personal-diary", "journal", "Insert [[personal/diary]]", ["alt+d"]),

  editor("editor-bold", "Bold", ["mod+alt+shift+b"]),
  editor("editor-italic", "Italic", ["mod+i"]),
  editor("editor-strikethrough", "Strikethrough", ["mod+shift+k"]),
  editor("editor-save-source", "Save the page (continuous editor)", ["mod+s"]),

  // While a chat can receive them: the Chat screen, or a side-panel chat with focus.
  app("chat-context-next", "chat", "Next notes context in Chat", ["alt+n"]),
  app("chat-context-previous", "chat", "Previous notes context in Chat", ["alt+shift+n"]),
  app("chat-mode-next", "chat", "Next answer mode in Chat", ["alt+a"]),
  app("chat-mode-previous", "chat", "Previous answer mode in Chat", ["alt+shift+a"]),

  { ...app("toggle-bionic", "reading", "Toggle Bionic reading", ["mod+alt+b"]), singlePress: true },
  { ...app("bookmark", "reading", "Bookmark Library reading or playback", ["mod+alt+m"]), singlePress: true },

  flashcards("flashcards-reveal", "Show the answer, then grade Good", ["space", "enter"]),
  flashcards("flashcards-again", "Grade: Again", ["1"]),
  flashcards("flashcards-hard", "Grade: Hard", ["2"]),
  flashcards("flashcards-good", "Grade: Good", ["3"]),
  flashcards("flashcards-easy", "Grade: Easy", ["4"]),
];

/** Keys that make typing and widgets work. Shown in Settings, never reassigned. */
export const FIXED_SHORTCUTS: readonly FixedShortcut[] = [
  { section: "general", label: "Scroll to the top or bottom", keys: ["home", "end"] },
  { section: "general", label: "Scroll by a screen", keys: ["pageup", "pagedown"] },
  { section: "general", label: "Close the side panel or leave zen mode", keys: ["escape"] },
  { section: "editor", label: "New block", keys: ["enter"] },
  { section: "editor", label: "Line break inside a block", keys: ["shift+enter"] },
  { section: "editor", label: "Indent or outdent", keys: ["tab", "shift+tab"] },
  { section: "editor", label: "Delete an empty block", keys: ["backspace"] },
  { section: "editor", label: "Move to the block above or below", keys: ["arrowup", "arrowdown"] },
  { section: "editor", label: "Select blocks", keys: ["shift+arrowup", "shift+arrowdown"] },
  { section: "editor", label: "Stop editing", keys: ["escape"] },
  { section: "editor", label: "Commands, links and tags", keys: ["/", "[[", "#"] },
  // On Linux the window handles these before the page sees them, so they
  // can't be moved.
  { section: "editor", label: "Undo", keys: ["mod+z"] },
  { section: "editor", label: "Redo", keys: ["mod+shift+z", "mod+y"] },
  { section: "editor", label: "Copy, cut, paste, select all", keys: ["mod+c", "mod+x", "mod+v", "mod+a"] },
  { section: "reading", label: "Turn the page", keys: ["arrowleft", "arrowright", "pageup", "pagedown"] },
  { section: "reading", label: "Reader controls, full screen", keys: ["f8", "f11"] },
  { section: "dialogs", label: "Close", keys: ["escape"] },
  { section: "dialogs", label: "Move through a list", keys: ["arrowup", "arrowdown", "home", "end"] },
  { section: "dialogs", label: "Choose", keys: ["enter"] },
  { section: "dialogs", label: "Next or previous field", keys: ["tab", "shift+tab"] },
];

/** Bindings no action may take, with what they already do. */
const RESERVED: Record<string, string> = {
  enter: "starts a new block or chooses", "shift+enter": "adds a line break",
  tab: "indents and moves focus", "shift+tab": "outdents and moves focus",
  backspace: "deletes text", delete: "deletes text", escape: "closes and stops editing",
  arrowup: "moves the cursor", arrowdown: "moves the cursor",
  arrowleft: "moves the cursor", arrowright: "moves the cursor",
  "shift+arrowup": "selects blocks", "shift+arrowdown": "selects blocks",
  home: "scrolls to the top", end: "scrolls to the bottom",
  pageup: "scrolls by a screen", pagedown: "scrolls by a screen",
  "mod+c": "copies", "mod+x": "cuts", "mod+v": "pastes", "mod+a": "selects all",
  "mod+z": "undoes", "mod+shift+z": "redoes", "mod+y": "redoes",
  f8: "shows the reader controls", f11: "toggles full screen",
};

const DEFINITIONS = new Map(SHORTCUT_DEFINITIONS.map((definition) => [definition.id, definition]));
export const SHORTCUT_STORAGE_KEY = "grafium.shortcuts.v1";
const MAX_BINDINGS = 6;

export function shortcutDefinition(id: string): ShortcutDefinition | undefined {
  return DEFINITIONS.get(id);
}

// ─── Saved bindings ──────────────────────────────────────────────────────────

type Overrides = Record<string, string[]>;

let overrides: Overrides | null = null;
const listeners = new Set<() => void>();

function storage(): Storage | null {
  try {
    return typeof localStorage !== "undefined" ? localStorage : null;
  } catch {
    return null;
  }
}

function sameBindings(a: readonly string[], b: readonly string[]): boolean {
  return a.length === b.length && a.every((binding, index) => binding === b[index]);
}

function canonicalList(bindings: readonly string[]): string[] {
  const seen = new Set<string>();
  for (const binding of bindings) {
    const canonical = canonicalBinding(binding);
    if (canonical) seen.add(canonical);
  }
  return [...seen].slice(0, MAX_BINDINGS);
}

function loadOverrides(): Overrides {
  if (overrides) return overrides;
  overrides = {};
  try {
    const raw = storage()?.getItem(SHORTCUT_STORAGE_KEY);
    const parsed = raw ? JSON.parse(raw) : null;
    const saved = parsed && typeof parsed === "object" ? parsed.bindings : null;
    if (saved && typeof saved === "object") {
      for (const [id, value] of Object.entries(saved)) {
        const definition = DEFINITIONS.get(id);
        if (!definition || !Array.isArray(value)) continue;
        const valid = canonicalList(value.filter((binding): binding is string =>
          typeof binding === "string" && validateShortcutBinding(id, binding) === null));
        overrides[id] = valid;
      }
    }
  } catch {
    // Unreadable saved shortcuts fall back to the defaults.
  }
  return overrides;
}

function save(): void {
  const current = loadOverrides();
  const bindings: Overrides = {};
  for (const [id, list] of Object.entries(current)) {
    const definition = DEFINITIONS.get(id);
    if (definition && !sameBindings(list, defaultShortcutBindings(id))) bindings[id] = list;
  }
  try {
    storage()?.setItem(SHORTCUT_STORAGE_KEY, JSON.stringify({ version: 1, bindings }));
  } catch {
    // Keep working with this session's shortcuts when storage is unavailable.
  }
  for (const listener of listeners) listener();
}

/** Run `listener` whenever any shortcut changes. Returns an unsubscribe function. */
export function onShortcutsChanged(listener: () => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Forget cached bindings so the next read comes from storage. For tests. */
export function reloadShortcutsFromStorage(): void {
  overrides = null;
  for (const listener of listeners) listener();
}

export function defaultShortcutBindings(id: string): string[] {
  return canonicalList(DEFINITIONS.get(id)?.defaults ?? []);
}

/** The bindings in effect for an action, canonical. */
export function shortcutBindings(id: string): string[] {
  return loadOverrides()[id] ?? defaultShortcutBindings(id);
}

export function isShortcutCustomized(id: string): boolean {
  const saved = loadOverrides()[id];
  return saved !== undefined && !sameBindings(saved, defaultShortcutBindings(id));
}

export function setShortcutBindings(id: string, bindings: readonly string[]): void {
  if (!DEFINITIONS.has(id)) return;
  loadOverrides()[id] = canonicalList(bindings);
  save();
}

export interface ResetResult {
  /** Defaults left out because other actions use those keys now. */
  skipped: Array<{ binding: string; usedBy: ShortcutDefinition[] }>;
}

/**
 * Give an action back its default keys. A default that another action has
 * taken since stays with that action, so no key does two things; it is
 * listed in `skipped` for the caller to explain.
 */
export function resetShortcut(id: string): ResetResult {
  const current = loadOverrides();
  delete current[id];
  const kept: string[] = [];
  const skipped: ResetResult["skipped"] = [];
  for (const binding of defaultShortcutBindings(id)) {
    const usedBy = findShortcutConflicts(id, binding);
    if (usedBy.length) skipped.push({ binding, usedBy });
    else kept.push(binding);
  }
  if (skipped.length) current[id] = kept;
  save();
  return { skipped };
}

export function resetAllShortcuts(): void {
  overrides = {};
  save();
}

/** Whether a key event presses one of an action's single-press bindings. */
export function matchesShortcut(event: KeyboardEvent, id: string): boolean {
  const pressed = bindingFromEvent(event);
  return pressed !== null && shortcutBindings(id).includes(pressed);
}

// ─── Rules for new bindings ──────────────────────────────────────────────────

const PLAIN_SEQUENCE_KEY = /^[a-z0-9]$/;

/**
 * Why `binding` cannot be used for action `id`, or `null` when it can.
 * Messages are written to be shown as they are.
 */
export function validateShortcutBinding(id: string, binding: string): string | null {
  const definition = DEFINITIONS.get(id);
  if (!definition) return "Unknown shortcut.";
  const chords = bindingChords(binding);
  if (chords.length === 0 || chords.some((chord) => !chordKey(chord))) return "Press a key.";
  const canonical = chords.join(" ");
  // An action may always keep or get back its own defaults (flashcards use Enter).
  if (defaultShortcutBindings(id).includes(canonical)) return null;
  if (RESERVED[canonical]) return `That key ${RESERVED[canonical]}, so it can't be changed.`;
  if (chords.length > 1) {
    if (definition.singlePress) return "This shortcut works in the book reader too, so use a single combination.";
    if (definition.scope !== "app") return "Key sequences only work for app-wide shortcuts. Use a single combination.";
    if (chords.length > 3 || !chords.every((chord) => PLAIN_SEQUENCE_KEY.test(chord))) {
      return "A sequence is two or three plain letter or number keys, like g j.";
    }
    return null;
  }
  const [chord] = chords;
  if (chordHasCommandModifier(chord) || isFunctionKey(chordKey(chord))) return null;
  if (definition.scope === "flashcards") return null;
  return definition.scope === "editor" || definition.singlePress
    ? "Hold Ctrl, Alt or Cmd too, so typing still works."
    : "Hold Ctrl, Alt or Cmd too, or press two letters in a row like g j.";
}

const SCOPE_OVERLAP: Record<ShortcutScope, readonly ShortcutScope[]> = {
  app: ["app", "editor", "flashcards"],
  editor: ["app", "editor"],
  flashcards: ["app", "flashcards"],
};

function startsWith(longer: readonly string[], shorter: readonly string[]): boolean {
  return shorter.length <= longer.length && shorter.every((chord, index) => chord === longer[index]);
}

/**
 * Whether two bindings collide: the same keys, or a sequence and a key that
 * starts it (pressing the key would act before the sequence could finish).
 */
function overlaps(a: readonly string[], b: readonly string[]): boolean {
  return startsWith(a, b) || startsWith(b, a);
}

/** Every other action already using `binding` where it would also work for `id`. */
export function findShortcutConflicts(id: string, binding: string): ShortcutDefinition[] {
  const definition = DEFINITIONS.get(id);
  if (!definition) return [];
  const chords = bindingChords(binding);
  return SHORTCUT_DEFINITIONS.filter((other) =>
    other.id !== id
    && SCOPE_OVERLAP[definition.scope].includes(other.scope)
    && shortcutBindings(other.id).some((existing) => overlaps(bindingChords(existing), chords)));
}

export type AssignResult =
  | { ok: true }
  | { ok: false; error: string }
  | { ok: false; conflicts: ShortcutDefinition[]; binding: string };

/**
 * Add `binding` to action `id`. With `moveFromConflict`, every action that
 * had it loses it; without, the conflicts are returned for the caller to
 * confirm.
 */
export function addShortcutBinding(id: string, binding: string, moveFromConflict = false): AssignResult {
  const error = validateShortcutBinding(id, binding);
  if (error) return { ok: false, error };
  const canonical = canonicalBinding(binding);
  const own = shortcutBindings(id);
  if (own.includes(canonical)) return { ok: true };
  if (own.length >= MAX_BINDINGS) {
    return { ok: false, error: `An action can have up to ${MAX_BINDINGS} shortcuts.` };
  }
  const chords = bindingChords(canonical);
  const shadowed = own.find((existing) => overlaps(bindingChords(existing), chords));
  if (shadowed) return { ok: false, error: `That overlaps ${shadowed}, which this action already has. Remove it first.` };
  const conflicts = findShortcutConflicts(id, canonical);
  if (conflicts.length && !moveFromConflict) return { ok: false, conflicts, binding: canonical };
  const current = loadOverrides();
  for (const other of conflicts) {
    current[other.id] = shortcutBindings(other.id).filter((existing) => !overlaps(bindingChords(existing), chords));
  }
  current[id] = [...own, canonical];
  save();
  return { ok: true };
}

export function removeShortcutBinding(id: string, binding: string): void {
  const canonical = canonicalBinding(binding);
  setShortcutBindings(id, shortcutBindings(id).filter((existing) => existing !== canonical));
}
