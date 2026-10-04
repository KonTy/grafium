import { hasKeyboardOverlay } from "./mainPaneScroll";

/**
 * Keyboard control of the Chat composer's Notes context and Answer mode
 * menus. The keys themselves are set in Settings > Keyboard Shortcuts > Chat
 * (Alt+N / Alt+Shift+N and Alt+A / Alt+Shift+A by default).
 */

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

const chatAvailable = () => chatComposerTarget() !== null;

function onChat(run: (target: ChatComposerControls) => void): () => void {
  return () => {
    const target = chatComposerTarget();
    if (target) run(target);
  };
}

/**
 * The Chat actions by shortcut id. Each claims its keys only while a chat
 * can receive them, so the same keys keep working elsewhere.
 */
export const CHAT_SHORTCUT_ACTIONS: Readonly<Record<string, { run: () => void; when: () => boolean }>> = {
  "chat-context-next": { run: onChat((chat) => chat.cycleContext(1)), when: chatAvailable },
  "chat-context-previous": { run: onChat((chat) => chat.cycleContext(-1)), when: chatAvailable },
  "chat-mode-next": { run: onChat((chat) => chat.cycleMode(1)), when: chatAvailable },
  "chat-mode-previous": { run: onChat((chat) => chat.cycleMode(-1)), when: chatAvailable },
};
