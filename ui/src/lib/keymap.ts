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

import { bindingChords, bindingFromEvent, chordHasCommandModifier, chordKey, isFunctionKey } from "./keyBinding";
import { readerShortcut, setReaderShortcutBindings } from "./readerHotkeys";
import { onShortcutsChanged, SHORTCUT_DEFINITIONS, shortcutBindings, type ShortcutSection } from "./shortcutRegistry";

/** Return `false` to leave the key alone (for example, nothing to focus). */
export type ActionFn = () => void | boolean;

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

interface ParsedShortcut {
  id?: string;
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
    this.shortcuts = shortcuts.map((s) => ({
      id: s.id,
      sequence: bindingChords(s.binding),
      action: s.action,
      navOnly: s.navOnly !== false,
    }));
  }

  getShortcuts(): Shortcut[] {
    return this.registeredShortcuts;
  }

  handleKeydown(e: KeyboardEvent): boolean {
    if (e.defaultPrevented || e.isComposing || e.keyCode === 229) {
      this.pendingChord = [];
      return false;
    }
    const target = e.target as HTMLElement | null;
    const inField =
      target?.tagName === "INPUT" ||
      target?.tagName === "TEXTAREA" ||
      target?.tagName === "SELECT" ||
      !!target?.isContentEditable ||
      !!target?.closest?.("[contenteditable='true'], [role='textbox']");
    const navBlocked = this._editing || inField;

    const keyStr = bindingFromEvent(e);
    if (!keyStr) return false;

    const available = navBlocked
      ? this.shortcuts.filter((s) => !s.navOnly)
      : this.shortcuts;
    const active = available.filter((s) =>
      (s.id !== "toggle-bionic" && s.id !== "bookmark") || readerShortcut(e) === s.id);

    if (navBlocked || readerShortcut(e)) {
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
      this.pendingChord = [];
      if (exactMatch.action() === false) return false;
      e.preventDefault();
      e.stopPropagation();
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

export interface ShortcutActions {
  goJournal: () => void;
  goJournalDate: () => void;
  goLink: () => void;
  goJournalEdit: () => void;
  goHome: () => void;
  goAllPages: () => void;
  goGraph: () => void;
  goFlashcards: () => void;
  goTomorrow: () => void;
  goTasks: () => void;
  goStudies?: () => void;
  goLibrary?: () => void;
  goChat: () => void;
  goNextJournal: () => void;
  goPrevJournal: () => void;
  goForward: () => void;
  goBackward: () => void;
  search: () => void;
  focusLocalSearch: () => void | boolean;
  toggleSidebar: () => void;
  toggleRightSidebar: () => void;
  toggleTheme: () => void;
  toggleHelp?: () => void;
  toggleSettings: () => void;
  toggleWideMode: () => void;
  toggleZenMode: () => void;
  toggleBionicReader?: () => void;
  bookmark?: () => void;
  newPage?: () => void;
  reindex?: () => void;
  undo?: () => void;
  redo?: () => void;
  commandPalette: () => void;
  importMedia: () => void;
  importBooks: () => void;
  insertTimeStamp: () => void;
  insertPersonalDiary: () => void;
  zoomIn?: () => void;
  zoomOut?: () => void;
  zoomReset?: () => void;
}

function shortcutHandlers(actions: ShortcutActions): Record<string, ActionFn | undefined> {
  return {
    "toggle-bionic": actions.toggleBionicReader,
    bookmark: actions.bookmark,
    help: actions.toggleHelp,
    "go-journal": actions.goJournal,
    "go-journal-date": actions.goJournalDate,
    "go-link": actions.goLink,
    "go-home": actions.goHome,
    "go-all-pages": actions.goAllPages,
    "go-graph": actions.goGraph,
    "go-flashcards": actions.goFlashcards,
    "go-tomorrow": actions.goTomorrow,
    "go-tasks": actions.goTasks,
    "go-studies": actions.goStudies,
    "go-library": actions.goLibrary,
    "go-next-journal": actions.goNextJournal,
    "go-prev-journal": actions.goPrevJournal,
    "go-backward": actions.goBackward,
    "go-forward": actions.goForward,
    "go-chat": actions.goChat,
    "toggle-left-sidebar": actions.toggleSidebar,
    "toggle-right-sidebar": actions.toggleRightSidebar,
    "toggle-theme": actions.toggleTheme,
    "toggle-wide": actions.toggleWideMode,
    "toggle-zen": actions.toggleZenMode,
    "toggle-settings": actions.toggleSettings,
    "search-global": actions.search,
    "search-local": actions.focusLocalSearch,
    "command-palette": actions.commandPalette,
    "import-media": actions.importMedia,
    "import-books": actions.importBooks,
    "insert-time": actions.insertTimeStamp,
    "insert-personal-diary": actions.insertPersonalDiary,
    "zoom-in": actions.zoomIn,
    "zoom-out": actions.zoomOut,
    "zoom-reset": actions.zoomReset,
  };
}

/**
 * Sequences (`g j`) and lone plain keys only act outside text editing;
 * anything holding Ctrl, Cmd or Alt, and function keys, act everywhere.
 */
function navOnlyBinding(binding: string): boolean {
  const chords = bindingChords(binding);
  if (chords.length !== 1) return true;
  return !chordHasCommandModifier(chords[0]) && !isFunctionKey(chordKey(chords[0]));
}

let registeredActions: ShortcutActions | null = null;
let stopFollowingSettings: (() => void) | null = null;

function applyShortcuts(): void {
  setReaderShortcutBindings({
    "toggle-bionic": shortcutBindings("toggle-bionic"),
    bookmark: shortcutBindings("bookmark"),
  });
  const actions = registeredActions;
  if (!actions) return;
  const handlers = shortcutHandlers(actions);
  const shortcuts: Shortcut[] = [];
  for (const definition of SHORTCUT_DEFINITIONS) {
    if (definition.scope !== "app") continue;
    const handler = handlers[definition.id];
    if (!handler) continue;
    for (const binding of shortcutBindings(definition.id)) {
      const navOnly = navOnlyBinding(binding);
      shortcuts.push({
        id: definition.id,
        description: definition.label,
        category: definition.section,
        binding,
        // The combo works while editing, so it also starts editing today.
        action: definition.id === "go-journal" && !navOnly ? actions.goJournalEdit : handler,
        navOnly,
      });
    }
  }
  keymap_manager.register(shortcuts);
}

/**
 * Register the app-wide shortcuts. Call once at startup with the action
 * callbacks; the bindings come from Settings and update when they change.
 */
export function registerDefaultShortcuts(actions: ShortcutActions) {
  registeredActions = actions;
  applyShortcuts();
  stopFollowingSettings ??= onShortcutsChanged(applyShortcuts);
}

export interface AppCommand {
  id: string;
  label: string;
  section: ShortcutSection;
  /** Current keys, possibly none. */
  bindings: string[];
  run: ActionFn;
}

/**
 * Every app-wide action that can run, with its current keys, for the command
 * palette. Actions without any keys are listed too, so removing a shortcut
 * never hides the command.
 */
export function appCommands(): AppCommand[] {
  if (!registeredActions) return [];
  const handlers = shortcutHandlers(registeredActions);
  const commands: AppCommand[] = [];
  for (const definition of SHORTCUT_DEFINITIONS) {
    const run = handlers[definition.id];
    if (definition.scope !== "app" || !run || definition.id === "command-palette") continue;
    commands.push({
      id: definition.id,
      label: definition.label,
      section: definition.section,
      bindings: shortcutBindings(definition.id),
      run,
    });
  }
  return commands;
}
