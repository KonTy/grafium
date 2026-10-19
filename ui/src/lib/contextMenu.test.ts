import { describe, expect, it } from "vitest";
import { calibrateZoom, clampContextMenuPosition, placeContextMenu } from "./contextMenu";

describe("placeContextMenu", () => {
  const viewport = { width: 1000, height: 700 };
  const size = { width: 200, height: 100 };

  it("opens at the cursor when nothing is zoomed", () => {
    expect(placeContextMenu({ anchor: { x: 400, y: 300 }, size, viewport })).toMatchObject({
      left: 400,
      top: 300,
    });
  });

  it.each([1.25, 1.5, 0.8])(
    "converts viewport pixels into CSS pixels so the menu paints at the cursor at zoom %s",
    (zoom) => {
      const placed = placeContextMenu({ anchor: { x: 400, y: 300 }, size, viewport, zoom });
      // A fixed element is painted at left * zoom, which must be where the user clicked.
      expect(placed.left * zoom).toBeCloseTo(400, 6);
      expect(placed.top * zoom).toBeCloseTo(300, 6);
    }
  );

  it("subtracts the containing block's origin before scaling", () => {
    const placed = placeContextMenu({
      anchor: { x: 410, y: 320 },
      size,
      viewport,
      zoom: 2,
      origin: { x: 10, y: 20 },
    });
    expect(placed).toMatchObject({ left: 200, top: 150 });
  });

  it("flips to the left of and above the cursor when it would run off the screen", () => {
    expect(placeContextMenu({ anchor: { x: 950, y: 650 }, size, viewport })).toMatchObject({
      left: 750,
      top: 550,
    });
  });

  it("flips around the trigger's top edge when anchored below a button", () => {
    const placed = placeContextMenu({
      anchor: { x: 100, y: 690 },
      flipAnchor: { x: 100, y: 660 },
      size,
      viewport,
    });
    expect(placed.top).toBe(560);
  });

  it("pins to the screen edge when it fits on neither side", () => {
    const placed = placeContextMenu({
      anchor: { x: 200, y: 50 },
      size: { width: 280, height: 100 },
      viewport: { width: 300, height: 700 },
    });
    expect(placed.left).toBe(12);
  });

  it("limits a menu taller than the screen and keeps it at the top", () => {
    const placed = placeContextMenu({
      anchor: { x: 100, y: 400 },
      size: { width: 200, height: 900 },
      viewport,
      zoom: 1.25,
    });
    expect(placed.top * 1.25).toBeCloseTo(8, 6);
    expect(placed.maxHeight * 1.25).toBeCloseTo(684, 6);
  });

  it("never leaves the top-left margin", () => {
    expect(placeContextMenu({ anchor: { x: 1, y: 2 }, size, viewport })).toMatchObject({
      left: 8,
      top: 8,
    });
  });
});

describe("calibrateZoom", () => {
  it("measures rendered pixels per CSS pixel", () => {
    expect(calibrateZoom(1250, 1000)).toBe(1.25);
  });

  it("falls back to no zoom when nothing is laid out", () => {
    expect(calibrateZoom(0, 1000)).toBe(1);
    expect(calibrateZoom(Number.NaN, 1000)).toBe(1);
    expect(calibrateZoom(100, 0)).toBe(1);
  });
});

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
