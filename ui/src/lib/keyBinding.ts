/**
 * One spelling for every keyboard shortcut.
 *
 * Bindings are strings: `+` joins the keys pressed together and a space
 * separates the presses of a sequence (`"mod+shift+j"`, `"g j"`). `mod` is the
 * platform's command key: Ctrl, or Cmd on macOS. Everything that stores,
 * compares, records or matches shortcuts goes through this module, so a
 * binding written by hand, recorded in Settings or read from a key event
 * always compares equal when it means the same keys.
 *
 * Pure and dependency-free: the isolated book reader imports it too.
 */

const MODIFIER_ORDER = ["mod", "ctrl", "meta", "alt", "shift"] as const;
type Modifier = (typeof MODIFIER_ORDER)[number];

const ALIASES: Record<string, string> = {
  control: "ctrl",
  cmd: "meta",
  command: "meta",
  super: "meta",
  win: "meta",
  option: "alt",
  opt: "alt",
  esc: "escape",
  spacebar: "space",
  up: "arrowup",
  down: "arrowdown",
  left: "arrowleft",
  right: "arrowright",
  return: "enter",
  del: "delete",
};

/**
 * Punctuation read by position, so Ctrl-Shift-. (journal seek) and Ctrl-[
 * (back) are the same keys on every layout. Other punctuation is read as
 * the character it types: on a German keyboard `+` and `-` sit where US
 * keyboards have `]` and `/`, and Ctrl-+ must still zoom in.
 */
const PHYSICAL_KEYS: Record<string, string> = {
  Period: ".",
  Comma: ",",
  BracketLeft: "[",
  BracketRight: "]",
};
const PHYSICAL_CHARACTERS = new Set(Object.values(PHYSICAL_KEYS));

export function isMacPlatform(): boolean {
  return typeof navigator !== "undefined" && /Mac|iPhone|iPad/.test(navigator.platform);
}

function isModifier(part: string): part is Modifier {
  return (MODIFIER_ORDER as readonly string[]).includes(part);
}

/**
 * A typed punctuation character already says whether Shift was needed
 * (`?` is Shift-/ on one layout and a plain key on another), so Shift is not
 * part of its binding. `plus` names the `+` character, which separates keys.
 */
function shiftIsImplied(key: string): boolean {
  return key === "plus" || (key.length === 1 && !/[a-z0-9]/.test(key) && !PHYSICAL_CHARACTERS.has(key));
}

function canonicalChord(chord: string, mac: boolean): string {
  const modifiers = new Set<Modifier>();
  let key = "";
  for (const raw of chord.split("+")) {
    const lower = raw.trim().toLowerCase();
    if (!lower) continue;
    const part = ALIASES[lower] ?? lower;
    if (isModifier(part)) {
      // `mod` is Ctrl elsewhere and Cmd on macOS; spell it one way.
      modifiers.add(part === "ctrl" && !mac ? "mod" : part === "meta" && mac ? "mod" : part);
    } else {
      key = part;
    }
  }
  if (shiftIsImplied(key)) modifiers.delete("shift");
  return [...MODIFIER_ORDER.filter((modifier) => modifiers.has(modifier)), key].filter(Boolean).join("+");
}

/** The canonical spelling of a binding string, for storage and comparison. */
export function canonicalBinding(binding: string, mac = isMacPlatform()): string {
  return binding
    .trim()
    .split(/\s+/)
    .filter(Boolean)
    .map((chord) => canonicalChord(chord, mac))
    .join(" ");
}

/** The presses of a binding: one for a combo, several for a sequence. */
export function bindingChords(binding: string, mac = isMacPlatform()): string[] {
  const canonical = canonicalBinding(binding, mac);
  return canonical ? canonical.split(" ") : [];
}

function chordParts(chord: string): { modifiers: Modifier[]; key: string } {
  const parts = chord.split("+").filter(Boolean);
  const key = parts.length && !isModifier(parts[parts.length - 1]) ? parts[parts.length - 1] : "";
  return { modifiers: parts.filter(isModifier), key };
}

/** Whether a single press holds Ctrl, Cmd or Alt (Shift alone does not count). */
export function chordHasCommandModifier(chord: string): boolean {
  return chordParts(chord).modifiers.some((modifier) => modifier !== "shift");
}

/** F1 to F24. */
export function isFunctionKey(key: string): boolean {
  return /^f([1-9]|1[0-9]|2[0-4])$/.test(key);
}

export function chordKey(chord: string): string {
  return chordParts(chord).key;
}

const IGNORED_KEYS = new Set([
  "Control", "Alt", "Shift", "Meta", "AltGraph", "OS", "Hyper", "Super", "Unidentified", "Process",
]);

/** Punctuation or a symbol, as opposed to a letter, digit or named key. */
function isPunctuation(key: string): boolean {
  return key.length === 1 && /[\p{P}\p{S}]/u.test(key);
}

/** A letter, digit, `.`, `,`, `[` or `]` named by where it sits on a US keyboard. */
function positionKeyName(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3).toLowerCase();
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  return PHYSICAL_KEYS[code] ?? null;
}

function eventKeyName(event: KeyboardEvent, mac: boolean): string | null {
  if (event.key === "+") return "plus";
  const commandHeld = event.ctrlKey || event.metaKey || event.altKey;
  // A dead key types nothing by itself, but with Ctrl, Cmd or Alt held its
  // position still names it: `[` is a dead `^` on French keyboards.
  if (event.key === "Dead") return commandHeld ? positionKeyName(event.code) : null;
  // With Ctrl, Cmd or Alt held, read letters by position: Alt on macOS turns
  // letters into symbols, and Ctrl-Shift-J may report "J" or a translated key.
  if (commandHeld && /^Key[A-Z]$/.test(event.code)) return event.code.slice(3).toLowerCase();
  // Digits too, unless the key types punctuation there: AZERTY keyboards put
  // `-` on the 6 key, and Ctrl-- must still zoom out. Option on macOS turns
  // digits into symbols, so with it held the position wins.
  if (commandHeld && /^Digit[0-9]$/.test(event.code) && (!isPunctuation(event.key) || (mac && event.altKey))) {
    return event.code.slice(5);
  }
  if (PHYSICAL_KEYS[event.code]) return PHYSICAL_KEYS[event.code];
  if (event.key === " ") return "space";
  const key = event.key.toLowerCase();
  return ALIASES[key] ?? key;
}

/**
 * The binding for one key event, or `null` for a lone modifier, an unknown
 * key, text composition, or AltGr (which types characters such as `@` on many
 * layouts and must never trigger a Ctrl+Alt shortcut).
 */
export function bindingFromEvent(event: KeyboardEvent, mac = isMacPlatform()): string | null {
  if (event.isComposing || event.keyCode === 229) return null;
  if (event.getModifierState?.("AltGraph")) return null;
  if (IGNORED_KEYS.has(event.key)) return null;
  const key = eventKeyName(event, mac);
  if (!key) return null;
  const modifiers: string[] = [];
  if (event.ctrlKey) modifiers.push("ctrl");
  if (event.metaKey) modifiers.push("meta");
  if (event.altKey) modifiers.push("alt");
  if (event.shiftKey) modifiers.push("shift");
  return canonicalChord([...modifiers, key].join("+"), mac);
}

/** Whether a key event presses `binding` (combos only; sequences never match one event). */
export function eventMatchesBinding(event: KeyboardEvent, binding: string, mac = isMacPlatform()): boolean {
  const chords = bindingChords(binding, mac);
  return chords.length === 1 && bindingFromEvent(event, mac) === chords[0];
}
