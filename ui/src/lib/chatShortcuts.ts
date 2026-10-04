import type { Shortcut } from "./keymap";
import { hasKeyboardOverlay } from "./mainPaneScroll";

/** Keyboard control of the Chat composer's Notes context and Answer mode menus. */
export const CHAT_CONTEXT_NEXT = "alt+n";
export const CHAT_CONTEXT_PREVIOUS = "alt+shift+n";
export const CHAT_MODE_NEXT = "alt+a";
export const CHAT_MODE_PREVIOUS = "alt+shift+a";

export type CycleStep = 1 | -1;

export interface ChatComposerControls {
  /** The conversation's root element, so keys reach the chat that has focus. */
  root(): Element | null | undefined;
  /** A side-panel chat answers only while focused; the Chat screen always does. */
  compact: boolean;
  cycleContext(step: CycleStep): void;
  cycleMode(step: CycleStep): void;
}

const composers = new Set<ChatComposerControls>();

/** Makes an active conversation reachable by the Chat shortcuts until disposed. */
export function registerChatComposer(controls: ChatComposerControls): () => void {
  composers.add(controls);
  return () => { composers.delete(controls); };
}

/** The chat a shortcut acts on: the focused one, else the open Chat screen. */
export function chatComposerTarget(doc: Document = document): ChatComposerControls | null {
  if (composers.size === 0 || hasKeyboardOverlay(doc)) return null;
  const focused = doc.activeElement;
  let screen: ChatComposerControls | null = null;
  for (const composer of composers) {
    if (focused && composer.root()?.contains(focused)) return composer;
    if (!composer.compact) screen ??= composer;
  }
  return screen;
}

/** The next enabled choice after `current` in the `step` direction, wrapping around. */
export function cycleChoice<T>(
  choices: readonly { value: T; enabled: boolean }[],
  current: T,
  step: CycleStep,
): T | null {
  const found = choices.findIndex((choice) => choice.value === current);
  const start = found >= 0 ? found : step > 0 ? -1 : choices.length;
  for (let offset = 1; offset <= choices.length; offset += 1) {
    const index = (((start + step * offset) % choices.length) + choices.length) % choices.length;
    if (index === found) return null;
    if (choices[index].enabled) return choices[index].value;
  }
  return null;
}

/** Shortcuts that only claim their keys while a chat can receive them. */
export function chatComposerShortcuts(): Shortcut[] {
  const shortcut = (id: string, description: string, binding: string,
    run: (target: ChatComposerControls) => void): Shortcut => ({
    id, description, binding, category: "chat", navOnly: false,
    when: () => chatComposerTarget() !== null,
    action: () => {
      const target = chatComposerTarget();
      if (target) run(target);
    },
  });
  const context = "Next notes context in Chat (Shift for previous)";
  const mode = "Next answer mode in Chat (Shift for previous)";
  return [
    shortcut("chat-context", context, CHAT_CONTEXT_NEXT, (chat) => chat.cycleContext(1)),
    shortcut("chat-context", context, CHAT_CONTEXT_PREVIOUS, (chat) => chat.cycleContext(-1)),
    shortcut("chat-mode", mode, CHAT_MODE_NEXT, (chat) => chat.cycleMode(1)),
    shortcut("chat-mode", mode, CHAT_MODE_PREVIOUS, (chat) => chat.cycleMode(-1)),
  ];
}
