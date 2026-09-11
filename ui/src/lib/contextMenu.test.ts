import { describe, expect, it } from "vitest";
import { clampContextMenuPosition } from "./contextMenu";

describe("context menu positioning", () => {
  it("keeps menus inside the viewport", () => {
    expect(
      clampContextMenuPosition(790, 590, {
        width: 220,
        height: 160,
        margin: 8,
        viewportWidth: 800,
        viewportHeight: 600,
      })
    ).toEqual({ x: 572, y: 432 });
  });

  it("keeps a margin from the top and left edges", () => {
    expect(
      clampContextMenuPosition(2, 3, {
        width: 220,
        height: 160,
        margin: 8,
        viewportWidth: 800,
        viewportHeight: 600,
      })
    ).toEqual({ x: 8, y: 8 });
  });
});
