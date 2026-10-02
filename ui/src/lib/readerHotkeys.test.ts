import { afterEach, describe, expect, it, vi } from "vitest";
import { BIONIC_SHORTCUT, BOOKMARK_SHORTCUT, readerShortcut } from "./readerHotkeys";

afterEach(() => vi.restoreAllMocks());

function event(init: KeyboardEventInit = {}) {
  return new KeyboardEvent("keydown", {
    key: "b", code: "KeyB", ctrlKey: true, altKey: true, cancelable: true, ...init,
  });
}

describe("isolated reader shortcuts", () => {
  it("exports the registered default bindings", () => {
    expect(BIONIC_SHORTCUT).toBe("mod+alt+b");
    expect(BOOKMARK_SHORTCUT).toBe("mod+alt+m");
  });

  it.each(["Linux x86_64", "Win32", "MacIntel"])("matches the actual modifier on %s and physical non-English keys", (platform) => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue(platform);
    const modifiers = { ctrlKey: platform !== "MacIntel", metaKey: platform === "MacIntel" };
    expect(readerShortcut(event({ ...modifiers, key: "и" }))).toBe("toggle-bionic");
    expect(readerShortcut(event({ ...modifiers, key: "µ", code: "KeyM" }))).toBe("bookmark");
    expect(readerShortcut(event({ ctrlKey: !modifiers.ctrlKey, metaKey: !modifiers.metaKey }))).toBeNull();
    expect(readerShortcut(event({ ctrlKey: true, metaKey: true }))).toBeNull();
  });

  it.each([
    { repeat: true }, { isComposing: true }, { keyCode: 229 }, { shiftKey: true },
    { ctrlKey: false }, { altKey: false }, { metaKey: true },
    { code: "KeyN" }, { code: "Digit1" },
  ])("leaves unintended input alone: %j", (init) => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("Linux");
    expect(readerShortcut(event(init))).toBeNull();
  });

  it("ignores AltGraph and already handled events", () => {
    const altGraph = event();
    vi.spyOn(altGraph, "getModifierState").mockImplementation(key => key === "AltGraph");
    expect(readerShortcut(altGraph)).toBeNull();
    const prevented = event();
    prevented.preventDefault();
    expect(readerShortcut(prevented)).toBeNull();
  });

  it("falls back to the letter only when no physical code is provided", () => {
    expect(readerShortcut(event({ code: "", key: "B" }))).toBe("toggle-bionic");
    expect(readerShortcut(event({ code: "", key: "M" }))).toBe("bookmark");
  });
});
