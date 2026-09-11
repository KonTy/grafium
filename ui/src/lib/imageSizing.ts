export interface ImageDimensions {
  width: number;
  height: number;
}

export const IMAGE_SIZE_SCALES = [0.25, 0.5, 2, 3, 4, 5, 6, 7, 8] as const;
export type ImageSizeScale = typeof IMAGE_SIZE_SCALES[number];

const MIN_IMAGE_SIZE = 40;
const DEFAULT_IMAGE_WIDTH = 240;
const DEFAULT_IMAGE_HEIGHT = 180;
const MAX_IMAGE_SIZE = 3200;

export function formatImageScale(scale: number): string {
  if (scale === 0.25) return "1/4x";
  if (scale === 0.5) return "1/2x";
  return `${scale}x`;
}

export function scaledImageDimensions(
  scale: number,
  baseWidth = DEFAULT_IMAGE_WIDTH,
  baseHeight = DEFAULT_IMAGE_HEIGHT,
): ImageDimensions {
  const targetWidth = baseWidth * scale;
  const targetHeight = baseHeight * scale;
  const fitScale = Math.min(1, MAX_IMAGE_SIZE / targetWidth, MAX_IMAGE_SIZE / targetHeight);

  return {
    width: Math.max(MIN_IMAGE_SIZE, Math.round(targetWidth * fitScale)),
    height: Math.max(MIN_IMAGE_SIZE, Math.round(targetHeight * fitScale)),
  };
}

export function formatScaledImageDimensions(
  scale: number,
  baseWidth = DEFAULT_IMAGE_WIDTH,
  baseHeight = DEFAULT_IMAGE_HEIGHT,
): string {
  const { width, height } = scaledImageDimensions(scale, baseWidth, baseHeight);
  return `${width}x${height}px`;
}

export function imageIndexFromElement(img: HTMLImageElement): number | null {
  const raw = img.dataset.imageIndex;
  if (!raw) return null;
  const index = Number(raw);
  return Number.isInteger(index) && index >= 0 ? index : null;
}

export function renderedImageBaseSize(img: HTMLImageElement, hostWidth?: number): ImageDimensions {
  const rect = img.getBoundingClientRect();
  const fallback = Math.max(MIN_IMAGE_SIZE * 2, Math.round(rect.width || DEFAULT_IMAGE_WIDTH));
  const naturalWidth = img.naturalWidth || fallback;
  const naturalHeight = img.naturalHeight || rect.height || fallback;
  const aspectRatio = naturalWidth > 0 && naturalHeight > 0 ? naturalWidth / naturalHeight : 1;
  const displayedWidth = rect.width > 0 ? rect.width : Math.min(naturalWidth, hostWidth || fallback);
  const width = Math.max(MIN_IMAGE_SIZE * 2, Math.round(displayedWidth));

  return {
    width,
    height: Math.max(MIN_IMAGE_SIZE, Math.round(width / aspectRatio)),
  };
}

export function assetPathFromImageUrl(urlText: string | null | undefined): string | null {
  if (!urlText) return null;
  try {
    const url = new URL(urlText);
    if (url.protocol !== "grafium-asset:") return null;
    return decodeURIComponent(url.pathname.replace(/^\/+/, ""));
  } catch {
    return null;
  }
}

export function imageSourceUrl(img: HTMLImageElement): string {
  return img.currentSrc || img.getAttribute("src") || img.dataset.src || "";
}
