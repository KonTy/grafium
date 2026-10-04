import { afterEach, describe, expect, it, vi } from "vitest";
import { chatComposerTarget, cycleChoice, registerChatComposer, type ChatComposerControls } from "./chatShortcuts";

const choices = (enabled: Record<string, boolean>) =>
  Object.entries(enabled).map(([value, on]) => ({ value, enabled: on }));

describe("cycleChoice", () => {
  const menu = choices({ selection: false, page: true, graph: true, library: false, none: true });

  it("moves to the next enabled choice and wraps around in both directions", () => {
    expect(cycleChoice(menu, "page", 1)).toBe("graph");
    expect(cycleChoice(menu, "graph", 1)).toBe("none");
    expect(cycleChoice(menu, "none", 1)).toBe("page");
    expect(cycleChoice(menu, "page", -1)).toBe("none");
    expect(cycleChoice(menu, "none", -1)).toBe("graph");
  });

  it("stays put when nothing else can be chosen", () => {
    expect(cycleChoice(choices({ graph: true, library: false }), "graph", 1)).toBeNull();
    expect(cycleChoice([], "graph", -1)).toBeNull();
  });

  it("starts from the matching end when the current choice is not listed", () => {
    expect(cycleChoice(menu, "book", 1)).toBe("page");
    expect(cycleChoice(menu, "book", -1)).toBe("none");
  });
});

describe("chatComposerTarget", () => {
  const disposers: Array<() => void> = [];

  afterEach(() => {
    disposers.splice(0).forEach((dispose) => dispose());
    document.body.replaceChildren();
  });

  function chat(compact: boolean) {
    const root = document.createElement("section");
    const message = document.createElement("textarea");
    root.append(message);
    document.body.append(root);
    const controls: ChatComposerControls = {
      root: () => root, compact, cycleContext: vi.fn(), cycleMode: vi.fn(),
    };
    disposers.push(registerChatComposer(controls));
    return { controls, message };
  }

  it("acts on the focused chat, otherwise on the Chat screen", () => {
    const screen = chat(false);
    const side = chat(true);
    side.message.focus();
    expect(chatComposerTarget()).toBe(side.controls);
    screen.message.focus();
    expect(chatComposerTarget()).toBe(screen.controls);
    const elsewhere = document.createElement("input");
    document.body.append(elsewhere);
    elsewhere.focus();
    expect(chatComposerTarget()).toBe(screen.controls);
  });

  it("ignores a side-panel chat without focus and any chat behind a dialog", () => {
    const side = chat(true);
    expect(chatComposerTarget()).toBeNull();
    side.message.focus();
    expect(chatComposerTarget()).toBe(side.controls);
    const dialog = document.createElement("div");
    dialog.setAttribute("role", "dialog");
    Object.defineProperty(dialog, "getClientRects", { value: () => [new DOMRect(0, 0, 200, 200)] });
    document.body.append(dialog);
    expect(chatComposerTarget()).toBeNull();
  });

  it("forgets a chat once it is no longer active", () => {
    const { controls } = chat(false);
    expect(chatComposerTarget()).toBe(controls);
    disposers.splice(0).forEach((dispose) => dispose());
    expect(chatComposerTarget()).toBeNull();
  });
});
