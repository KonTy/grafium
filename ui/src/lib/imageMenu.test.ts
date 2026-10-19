import { describe, expect, it, vi } from "vitest";
import type { ContextMenuAction } from "./contextMenuSections";
import { buildImageSizeSection } from "./imageMenu";
import { IMAGE_SIZE_SCALES, formatImageScale, scaledImageDimensions } from "./imageSizing";

function build(overrides: Partial<Parameters<typeof buildImageSizeSection>[0]> = {}) {
  const setWidth = vi.fn();
  const reset = vi.fn();
  const section = buildImageSizeSection({ baseWidth: 200, baseHeight: 100, setWidth, reset, ...overrides });
  return { section, setWidth, reset, items: section.items as ContextMenuAction[] };
}

describe("buildImageSizeSection", () => {
  it("is one titled section, so the menu keeps it apart from the other choices", () => {
    const { section } = build();

    expect(section.id).toBe("image-size");
    expect(section.title).toBe("Size");
  });

  it("offers the original size first, then every scale", () => {
    const { items } = build();

    expect(items.map((item) => item.label)).toEqual([
      "Original Size",
      ...IMAGE_SIZE_SCALES.map(formatImageScale),
    ]);
  });

  it("gives every item its own id, which the menu uses to drop repeats", () => {
    const { items } = build();

    expect(new Set(items.map((item) => item.id)).size).toBe(items.length);
  });

  it("shows the size each scale would give, next to its name", () => {
    const { items } = build({ baseWidth: 200, baseHeight: 100 });

    const half = items.find((item) => item.label === "1/2x");
    const double = items.find((item) => item.label === "2x");
    expect(half?.hint).toBe("100x50px");
    expect(double?.hint).toBe("400x200px");
  });

  it("sets the width a scale gives when it is chosen", () => {
    const { items, setWidth } = build({ baseWidth: 300, baseHeight: 150 });

    for (const scale of IMAGE_SIZE_SCALES) {
      const item = items.find((candidate) => candidate.label === formatImageScale(scale));
      item?.onSelect();
      expect(setWidth).toHaveBeenLastCalledWith(scaledImageDimensions(scale, 300, 150).width);
    }
    expect(setWidth).toHaveBeenCalledTimes(IMAGE_SIZE_SCALES.length);
  });

  it("resets the width, and only that, for the original size", () => {
    const { items, reset, setWidth } = build();

    items[0].onSelect();

    expect(reset).toHaveBeenCalledTimes(1);
    expect(reset).toHaveBeenCalledWith();
    expect(setWidth).not.toHaveBeenCalled();
  });

  it("hands back what the editor's own action returns, so a failure can be reported", async () => {
    const failure = new Error("could not save");
    const { items } = build({ setWidth: () => Promise.reject(failure) });

    await expect(items[1].onSelect()).rejects.toBe(failure);
  });
});
