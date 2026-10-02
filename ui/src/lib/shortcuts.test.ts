import { afterEach, describe, expect, it, vi } from "vitest";
import { bindingTitle, bindingAria, formatBinding, formatBindingList, groupShortcutRows, shortcutTitle, shortcutAria } from "./shortcuts";
import { keymap_manager, type Shortcut } from "./keymap";

afterEach(() => {
  vi.restoreAllMocks();
  keymap_manager.register([]);
});

describe("formatBinding", () => {
  it("renders modifier combos in Ctrl-Shift-J form", () => {
    expect(formatBinding("mod+shift+j")).toBe("Ctrl-Shift-J");
    expect(formatBinding("mod+k")).toBe("Ctrl-K");
    expect(formatBinding("mod+alt+b")).toBe("Ctrl-Alt-B");
  });

  it("renders journal seek aliases as Ctrl-> and Ctrl-<", () => {
    expect(formatBinding("mod+shift+.")).toBe("Ctrl->");
    expect(formatBinding("mod+shift+,")).toBe("Ctrl-<");
  });

  it("leaves vim chords unchanged", () => {
    expect(formatBinding("g j")).toBe("g j");
  });

  it("distinguishes the macOS platform modifier from an explicit Control key", () => {
    vi.spyOn(navigator, "platform", "get").mockReturnValue("MacIntel");
    expect(formatBinding("mod+alt+b")).toBe("Cmd-Alt-B");
    expect(formatBinding("ctrl+k")).toBe("Ctrl-K");
    expect(formatBinding("meta+k")).toBe("Cmd-K");
    expect(bindingAria("mod+alt+m")).toBe("Meta+Alt+m");
    expect(bindingTitle("Zoom in", "mod+=")).toBe("Zoom in (Cmd-=)");
  });
});

describe("registered shortcut hints", () => {
  it("shows real aliases, leaving unbound actions and ARIA chord sequences alone", () => {
    keymap_manager.register([
      { id: "journal", binding: "g j", action: () => {} },
      { id: "journal", binding: "mod+shift+j", action: () => {} },
      { id: "journal", binding: "mod+shift+j", action: () => {} },
      { id: "library", binding: "g l", action: () => {} },
    ]);
    expect(shortcutTitle("Journal", "journal")).toBe("Journal (g j | Ctrl-Shift-J)");
    expect(shortcutAria("journal")).toBe("Control+Shift+j");
    expect(shortcutTitle("Library", "library")).toBe("Library (g l)");
    expect(shortcutAria("library")).toBeUndefined();
    expect(shortcutTitle("Delete", "delete")).toBe("Delete");
    expect(shortcutAria("delete")).toBeUndefined();
  });
});

describe("groupShortcutRows", () => {
  it("groups aliases onto one row with chord and modifier columns", () => {
    const shortcuts: Shortcut[] = [
      { id: "go-journal", description: "Go to today's journal", category: "navigation", binding: "g j", action: () => {} },
      {
        id: "go-journal",
        description: "Go to today's journal",
        category: "navigation",
        binding: "mod+shift+j",
        action: () => {},
      },
      { id: "go-tasks", description: "Go to tasks", category: "navigation", binding: "mod+shift+t", action: () => {} },
    ];
    const rows = groupShortcutRows(shortcuts);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({
      id: "go-journal",
      chords: ["g j"],
      modifiers: ["mod+shift+j"],
    });
    expect(formatBindingList(rows[0].chords.concat(rows[0].modifiers))).toBe("g j | Ctrl-Shift-J");
    expect(rows[1].chords).toEqual([]);
    expect(rows[1].modifiers).toEqual(["mod+shift+t"]);
  });
});
