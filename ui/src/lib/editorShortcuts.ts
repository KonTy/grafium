import { Compartment, type Extension } from "@codemirror/state";
import { keymap, type EditorView, type KeyBinding } from "@codemirror/view";
import { bindingChords, canonicalBinding } from "./keyBinding";
import { defaultShortcutBindings, onShortcutsChanged, shortcutBindings } from "./shortcutRegistry";

export type EditorShortcutId =
  | "editor-bold"
  | "editor-italic"
  | "editor-strikethrough"
  | "editor-save-source";

const EDITOR_SHORTCUT_IDS: readonly EditorShortcutId[] = [
  "editor-bold", "editor-italic", "editor-strikethrough", "editor-save-source",
];

export type EditorShortcutHandlers = Partial<Record<EditorShortcutId, (view: EditorView) => boolean>>;

const MODIFIERS: Record<string, string> = { mod: "Mod", ctrl: "Ctrl", meta: "Meta", alt: "Alt", shift: "Shift" };
const KEY_NAMES: Record<string, string> = {
  escape: "Escape", enter: "Enter", space: "Space", tab: "Tab", backspace: "Backspace",
  delete: "Delete", arrowup: "ArrowUp", arrowdown: "ArrowDown", arrowleft: "ArrowLeft",
  arrowright: "ArrowRight", home: "Home", end: "End", pageup: "PageUp", pagedown: "PageDown",
  plus: "+",
};

/** A binding in CodeMirror's spelling (`mod+shift+z` -> `Mod-Shift-z`), or null for sequences. */
export function codeMirrorKey(binding: string): string | null {
  const chords = bindingChords(binding);
  if (chords.length !== 1) return null;
  const parts = chords[0].split("+");
  const key = parts.pop() ?? "";
  if (!key) return null;
  const name = KEY_NAMES[key] ?? (/^f\d{1,2}$/.test(key) ? key.toUpperCase() : key);
  return [...parts.map((part) => MODIFIERS[part] ?? part), name].join("-");
}

export function editorShortcutKeymap(handlers: EditorShortcutHandlers): KeyBinding[] {
  const bindings: KeyBinding[] = [];
  for (const [id, run] of Object.entries(handlers) as Array<[EditorShortcutId, (view: EditorView) => boolean]>) {
    if (!run) continue;
    for (const binding of shortcutBindings(id)) {
      const key = codeMirrorKey(binding);
      if (key) bindings.push({ key, run, preventDefault: true });
    }
  }
  return bindings;
}

const CODEMIRROR_MODIFIERS: Array<[RegExp, string]> = [
  [/^(cmd|meta|m)$/i, "meta"],
  [/^a(lt)?$/i, "alt"],
  [/^(c|ctrl|control)$/i, "ctrl"],
  [/^s(hift)?$/i, "shift"],
  [/^mod$/i, "mod"],
];

/**
 * A key in CodeMirror's spelling (`Shift-Mod-k`, `Ctrl-Shift-z`) as a
 * canonical binding (`mod+shift+k`), whatever order the modifiers come in.
 */
export function bindingFromCodeMirrorKey(key: string): string {
  // As CodeMirror reads it: a trailing `-` is the minus key, not a separator.
  const parts = key.split(/-(?!$)/);
  const name = parts.pop() ?? "";
  const modifiers = parts.map((part) => CODEMIRROR_MODIFIERS.find(([pattern]) => pattern.test(part))?.[1] ?? part);
  const keyName = name === "+" ? "plus" : name === " " ? "space" : name;
  return canonicalBinding([...modifiers, keyName].join("+"));
}

/** The key a CodeMirror binding uses on this platform, picked as CodeMirror does. */
function platformKey(binding: KeyBinding): string | undefined {
  const platform = typeof navigator === "undefined" ? null
    : /Mac/.test(navigator.platform) ? "mac"
    : /Win/.test(navigator.platform) ? "win"
    : /Linux|X11/.test(navigator.platform) ? "linux" : null;
  return (platform && binding[platform]) || binding.key;
}

/**
 * CodeMirror's built-in keys that are also Grafium's default editor
 * shortcuts are dropped from its keymaps, so a shortcut moved elsewhere in
 * Settings stops working here instead of falling back to the built-in
 * command (Ctrl-Shift-K would otherwise delete the line).
 */
export function withoutShortcutDefaults(bindings: readonly KeyBinding[]): KeyBinding[] {
  const defaults = new Set(EDITOR_SHORTCUT_IDS.flatMap((id) => defaultShortcutBindings(id)));
  return bindings.filter((binding) => {
    const key = platformKey(binding);
    return !key || !defaults.has(bindingFromCodeMirrorKey(key));
  });
}

/**
 * Editor shortcuts that follow Settings. Add `extension` to the editor state
 * and call `attach` once the view exists; the returned function stops
 * following when the view is destroyed.
 */
export function followEditorShortcuts(handlers: EditorShortcutHandlers): {
  extension: Extension;
  attach: (view: EditorView) => () => void;
} {
  const compartment = new Compartment();
  const build = () => keymap.of(editorShortcutKeymap(handlers));
  return {
    extension: compartment.of(build()),
    attach(view) {
      // A cached editor state may predate the latest change.
      view.dispatch({ effects: compartment.reconfigure(build()) });
      return onShortcutsChanged(() => view.dispatch({ effects: compartment.reconfigure(build()) }));
    },
  };
}
