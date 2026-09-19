import { describe, expect, it } from "vitest";
import { formatBinding, formatBindingList, groupShortcutRows } from "./shortcuts";
import type { Shortcut } from "./keymap";

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
