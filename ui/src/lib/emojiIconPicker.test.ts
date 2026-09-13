import { describe, expect, it } from "vitest";
import { EMOJI_ICON_SLASH_COMMANDS, emojiIconMenuBeforeCursor, iconHtmlForName } from "./emojiIconPicker";

describe("emoji/icon picker", () => {
  it("provides working entry points for the slash command menu", () => {
    expect(EMOJI_ICON_SLASH_COMMANDS.map((command) => command.label)).toEqual(["/em", "/emoji", "/icon"]);
    for (const command of EMOJI_ICON_SLASH_COMMANDS) {
      expect(emojiIconMenuBeforeCursor(command.apply)?.entries.length).toBeGreaterThan(0);
    }
  });

  it("opens a mixed picker for /em", () => {
    const menu = emojiIconMenuBeforeCursor("hello /em");
    expect(menu?.entries.some((entry) => entry.kind === "emoji")).toBe(true);
    expect(menu?.entries.some((entry) => entry.kind === "icon")).toBe(true);
  });

  it("filters /emoji to emoji entries", () => {
    const menu = emojiIconMenuBeforeCursor("/emoji smile");
    expect(menu?.entries.length).toBeGreaterThan(0);
    expect(menu?.entries.every((entry) => entry.kind === "emoji")).toBe(true);
    expect(menu?.entries.some((entry) => entry.name.includes("smile"))).toBe(true);
  });

  it("filters /icon to icon shortcodes", () => {
    const menu = emojiIconMenuBeforeCursor("/icon star");
    expect(menu?.entries.length).toBeGreaterThan(0);
    expect(menu?.entries.every((entry) => entry.kind === "icon")).toBe(true);
    expect(menu?.entries[0].insert).toMatch(/^:icon-/);
  });

  it("does not hijack other slash words", () => {
    expect(emojiIconMenuBeforeCursor("/embed")).toBeNull();
  });

  it("renders known icon shortcodes only", () => {
    expect(iconHtmlForName("star")).toContain("grafium-icon");
    expect(iconHtmlForName("missing")).toBeNull();
  });
});
