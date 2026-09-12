/**
 * Dual-mode keyboard shortcut system (like org-style / Vim).
 *
 * - Navigation mode: active when no block is being edited.
 *   Keypresses trigger shortcuts (g j = go journal, etc.)
 * - Edit mode: active when a block's CodeMirror editor has focus.
 *   Keypresses go to the editor. Only Escape exits to nav mode.
 *
 * Supports chord sequences (e.g. "g j" = two keypresses in sequence).
 */

export type ActionFn = () => void;

export interface Shortcut {
  /** Groups aliases into one help row (e.g. "g j" and "mod+shift+j"). */
  id?: string;
  /** Binding string: "mod+k", "g j", "t t", etc. */
  binding: string;
  /** Action to perform */
  action: ActionFn;
  /** Only active in navigation mode (default true) */
  navOnly?: boolean;
  /** Description for help screen */
  description?: string;
  /** Category for grouping */
  category?: string;
}

// Normalize "mod" to platform-appropriate modifier
function modKey(): string {
  return navigator.platform.includes("Mac") ? "Meta" : "Control";
}

function normalizeKey(key: string): string {
  return key
    .replace(/mod/gi, modKey())
    .replace(/ctrl/gi, "Control")
    .replace(/alt/gi, "Alt")
    .replace(/shift/gi, "Shift")
    .replace(/meta/gi, "Meta");
}

function eventToKeyString(e: KeyboardEvent): string {
  const parts: string[] = [];
  if (e.ctrlKey) parts.push("Control");
  if (e.altKey) parts.push("Alt");
  if (e.shiftKey) parts.push("Shift");
  if (e.metaKey) parts.push("Meta");

  let key = e.key;
  // Physical keys so Ctrl-Shift-. matches Ctrl-> on US keyboards, and
  // Ctrl-Shift-C is KeyC even when the webview reports "C" or a translated key.
  if ((e.ctrlKey || e.metaKey || e.altKey) && /^Key[A-Z]$/.test(e.code)) {
    key = e.code.slice(3).toLowerCase();
  } else if (e.code === "Period") key = ".";
  else if (e.code === "Comma") key = ",";
  else if (e.code === "BracketLeft") key = "[";
  else if (e.code === "BracketRight") key = "]";
  if (key === " ") key = "Space";
  if (key.length === 1) key = key.toLowerCase();

  if (!["Control", "Alt", "Shift", "Meta"].includes(key)) {
    parts.push(key);
  }

  return parts.sort().join("+");
}

function parseBinding(binding: string): string[][] {
  // A binding can be a chord sequence: "g j" means press g, then j
  // Or a combo: "mod+k" means hold mod and press k
  const chords = binding.split(" ").map((chord) =>
    chord.split("+").map((k) => normalizeKey(k.trim()))
  );
  // Each chord is an array of keys that form a single keypress
  // Convert each chord to a single normalized string
  return chords.map((parts) => parts.sort());
}

function chordToString(parts: string[]): string {
  return [...parts].sort().join("+");
}

interface ParsedShortcut {
  /** Array of chord strings to match in sequence */
  sequence: string[];
  action: ActionFn;
  navOnly: boolean;
}

class KeymapManager {
  private shortcuts: ParsedShortcut[] = [];
  private registeredShortcuts: Shortcut[] = [];
  private pendingChord: string[] = [];
  private chordTimeout: number | null = null;
  private _editing = false;
  private listeners: Set<(editing: boolean) => void> = new Set();

  get isEditing(): boolean {
    return this._editing;
  }

  set isEditing(val: boolean) {
    this._editing = val;
    this.pendingChord = [];
    this.listeners.forEach((fn) => fn(val));
  }

  onModeChange(fn: (editing: boolean) => void): () => void {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  register(shortcuts: Shortcut[]) {
    this.registeredShortcuts = shortcuts;
    this.shortcuts = shortcuts.map((s) => {
      const chords = parseBinding(s.binding);
      return {
        sequence: chords.map((c) => chordToString(c)),
        action: s.action,
        navOnly: s.navOnly !== false,
      };
    });
  }

  getShortcuts(): Shortcut[] {
    return this.registeredShortcuts;
  }

  handleKeydown(e: KeyboardEvent): boolean {
    const target = e.target as HTMLElement | null;
    const inField =
      target?.tagName === "INPUT" ||
      target?.tagName === "TEXTAREA" ||
      target?.tagName === "SELECT" ||
      !!target?.isContentEditable ||
      !!target?.closest?.("[contenteditable='true'], [role='textbox']");
    const navBlocked = this._editing || inField;

    const keyStr = eventToKeyString(e);
    if (!keyStr || keyStr === "Shift" || keyStr === "Control" || keyStr === "Alt" || keyStr === "Meta") {
      return false;
    }

    const active = navBlocked
      ? this.shortcuts.filter((s) => !s.navOnly)
      : this.shortcuts;

    if (navBlocked) {
      this.pendingChord = [];
    }

    this.pendingChord.push(keyStr);

    if (this.chordTimeout !== null) {
      clearTimeout(this.chordTimeout);
    }

    const pending = [...this.pendingChord];
    const exactMatch = active.find(
      (s) =>
        s.sequence.length === pending.length &&
        s.sequence.every((chord, i) => chord === pending[i])
    );

    if (exactMatch) {
      e.preventDefault();
      e.stopPropagation();
      this.pendingChord = [];
      exactMatch.action();
      return true;
    }

    const prefixMatch = active.some(
      (s) =>
        s.sequence.length > pending.length &&
        pending.every((chord, i) => chord === s.sequence[i])
    );

    if (prefixMatch) {
      e.preventDefault();
      this.chordTimeout = window.setTimeout(() => {
        this.pendingChord = [];
      }, 1000);
      return true;
    }

    this.pendingChord = [];
    return false;
  }
}

// Singleton instance
export const keymap_manager = new KeymapManager();

/**
 * Register the default outline-style shortcuts.
 * Call this once at app startup, passing action callbacks.
 */
export function registerDefaultShortcuts(actions: {
  goJournal: () => void;
  goJournalEdit: () => void;
  goHome: () => void;
  goAllPages: () => void;
  goGraph: () => void;
  goFlashcards: () => void;
  goTomorrow: () => void;
  goTasks: () => void;
  goChat: () => void;
  goNextJournal: () => void;
  goPrevJournal: () => void;
  goForward: () => void;
  goBackward: () => void;
  search: () => void;
  searchInPage: () => void;
  focusLocalSearch: () => void;
  toggleSidebar: () => void;
  toggleRightSidebar: () => void;
  toggleTheme: () => void;
  toggleHelp?: () => void;
  toggleSettings: () => void;
  toggleWideMode: () => void;
  toggleZenMode: () => void;
  newPage?: () => void;
  reindex?: () => void;
  undo?: () => void;
  redo?: () => void;
  commandPalette: () => void;
  importMedia: () => void;
  importBooks: () => void;
  insertTimeStamp: () => void;
  insertPersonalJournal: () => void;
}) {
  const pair = (
    id: string,
    description: string,
    category: string,
    action: ActionFn,
    bindings: Array<{ binding: string; navOnly?: boolean }>,
  ): Shortcut[] =>
    bindings.map((b) => ({
      id,
      description,
      category,
      action,
      binding: b.binding,
      navOnly: b.navOnly ?? !b.binding.includes("+"),
    }));

  const shortcuts: Shortcut[] = [
    ...pair("go-journal", "Go to today's journal", "navigation", actions.goJournal, [
      { binding: "g j" },
    ]),
    ...pair("go-journal", "Go to today's journal", "navigation", actions.goJournalEdit, [
      { binding: "mod+shift+j", navOnly: false },
    ]),
    ...pair("go-home", "Go to home", "navigation", actions.goHome, [
      { binding: "g h" },
      { binding: "mod+shift+h", navOnly: false },
    ]),
    ...pair("go-all-pages", "Go to all pages", "navigation", actions.goAllPages, [
      { binding: "g a" },
      { binding: "mod+shift+a", navOnly: false },
    ]),
    ...pair("go-graph", "Go to graph view", "navigation", actions.goGraph, [
      { binding: "g g" },
      { binding: "mod+shift+g", navOnly: false },
    ]),
    ...pair("go-flashcards", "Go to flashcards", "navigation", actions.goFlashcards, [
      { binding: "g f" },
      { binding: "mod+shift+f", navOnly: false },
    ]),
    ...pair("go-tomorrow", "Go to tomorrow's journal page", "navigation", actions.goTomorrow, [
      { binding: "g t" },
    ]),
    ...pair("go-tasks", "Go to tasks", "navigation", actions.goTasks, [
      { binding: "mod+shift+t", navOnly: false },
    ]),
    ...pair("go-next-journal", "Go to next journal", "navigation", actions.goNextJournal, [
      { binding: "g n" },
      { binding: "mod+shift+.", navOnly: false },
    ]),
    ...pair("go-prev-journal", "Go to previous journal", "navigation", actions.goPrevJournal, [
      { binding: "g p" },
      { binding: "mod+shift+,", navOnly: false },
    ]),
    ...pair("go-backward", "Go backward", "navigation", actions.goBackward, [
      { binding: "mod+[", navOnly: false },
    ]),
    ...pair("go-forward", "Go forward", "navigation", actions.goForward, [
      { binding: "mod+]", navOnly: false },
    ]),
    ...pair("go-chat", "Go to Chat tab", "navigation", actions.goChat, [
      { binding: "mod+shift+c", navOnly: false },
    ]),

    ...pair("toggle-left-sidebar", "Toggle left sidebar", "toggle", actions.toggleSidebar, [
      { binding: "t l" },
      { binding: "mod+b", navOnly: false },
    ]),
    ...pair("toggle-right-sidebar", "Toggle right sidebar", "toggle", actions.toggleRightSidebar, [
      { binding: "t r" },
      { binding: "mod+alt+b", navOnly: false },
      { binding: "mod+.", navOnly: false },
    ]),
    ...pair("toggle-theme", "Open theme settings", "toggle", actions.toggleTheme, [
      { binding: "t t" },
    ]),
    ...pair("toggle-wide", "Toggle wide mode", "toggle", actions.toggleWideMode, [
      { binding: "t w" },
      { binding: "alt+w", navOnly: false },
    ]),
    ...pair("toggle-zen", "Toggle zen mode", "toggle", actions.toggleZenMode, [
      { binding: "t z" },
      { binding: "alt+z", navOnly: false },
    ]),
    ...pair("toggle-settings", "Toggle settings", "toggle", actions.toggleSettings, [
      { binding: "t s" },
      { binding: "alt+s", navOnly: false },
    ]),

    ...pair("search-global", "Global search", "search", actions.search, [
      { binding: "mod+k", navOnly: false },
    ]),
    ...pair("search-in-page", "Search in page", "search", actions.searchInPage, [
      { binding: "mod+shift+k", navOnly: false },
    ]),
    ...pair("search-local", "Focus page search", "search", actions.focusLocalSearch, [
      { binding: "mod+f", navOnly: false },
    ]),

    ...pair("command-palette", "Command palette", "basics", actions.commandPalette, [
      { binding: "mod+shift+p", navOnly: false },
    ]),
    ...pair("import-media", "Import media", "basics", actions.importMedia, [
      { binding: "alt+m", navOnly: false },
    ]),
    ...pair("import-books", "Import books", "basics", actions.importBooks, [
      { binding: "alt+b", navOnly: false },
    ]),
    ...pair("insert-time", "Insert current time", "basics", actions.insertTimeStamp, [
      { binding: "alt+t", navOnly: false },
    ]),
    ...pair("insert-personal-journal", "Insert [[personal/journal]]", "basics", actions.insertPersonalJournal, [
      { binding: "alt+j", navOnly: false },
    ]),
  ];

  keymap_manager.register(shortcuts);
}
