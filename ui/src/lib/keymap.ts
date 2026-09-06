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
  // Normalize some key names
  if (key === " ") key = "Space";
  if (key.length === 1) key = key.toLowerCase();

  // Don't add modifier keys as the main key
  if (!["Control", "Alt", "Shift", "Meta"].includes(key)) {
    parts.push(key);
  }

  return parts.join("+");
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
    // Skip if target is an input/textarea (native ones, not CodeMirror)
    const target = e.target as HTMLElement;
    if (
      target.tagName === "INPUT" ||
      target.tagName === "TEXTAREA" ||
      target.tagName === "SELECT"
    ) {
      return false;
    }

    // In edit mode, only process shortcuts marked as not navOnly
    if (this._editing) {
      return false;
    }

    const keyStr = eventToKeyString(e);
    if (!keyStr || keyStr === "Shift" || keyStr === "Control" || keyStr === "Alt" || keyStr === "Meta") {
      return false;
    }

    // Build the current chord sequence
    this.pendingChord.push(keyStr);

    if (this.chordTimeout !== null) {
      clearTimeout(this.chordTimeout);
    }

    // Check for matches
    const pending = [...this.pendingChord];
    const exactMatch = this.shortcuts.find(
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

    // Check if any shortcut starts with this prefix (potential chord in progress)
    const prefixMatch = this.shortcuts.some(
      (s) =>
        s.sequence.length > pending.length &&
        pending.every((chord, i) => chord === s.sequence[i])
    );

    if (prefixMatch) {
      e.preventDefault();
      // Wait for next key in chord
      this.chordTimeout = window.setTimeout(() => {
        this.pendingChord = [];
      }, 1000);
      return true;
    }

    // No match at all - reset
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
  goHome: () => void;
  goAllPages: () => void;
  goGraph: () => void;
  goFlashcards: () => void;
  goTomorrow: () => void;
  goNextJournal: () => void;
  goPrevJournal: () => void;
  goForward: () => void;
  goBackward: () => void;
  search: () => void;
  searchInPage: () => void;
  toggleSidebar: () => void;
  toggleRightSidebar: () => void;
  toggleTheme: () => void;
  toggleHelp: () => void;
  toggleSettings: () => void;
  toggleWideMode: () => void;
  toggleZenMode: () => void;
  newPage: () => void;
  reindex: () => void;
  undo: () => void;
  redo: () => void;
  commandPalette: () => void;
}) {
  const shortcuts: Shortcut[] = [
    // ─── Navigation (g prefix) ─────────────────────
    // On Linux, these are also handled at the GTK/Rust level via __chordActions
    // because WebKitGTK JS keydown delivery can be unreliable.
    {
      binding: "g j",
      action: actions.goJournal,
      category: "navigation",
      description: "Go to today's journal",
    },
    {
      // Editor-safe variant of "g j": Ctrl+Shift+J works even while
      // typing in a block. The always-on delivery lives in
      // App.svelte's handleGlobalKeydown; this entry only exists so
      // a future help/command-palette listing shows it.
      binding: "mod+shift+j",
      action: actions.goJournal,
      navOnly: false,
      category: "navigation",
      description: "Go to today's journal (works while editing)",
    },
    {
      binding: "g h",
      action: actions.goHome,
      category: "navigation",
      description: "Go to home",
    },
    {
      binding: "g a",
      action: actions.goAllPages,
      category: "navigation",
      description: "Go to all pages",
    },
    {
      binding: "g g",
      action: actions.goGraph,
      category: "navigation",
      description: "Go to graph view",
    },
    {
      binding: "g f",
      action: actions.goFlashcards,
      category: "navigation",
      description: "Go to flashcards",
    },
    {
      binding: "g t",
      action: actions.goTomorrow,
      category: "navigation",
      description: "Go to tomorrow",
    },
    {
      binding: "g n",
      action: actions.goNextJournal,
      category: "navigation",
      description: "Go to next journal",
    },
    {
      binding: "g p",
      action: actions.goPrevJournal,
      category: "navigation",
      description: "Go to previous journal",
    },

    // ─── Toggle (t prefix) ─────────────────────────
    {
      binding: "t l",
      action: actions.toggleSidebar,
      category: "toggle",
      description: "Toggle left sidebar",
    },
    {
      binding: "t r",
      action: actions.toggleRightSidebar,
      category: "toggle",
      description: "Toggle right sidebar",
    },
    {
      binding: "t t",
      action: actions.toggleTheme,
      category: "toggle",
      description: "Toggle theme",
    },
    {
      binding: "t w",
      action: actions.toggleWideMode,
      category: "toggle",
      description: "Toggle wide mode",
    },
    {
      binding: "t z",
      action: actions.toggleZenMode,
      category: "toggle",
      description: "Toggle zen mode",
    },
    {
      binding: "t s",
      action: actions.toggleSettings,
      category: "toggle",
      description: "Toggle settings",
    },

    // ─── Global with modifiers ─────────────────────
    {
      binding: "mod+k",
      action: actions.search,
      category: "search",
      description: "Global search",
    },
    {
      binding: "mod+shift+k",
      action: actions.searchInPage,
      category: "search",
      description: "Search in page",
    },
    {
      binding: "mod+shift+p",
      action: actions.commandPalette,
      category: "basics",
      description: "Command palette",
    },
    // Disabled due conflict with slash command entry on some keyboard layouts
    // where '/' requires Shift and could interfere with editor slash menu.

    // ─── History navigation ────────────────────────
    {
      binding: "mod+[",
      action: actions.goBackward,
      category: "navigation",
      description: "Go backward",
    },
    {
      binding: "mod+]",
      action: actions.goForward,
      category: "navigation",
      description: "Go forward",
    },

    // ─── Familiar aliases (VS Code / Obsidian / Cursor muscle memory) ─────────
    {
      binding: "mod+b",
      action: actions.toggleSidebar,
      category: "toggle",
      description: "Toggle left sidebar",
    },
    {
      binding: "mod+shift+a",
      action: actions.toggleRightSidebar,
      category: "toggle",
      description: "Toggle Knowledge Panel",
    },
  ];

  keymap_manager.register(shortcuts);
}
