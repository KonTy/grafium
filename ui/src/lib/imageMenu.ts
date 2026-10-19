/**
 * The size choices a right-click on an image offers. Both page editors show the
 * same list, so it is built here; what a choice does to the page is up to the editor.
 */
import type { ContextMenuSection } from "./contextMenuSections";
import {
  formatImageScale,
  formatScaledImageDimensions,
  IMAGE_SIZE_SCALES,
  scaledImageDimensions,
} from "./imageSizing";

export interface ImageSizeChoices {
  /** How big the image is shown now; the scales are multiples of it. */
  baseWidth: number;
  baseHeight: number;
  /** Shows the image `width` pixels wide. */
  setWidth(width: number): void | Promise<void>;
  /** Drops the width so the image is shown at its own size again. */
  reset(): void | Promise<void>;
}

export function buildImageSizeSection({
  baseWidth,
  baseHeight,
  setWidth,
  reset,
}: ImageSizeChoices): ContextMenuSection {
  return {
    id: "image-size",
    title: "Size",
    items: [
      { id: "image.size.original", label: "Original Size", onSelect: () => reset() },
      ...IMAGE_SIZE_SCALES.map((scale) => ({
        id: `image.size.${scale}`,
        label: formatImageScale(scale),
        hint: formatScaledImageDimensions(scale, baseWidth, baseHeight),
        onSelect: () => setWidth(scaledImageDimensions(scale, baseWidth, baseHeight).width),
      })),
    ],
  };
}
