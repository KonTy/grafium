import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { bindingFromEvent, canonicalBinding, eventMatchesBinding } from "./keyBinding";
import {
  addShortcutBinding,
  findShortcutConflicts,
  isShortcutCustomized,
  matchesShortcut,
  onShortcutsChanged,
  reloadShortcutsFromStorage,
  removeShortcutBinding,
  resetAllShortcuts,
  resetShortcut,
  SHORTCUT_DEFINITIONS,
  SHORTCUT_SECTIONS,
  SHORTCUT_STORAGE_KEY,
  shortcutBindings,
  validateShortcutBinding,
} from "./shortcutRegistry";
import { defaultKeymap, deleteLine, historyKeymap } from "@codemirror/commands";
import { bindingFromCodeMirrorKey, codeMirrorKey, editorShortcutKeymap, withoutShortcutDefaults } from "./editorShortcuts";
import { keymap_manager, registerDefaultShortcuts } from "./keymap";
import { readerShortcut } from "./readerHotkeys";

function key(init: KeyboardEventInit): KeyboardEvent {
  return new KeyboardEvent("keydown", { bubbles: true, cancelable: true, ...init });
}

function memoryStorage(): Storage {
  const data = new Map<string, string>();
  return {
    get length() { return data.size; },
    clear: () => data.clear(),
    getItem: (name) => data.get(name) ?? null,
    key: (index) => [...data.keys()][index] ?? null,
    removeItem: (name) => { data.delete(name); },
    setItem: (name, value) => { data.set(name, String(value)); },
  };
}

beforeEach(() => {
  vi.stubGlobal("localStorage", memoryStorage());
  vi.spyOn(navigator, "platform", "get").mockReturnValue("Linux x86_64");
  reloadShortcutsFromStorage();
});

afterEach(() => {
  resetAllShortcuts();
  keymap_manager.register([]);
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe("canonical bindings", () => {
  it("spells the same keys one way, whatever order or alias was written", () => {
    expect(canonicalBinding("Shift+Mod+J")).toBe("mod+shift+j");
    expect(canonicalBinding("control+K")).toBe("mod+k");
    expect(canonicalBinding("alt+shift+mod+B")).toBe("mod+alt+shift+b");
    expect(canonicalBinding("g   j")).toBe("g j");
    expect(canonicalBinding("F1")).toBe("f1");
    expect(canonicalBinding("meta+k", true)).toBe("mod+k");
  });

  it("reads physical keys with modifiers held and refuses AltGr", () => {
    expect(bindingFromEvent(key({ key: "J", code: "KeyJ", ctrlKey: true, shiftKey: true }))).toBe("mod+shift+j");
    expect(bindingFromEvent(key({ key: ">", code: "Period", ctrlKey: true, shiftKey: true }))).toBe("mod+shift+.");
    expect(bindingFromEvent(key({ key: "+", code: "Equal", ctrlKey: true, shiftKey: true }))).toBe("mod+plus");
    // German layout: + and - sit where US keyboards have ] and /, and must
    // still zoom instead of going forward or matching Ctrl-/.
    expect(bindingFromEvent(key({ key: "+", code: "BracketRight", ctrlKey: true }))).toBe("mod+plus");
    expect(bindingFromEvent(key({ key: "-", code: "Slash", ctrlKey: true }))).toBe("mod+-");
    expect(bindingFromEvent(key({ key: "]", code: "BracketRight", ctrlKey: true }))).toBe("mod+]");
    expect(bindingFromEvent(key({ key: "?", code: "Slash", ctrlKey: true, shiftKey: true }))).toBe("mod+?");
    expect(canonicalBinding("mod+shift+?")).toBe("mod+?");
    expect(bindingFromEvent(key({ key: "ç", code: "KeyC", altKey: true }))).toBe("alt+c");
    expect(bindingFromEvent(key({ key: " ", code: "Space" }))).toBe("space");
    expect(bindingFromEvent(key({ key: "Shift", code: "ShiftLeft", shiftKey: true }))).toBeNull();
    const altGraph = key({ key: "@", code: "KeyQ", ctrlKey: true, altKey: true });
    vi.spyOn(altGraph, "getModifierState").mockImplementation((name) => name === "AltGraph");
    expect(bindingFromEvent(altGraph)).toBeNull();
    expect(eventMatchesBinding(key({ key: "k", code: "KeyK", ctrlKey: true }), "Ctrl+K")).toBe(true);
  });

  it("reads keys the way other keyboard layouts type them", () => {
    // AZERTY: the digit row types punctuation, and `-` is on the 6 key.
    expect(bindingFromEvent(key({ key: "-", code: "Digit6", ctrlKey: true }))).toBe("mod+-");
    expect(bindingFromEvent(key({ key: "à", code: "Digit0", ctrlKey: true }))).toBe("mod+0");
    expect(bindingFromEvent(key({ key: "1", code: "Digit1", ctrlKey: true }))).toBe("mod+1");
    // Option on macOS types symbols; the position still names the key.
    expect(bindingFromEvent(key({ key: "¡", code: "Digit1", altKey: true }), true)).toBe("alt+1");
    // A dead key types nothing alone, but with Ctrl held it is a shortcut key.
    expect(bindingFromEvent(key({ key: "Dead", code: "BracketLeft", ctrlKey: true }))).toBe("mod+[");
    expect(bindingFromEvent(key({ key: "Dead", code: "BracketLeft" }))).toBeNull();
    expect(bindingFromEvent(key({ key: "Dead", code: "Equal", ctrlKey: true }))).toBeNull();
  });
});

describe("shortcut catalog", () => {
  it("files every action under a known section with valid, unique ids", () => {
    const sections = new Set(SHORTCUT_SECTIONS.map(({ id }) => id));
    const ids = SHORTCUT_DEFINITIONS.map(({ id }) => id);
    expect(new Set(ids).size).toBe(ids.length);
    for (const definition of SHORTCUT_DEFINITIONS) {
      expect(sections.has(definition.section)).toBe(true);
      for (const binding of definition.defaults) expect(validateShortcutBinding(definition.id, binding)).toBeNull();
    }
  });

  it("ships without any conflicting defaults", () => {
    for (const definition of SHORTCUT_DEFINITIONS) {
      for (const binding of shortcutBindings(definition.id)) {
        expect(findShortcutConflicts(definition.id, binding).map(({ id }) => id), `${definition.id} ${binding}`).toEqual([]);
      }
    }
  });
});

describe("changing shortcuts", () => {
  it("rejects typing keys and keys that need a modifier", () => {
    expect(validateShortcutBinding("go-graph", "enter")).toMatch(/can't be changed/);
    expect(validateShortcutBinding("go-graph", "mod+c")).toMatch(/copies/);
    expect(validateShortcutBinding("editor-bold", "b")).toMatch(/Ctrl, Alt or Cmd/);
    expect(validateShortcutBinding("editor-bold", "g j")).toMatch(/single combination/);
    expect(validateShortcutBinding("go-graph", "g")).toMatch(/two letters/);
    expect(validateShortcutBinding("go-graph", "g x")).toBeNull();
    expect(validateShortcutBinding("flashcards-again", "a")).toBeNull();
    expect(validateShortcutBinding("flashcards-reveal", "enter")).toBeNull();
    // The window handles undo and redo before the page on Linux.
    expect(validateShortcutBinding("go-graph", "mod+z")).toMatch(/undoes/);
    expect(validateShortcutBinding("editor-bold", "mod+y")).toMatch(/redoes/);
    expect(validateShortcutBinding("go-graph", "mod+shift+z")).toMatch(/redoes/);
    // The reader runs Bionic and bookmark keys too, and only knows single presses.
    expect(validateShortcutBinding("bookmark", "b m")).toMatch(/single combination/);
    expect(validateShortcutBinding("bookmark", "b")).toMatch(/Ctrl, Alt or Cmd/);
    expect(validateShortcutBinding("bookmark", "f4")).toBeNull();
  });

  it("warns about a key in use and moves it only when asked", () => {
    const first = addShortcutBinding("go-graph", "mod+k");
    expect(first).toMatchObject({ ok: false, conflicts: [{ id: "search-global" }] });
    expect(shortcutBindings("search-global")).toContain("mod+k");
    expect(addShortcutBinding("go-graph", "mod+k", true)).toEqual({ ok: true });
    expect(shortcutBindings("go-graph")).toContain("mod+k");
    expect(shortcutBindings("search-global")).not.toContain("mod+k");
    expect(isShortcutCustomized("search-global")).toBe(true);
  });

  it("treats a sequence and the key that starts it as a conflict, and moves the key from every action", () => {
    const result = addShortcutBinding("flashcards-again", "g");
    expect(result.ok).toBe(false);
    const conflicts = "conflicts" in result ? result.conflicts.map(({ id }) => id) : [];
    expect(conflicts).toEqual(expect.arrayContaining(["go-journal", "go-home", "go-graph", "go-library"]));
    expect(addShortcutBinding("flashcards-again", "g", true)).toEqual({ ok: true });
    expect(shortcutBindings("go-journal")).toEqual(["mod+shift+j"]);
    expect(shortcutBindings("go-library")).toEqual([]);
    expect(findShortcutConflicts("flashcards-again", "g")).toEqual([]);
    // Editor and flashcard shortcuts never meet, so they can share keys.
    expect(findShortcutConflicts("flashcards-again", "mod+i")).toEqual([]);
  });

  it("refuses a sequence that another sequence of the same action would cut short", () => {
    expect(addShortcutBinding("go-graph", "g g x")).toMatchObject({ ok: false, error: expect.stringMatching(/overlaps g g/) });
  });

  it("resets an action without taking back keys another action uses now", () => {
    addShortcutBinding("go-graph", "mod+k", true);
    const { skipped } = resetShortcut("search-global");
    expect(skipped.map(({ binding, usedBy }) => [binding, usedBy.map(({ id }) => id)])).toEqual([["mod+k", ["go-graph"]]]);
    expect(shortcutBindings("search-global")).toEqual([]);
    expect(isShortcutCustomized("search-global")).toBe(true);
    removeShortcutBinding("go-graph", "mod+k");
    expect(resetShortcut("search-global").skipped).toEqual([]);
    expect(shortcutBindings("search-global")).toEqual(["mod+k"]);
    expect(isShortcutCustomized("search-global")).toBe(false);
  });

  it("saves on this device, survives a reload, and resets", () => {
    const changes = vi.fn();
    const stop = onShortcutsChanged(changes);
    addShortcutBinding("insert-time", "mod+alt+t");
    removeShortcutBinding("insert-time", "alt+t");
    expect(shortcutBindings("insert-time")).toEqual(["mod+alt+t"]);
    expect(JSON.parse(localStorage.getItem(SHORTCUT_STORAGE_KEY)!)).toEqual({
      version: 1, bindings: { "insert-time": ["mod+alt+t"] },
    });
    reloadShortcutsFromStorage();
    expect(shortcutBindings("insert-time")).toEqual(["mod+alt+t"]);
    resetShortcut("insert-time");
    expect(shortcutBindings("insert-time")).toEqual(["alt+t"]);
    expect(changes).toHaveBeenCalled();
    stop();
  });

  it("ignores saved shortcuts that are unknown or no longer allowed", () => {
    localStorage.setItem(SHORTCUT_STORAGE_KEY, JSON.stringify({
      version: 1, bindings: { "go-graph": ["enter", "mod+alt+g"], gone: ["mod+q"], "go-home": "bad" },
    }));
    reloadShortcutsFromStorage();
    expect(shortcutBindings("go-graph")).toEqual(["mod+alt+g"]);
    expect(shortcutBindings("go-home")).toEqual(["g h", "mod+shift+h"]);
  });
});

describe("shortcuts follow Settings everywhere", () => {
  it("re-registers app shortcuts as soon as a binding changes", () => {
    const goGraph = vi.fn();
    const actions = new Proxy({}, { get: (_target, name) => (name === "goGraph" ? goGraph : vi.fn()) });
    registerDefaultShortcuts(actions as Parameters<typeof registerDefaultShortcuts>[0]);
    addShortcutBinding("go-graph", "mod+alt+g");
    keymap_manager.handleKeydown(key({ key: "g", code: "KeyG", ctrlKey: true, altKey: true }));
    expect(goGraph).toHaveBeenCalledTimes(1);
    removeShortcutBinding("go-graph", "mod+shift+g");
    expect(keymap_manager.handleKeydown(key({ key: "G", code: "KeyG", ctrlKey: true, shiftKey: true }))).toBe(false);
  });

  it("moves the reader hotkeys and flashcard keys too", () => {
    registerDefaultShortcuts(new Proxy({}, { get: () => vi.fn() }) as Parameters<typeof registerDefaultShortcuts>[0]);
    addShortcutBinding("bookmark", "mod+alt+k");
    expect(readerShortcut(key({ key: "k", code: "KeyK", ctrlKey: true, altKey: true }))).toBe("bookmark");
    addShortcutBinding("toggle-bionic", "f4");
    expect(readerShortcut(key({ key: "F4", code: "F4" }))).toBe("toggle-bionic");
    expect(readerShortcut(key({ key: "k", code: "KeyK" }))).toBeNull();
    addShortcutBinding("flashcards-good", "g x".split(" ")[1]);
    expect(matchesShortcut(key({ key: "x", code: "KeyX" }), "flashcards-good")).toBe(true);
  });

  it("builds the editor keymap from Settings and drops CodeMirror's same defaults", () => {
    expect(codeMirrorKey("mod+shift+z")).toBe("Mod-Shift-z");
    expect(codeMirrorKey("mod+alt+shift+b")).toBe("Mod-Alt-Shift-b");
    expect(codeMirrorKey("mod+-")).toBe("Mod--");
    expect(codeMirrorKey("g j")).toBeNull();
    const italic = () => true;
    addShortcutBinding("editor-italic", "mod+u");
    expect(editorShortcutKeymap({ "editor-italic": italic }).map((binding) => binding.key)).toEqual(["Mod-i", "Mod-u"]);
    // CodeMirror writes modifiers in any order.
    expect(bindingFromCodeMirrorKey("Shift-Mod-k")).toBe("mod+shift+k");
    expect(bindingFromCodeMirrorKey("Ctrl-Shift-z")).toBe("mod+shift+z");
    expect(bindingFromCodeMirrorKey("Mod--")).toBe("mod+-");
    expect(bindingFromCodeMirrorKey("a-Shift-B")).toBe("alt+shift+b");
    expect(withoutShortcutDefaults([
      { key: "Mod-z" }, { key: "Shift-Mod-k" }, { key: "Mod-i" }, { key: "Alt-Shift-Mod-b" }, { key: "Mod-s" }, { key: "Mod-u" },
    ]).map((binding) => binding.key)).toEqual(["Mod-z", "Mod-u"]);
    // Moving Strikethrough off Ctrl-Shift-K must not leave CodeMirror's delete-line behind.
    expect(defaultKeymap.some((binding) => binding.run === deleteLine)).toBe(true);
    expect(withoutShortcutDefaults(defaultKeymap).some((binding) => binding.run === deleteLine)).toBe(false);
    // Undo and redo are built in and stay.
    expect(withoutShortcutDefaults(historyKeymap)).toHaveLength(historyKeymap.length);
  });
});
