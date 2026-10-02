import { describe, expect, it } from "vitest";
import { chatLayout } from "./chatLayout";

describe("docked Chat layout", () => {
  it("assigns the left reading margin to history instead of an empty gutter", () => {
    const wide = chatLayout(1400, 0, 0);
    const narrow = chatLayout(1400, 0, 15);
    expect(wide.history).toBe(240);
    expect(narrow.history).toBe(wide.history + 210);
    expect(narrow.inset).toBe(210);
  });
  it("retains manual adjustments when changing reading width", () => {
    expect(chatLayout(1400, 100, 15).history - chatLayout(1400, 100, 0).history).toBe(210);
    expect(chatLayout(1400, -300, 15).history).toBe(180);
  });
  it("protects the conversation at extreme settings without losing the saved offset", () => {
    for (const width of [641, 760, 900, 1400]) {
      const layout = chatLayout(width, 2000, 40);
      expect(width - layout.history - layout.inset - 32).toBeGreaterThanOrEqual(400);
      expect(layout.history).toBeGreaterThanOrEqual(180);
    }
    expect(chatLayout(1400, 300, 0).history).toBe(540);
  });
  it("uses actual available space, not the window width, for the mobile drawer", () => {
    expect(chatLayout(500, 100, 40)).toMatchObject({ drawer: true, inset: 0 });
    expect(chatLayout(640, 100, 40).drawer).toBe(true);
    expect(chatLayout(641, 100, 40).drawer).toBe(false);
  });
});
