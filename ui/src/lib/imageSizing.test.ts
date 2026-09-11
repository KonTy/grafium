import { describe, expect, it } from "vitest";
import {
  formatImageScale,
  formatScaledImageDimensions,
  IMAGE_SIZE_SCALES,
  scaledImageDimensions,
} from "./imageSizing";

describe("image sizing helpers", () => {
  it("includes shrink and enlarge presets in menu order", () => {
    expect(IMAGE_SIZE_SCALES).toEqual([0.25, 0.5, 2, 3, 4, 5, 6, 7, 8]);
    expect(IMAGE_SIZE_SCALES.map(formatImageScale)).toEqual([
      "1/4x",
      "1/2x",
      "2x",
      "3x",
      "4x",
      "5x",
      "6x",
      "7x",
      "8x",
    ]);
  });

  it("formats the scaled image dimensions shown beside menu items", () => {
    expect(formatScaledImageDimensions(0.5, 800, 600)).toBe("400x300px");
    expect(formatScaledImageDimensions(2, 800, 600)).toBe("1600x1200px");
  });

  it("clamps huge scale options to a safe maximum", () => {
    expect(scaledImageDimensions(8, 2000, 1000)).toEqual({ width: 3200, height: 1600 });
  });
});
